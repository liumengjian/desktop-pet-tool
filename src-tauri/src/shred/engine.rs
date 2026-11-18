//! 粉碎引擎：计划（遍历 / 统计）→ 覆盖写 → 删除 → 失败时解除占用重试。
//!
//! 全部使用 std + windows crate，不新增依赖；任务跑在后台线程，按时间节流推送进度。
//!
//! 安全约定：
//! - 目标是磁盘根目录、`%SystemRoot%` 本身时直接拒绝；
//! - 目录里的符号链接 / 联接点只删链接本身，绝不顺着链接删到别的位置；
//! - 只读 / 隐藏 / 系统属性在删除前清掉，否则 Windows 不允许删；
//! - 系统关键进程不强制结束（见 `lock::PROTECTED`），删不掉的登记为「重启后删除」。

use std::fs;
use std::io::Write;
#[cfg(target_os = "windows")]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::lock;

const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// 覆盖写块大小：1 MiB。机械盘也够快，SSD 上基本是连续写。
const CHUNK: usize = 1024 * 1024;
/// 一次粉碎最多展开这么多条（目录 + 文件），防止超大树把内存吃满。
const MAX_ENTRIES: usize = 200_000;

fn tr(zh: &str, en: &str) -> String {
    crate::i18n::t(zh, en)
}

#[derive(Clone, Debug)]
pub struct Options {
    /// 被占用时是否结束占用进程
    pub kill_locks: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { kill_locks: true }
    }
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub files_done: u64,
    pub files_total: u64,
    /// 正在处理的文件 / 目录名
    pub current: String,
    pub percent: u8,
}

#[derive(Default, Debug)]
pub struct Summary {
    /// 成功删除的文件数
    pub ok_files: u64,
    /// 成功删除的目录数
    pub ok_dirs: u64,
    pub failed: u64,
    /// 已粉碎文件的总字节数
    pub freed: u64,
    /// 被强制结束的占用进程
    pub killed: Vec<String>,
    pub failed_items: Vec<String>,
    /// 已登记为重启后删除的条目
    pub reboot: Vec<String>,
    pub canceled: bool,
}

struct FileEntry {
    path: PathBuf,
    /// 符号链接 / 联接点：只删链接，不覆盖写
    link: bool,
    size: u64,
}

#[derive(Default)]
struct Plan {
    files: Vec<FileEntry>,
    /// 目录（删除时按层级从深到浅）
    dirs: Vec<PathBuf>,
    bytes: u64,
}

/// 测量单个目标（给确认页展示用）：返回（是否目录，字节数，文件数，错误）
pub fn measure(path: &Path) -> (bool, u64, u64, Option<String>) {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => return (false, 0, 0, Some(io_reason(&e))),
    };
    // 链接 / 联接点：只算链接本身，不跟进去
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return (fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false), 0, 0, None);
    }
    if meta.is_file() {
        return (false, meta.len(), 1, None);
    }
    if !meta.is_dir() {
        return (false, 0, 0, Some(tr("不是文件或文件夹", "Not a file or folder")));
    }
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut stack = vec![path.to_path_buf()];
    let mut steps = 0usize;
    while let Some(dir) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            steps += 1;
            if steps > MAX_ENTRIES {
                return (true, bytes, files, None);
            }
            let p = entry.path();
            let md = match fs::symlink_metadata(&p) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                continue;
            }
            if md.is_dir() {
                stack.push(p);
            } else if md.is_file() {
                bytes += md.len();
                files += 1;
            }
        }
    }
    (true, bytes, files, None)
}

/// 拒绝明显不该粉碎的目标。
pub fn guard(path: &Path) -> Option<String> {
    if is_drive_root(path) {
        return Some(tr("不能粉碎磁盘根目录", "Cannot shred a drive root"));
    }
    if let Ok(sys) = std::env::var("SystemRoot") {
        if same_path(path, Path::new(&sys)) {
            return Some(tr(
                "不能粉碎 Windows 系统目录",
                "Cannot shred the Windows folder",
            ));
        }
    }
    None
}

fn is_drive_root(path: &Path) -> bool {
    // "C:\" / "\\server\share" 这类路径没有父目录
    path.parent().is_none()
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_string()
    };
    norm(a).eq_ignore_ascii_case(&norm(b))
}

/// 执行粉碎。`emit` 会被节流调用（约每 120ms 一次）。
pub fn run<F>(targets: &[String], opts: &Options, cancel: &AtomicBool, mut emit: F) -> Summary
where
    F: FnMut(Progress),
{
    let mut sum = Summary::default();

    // 1) 先做计划：能把总大小算准，进度条才有意义
    let mut plans: Vec<Plan> = Vec::new();
    for target in targets {
        let path = PathBuf::from(target);
        if cancel.load(Ordering::SeqCst) {
            sum.canceled = true;
            return sum;
        }
        if !path.exists() {
            sum.failed += 1;
            push_fail(
                &mut sum,
                path.to_string_lossy().as_ref(),
                &tr("路径不存在", "Path not found"),
            );
            continue;
        }
        if let Some(reason) = guard(&path) {
            sum.failed += 1;
            push_fail(&mut sum, path.to_string_lossy().as_ref(), &reason);
            continue;
        }
        match plan_path(&path) {
            Ok(plan) => plans.push(plan),
            Err(e) => {
                sum.failed += 1;
                push_fail(&mut sum, path.to_string_lossy().as_ref(), &e);
            }
        }
    }

    let total: u64 = plans.iter().map(|p| p.bytes).sum();
    let files_total: u64 = plans.iter().map(|p| p.files.len() as u64).sum();
    let mut rep = Reporter {
        emit: &mut emit,
        total,
        done: 0,
        files_total,
        files_done: 0,
        next: Instant::now(),
    };
    rep.flush("", true);

    // 2) 覆盖写 + 删除文件
    for plan in &plans {
        let mut killed_here: Vec<String> = Vec::new();
        for entry in &plan.files {
            if cancel.load(Ordering::SeqCst) {
                sum.canceled = true;
                break;
            }
            let name = entry
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            rep.flush(&name, false);

            let lp = long(&entry.path);
            clear_attrs(&lp);

            let mut write_err: Option<String> = None;
            let mut overwritten = false;
            if !entry.link && entry.size > 0 {
                match overwrite(&lp, entry.size, cancel, &mut rep, &name) {
                    Ok(()) => overwritten = true,
                    Err(e) => {
                        let locked = is_lock_conflict(&e);
                        write_err = Some(io_reason(&e));
                        // 文件被独占打开时，覆盖写本身就会失败；先解除占用再重试，
                        // 不能等到删除阶段才发现锁，否则文件内容可能已经被部分处理。
                        if locked && opts.kill_locks && !cancel.load(Ordering::SeqCst) {
                            let names = lock::release(&[entry.path.to_string_lossy().into_owned()]);
                            if !names.is_empty() {
                                killed_here.extend(names);
                                std::thread::sleep(Duration::from_millis(250));
                                clear_attrs(&lp);
                                match overwrite(&lp, entry.size, cancel, &mut rep, &name) {
                                    Ok(()) => {
                                        overwritten = true;
                                        write_err = None;
                                    }
                                    Err(retry) => write_err = Some(io_reason(&retry)),
                                }
                            }
                        }
                    }
                }
            }
            if cancel.load(Ordering::SeqCst) {
                sum.canceled = true;
                break;
            }

            // 覆盖成功后磁盘上已无数可读，即便删不掉也要把字节数算进「已粉碎」
            if overwritten {
                sum.freed += entry.size;
            }

            let outcome = remove_file_with_recovery(
                &entry.path,
                &lp,
                overwritten,
                entry.size,
                entry.link,
                opts,
                cancel,
                &mut rep,
                &name,
                &mut killed_here,
            );
            for k in killed_here.drain(..) {
                if !sum.killed.iter().any(|n| n.eq_ignore_ascii_case(&k)) {
                    sum.killed.push(k);
                }
            }
            rep.files_done += 1;

            match outcome {
                Outcome::Gone => {
                    sum.ok_files += 1;
                    if !overwritten {
                        sum.freed += entry.size;
                        rep.done = rep.done.saturating_add(entry.size);
                    }
                    rep.flush(&name, true);
                }
                Outcome::Scheduled => {
                    sum.reboot.push(entry.path.to_string_lossy().into_owned());
                }
                Outcome::Failed(reason) => {
                    sum.failed += 1;
                    let full = match &write_err {
                        Some(w) => format!("{reason}（{w}）"),
                        None => reason,
                    };
                    push_fail(&mut sum, entry.path.to_string_lossy().as_ref(), &full);
                }
            }
        }

        if sum.canceled {
            break;
        }

        // 3) 自底向上删空目录
        for dir in &plan.dirs {
            if cancel.load(Ordering::SeqCst) {
                sum.canceled = true;
                break;
            }
            let lp = long(dir);
            clear_attrs(&lp);
            let mut result = fs::remove_dir(&lp).map_err(|e| io_reason(&e));
            if result.is_err() && opts.kill_locks {
                let names = lock::release(&[dir.to_string_lossy().into_owned()]);
                for k in names {
                    if !sum.killed.iter().any(|n| n.eq_ignore_ascii_case(&k)) {
                        sum.killed.push(k);
                    }
                }
                std::thread::sleep(Duration::from_millis(200));
                clear_attrs(&lp);
                result = fs::remove_dir(&lp).map_err(|e| io_reason(&e));
            }
            match result {
                Ok(()) => sum.ok_dirs += 1,
                Err(e) => {
                    // 目录已经不存在（前面删文件时被上级一并带走）不算失败
                    if !dir.exists() {
                        sum.ok_dirs += 1;
                        continue;
                    }
                    sum.failed += 1;
                    push_fail(&mut sum, dir.to_string_lossy().as_ref(), &e);
                    if sum.failed_items.len() >= 24 {
                        // 失败列表限长，避免结尾刷屏
                        break;
                    }
                }
            }
        }
    }

    rep.done = rep.total;
    rep.flush("", true);
    sum
}

// ============== 计划 ==============

fn plan_path(root: &Path) -> Result<Plan, String> {
    let meta = fs::symlink_metadata(root).map_err(|e| io_reason(&e))?;
    let mut plan = Plan::default();

    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        // 目标本身就是链接：只删链接本身。链接指向文件还是目录决定用哪个删除 API。
        let target_is_dir = fs::metadata(root).map(|m| m.is_dir()).unwrap_or(false);
        if target_is_dir {
            plan.dirs.push(root.to_path_buf());
        } else {
            plan.files.push(FileEntry {
                path: root.to_path_buf(),
                link: true,
                size: 0,
            });
        }
        return Ok(plan);
    }

    if meta.is_file() {
        plan.bytes = meta.len();
        plan.files.push(FileEntry {
            path: root.to_path_buf(),
            link: false,
            size: meta.len(),
        });
        return Ok(plan);
    }

    if !meta.is_dir() {
        return Err(tr("不是文件或文件夹", "Not a file or folder"));
    }

    plan.dirs.push(root.to_path_buf());
    let mut stack = vec![root.to_path_buf()];
    let mut steps = 0usize;
    while let Some(dir) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            steps += 1;
            if steps > MAX_ENTRIES {
                return Err(tr(
                    "目录内条目超过 20 万，请分批粉碎",
                    "More than 200,000 entries, please shred in batches",
                ));
            }
            let path = entry.path();
            let md = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                // 链接 / 联接点：只删链接本身，不跟随
                let target_is_dir = fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
                if target_is_dir {
                    plan.dirs.push(path);
                } else {
                    plan.files.push(FileEntry {
                        path,
                        link: true,
                        size: 0,
                    });
                }
                continue;
            }
            if md.is_dir() {
                plan.dirs.push(path.clone());
                stack.push(path);
            } else if md.is_file() {
                plan.bytes += md.len();
                plan.files.push(FileEntry {
                    path,
                    link: false,
                    size: md.len(),
                });
            }
        }
    }

    // 目录按层级从深到浅删，保证父目录删的时候已经空了
    plan.dirs
        .sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    Ok(plan)
}

// ============== 覆盖写 ==============

fn overwrite<F>(
    path: &Path,
    size: u64,
    cancel: &AtomicBool,
    rep: &mut Reporter<'_, F>,
    name: &str,
) -> Result<(), std::io::Error>
where
    F: FnMut(Progress),
{
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(path)?;

    // 先回到文件头，防止被调用方以外的偏移影响（新打开的文件本来就在 0）
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut buf = vec![0u8; CHUNK.min(size as usize).max(1)];
    let mut left = size;
    while left > 0 {
        if cancel.load(Ordering::SeqCst) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                tr("已取消", "Canceled"),
            ));
        }
        let take = left.min(buf.len() as u64) as usize;
        // xorshift64*：便宜、无依赖，够把数据覆盖成不可读
        for word in buf[..take].chunks_exact_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            word.copy_from_slice(&state.to_le_bytes());
        }
        let tail = take % 8;
        if tail > 0 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let bytes = state.to_le_bytes();
            let start = take - tail;
            buf[start..take].copy_from_slice(&bytes[..tail]);
        }
        file.write_all(&buf[..take])?;
        left -= take as u64;
        rep.done = rep.done.saturating_add(take as u64);
        rep.flush(name, false);
    }
    file.flush()?;
    Ok(())
}

// ============== 删除 ==============

enum Outcome {
    Gone,
    Scheduled,
    Failed(String),
}

#[allow(clippy::too_many_arguments)]
fn remove_file_with_recovery<F>(
    path: &Path,
    long_path: &Path,
    overwritten: bool,
    size: u64,
    link: bool,
    opts: &Options,
    cancel: &AtomicBool,
    rep: &mut Reporter<'_, F>,
    name: &str,
    killed: &mut Vec<String>,
) -> Outcome
where
    F: FnMut(Progress),
{
    let err = match fs::remove_file(long_path) {
        Ok(()) => return Outcome::Gone,
        Err(e) => e,
    };
    let mut reason = io_reason(&err);
    let mut lock_conflict = is_lock_conflict(&err);

    if opts.kill_locks && !cancel.load(Ordering::SeqCst) {
        let names = lock::release(&[path.to_string_lossy().into_owned()]);
        if !names.is_empty() {
            killed.extend(names);
            // 等进程退出、句柄真正释放
            std::thread::sleep(Duration::from_millis(250));
            clear_attrs(long_path);
            if !overwritten && !link && size > 0 {
                let _ = overwrite(long_path, size, cancel, rep, name);
            }
            match fs::remove_file(long_path) {
                Ok(()) => return Outcome::Gone,
                Err(e) => {
                    lock_conflict = is_lock_conflict(&e);
                    reason = io_reason(&e);
                }
            }
        }
    }

    // 仍然删不掉：管理员身份下登记「重启后删除」，至少保证下次开机没有
    if is_admin() && schedule_reboot_delete(long_path) {
        return Outcome::Scheduled;
    }

    let detail = if lock_conflict {
        lock_holder_note(path)
    } else {
        String::new()
    };
    Outcome::Failed(format!("{reason}{detail}"))
}

#[cfg(target_os = "windows")]
fn is_admin() -> bool {
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
}

#[cfg(not(target_os = "windows"))]
fn is_admin() -> bool {
    false
}

/// MOVEFILE_DELAY_UNTIL_REBOOT 只对管理员生效，失败就当作没排上。
#[cfg(target_os = "windows")]
fn schedule_reboot_delete(path: &Path) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};

    let wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe { MoveFileExW(PCWSTR(wide.as_ptr()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT).is_ok() }
}

#[cfg(not(target_os = "windows"))]
fn schedule_reboot_delete(_path: &Path) -> bool {
    false
}

// ============== 小工具 ==============

/// 加 `\\?\` 前缀：绕过 MAX_PATH，长路径也能覆盖 / 删除。
pub fn long(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with(r"\\?\") {
        return PathBuf::from(s.as_ref());
    }
    if let Some(rest) = s.strip_prefix(r"\\") {
        return PathBuf::from(format!(r"\\?\UNC\{rest}"));
    }
    if s.chars().nth(1) == Some(':') {
        return PathBuf::from(format!(r"\\?\{s}"));
    }
    path.to_path_buf()
}

fn clear_attrs(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_NORMAL};

        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(_) => return,
        };
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes()
            & (FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM)
            == 0
        {
            return;
        }
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let _ = SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_NORMAL);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
    }
}

fn push_fail(sum: &mut Summary, path: &str, reason: &str) {
    sum.failed_items.push(format!("{path} — {reason}"));
    if sum.failed_items.len() > 24 {
        sum.failed_items.truncate(24);
    }
}

fn io_reason(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(32) => tr("文件正被其它程序占用", "File is in use by another program"),
        Some(33) => tr("文件被锁定", "File is locked"),
        Some(5) => tr("权限不足（需要管理员）", "Access denied (admin required)"),
        Some(19) => tr("磁盘写保护", "The disk is write-protected"),
        Some(2) => tr("路径不存在", "Path not found"),
        Some(3) => tr("路径不存在", "Path not found"),
        Some(1224) => tr("文件被内存映射占用", "File is mapped by another process"),
        _ => e.to_string(),
    }
}

/// 共享冲突 / 锁定 / 被内存映射：这类错误可以通过解除占用恢复。
fn is_lock_conflict(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(32) | Some(33) | Some(1224))
}

/// 删除失败时补充占用进程，避免只显示“被占用”却不知道关哪个程序。
fn lock_holder_note(path: &Path) -> String {
    let holders = lock::holders(&[path.to_string_lossy().into_owned()]);
    if holders.is_empty() {
        return String::new();
    }
    let names: Vec<String> = holders
        .into_iter()
        .map(|h| {
            if h.pid > 0 {
                format!("{} ({})", h.name, h.pid)
            } else {
                h.name
            }
        })
        .collect();
    if crate::i18n::is_en() {
        format!(" (held by: {})", names.join(", "))
    } else {
        format!("（占用进程：{}）", names.join("、"))
    }
}

/// 进度节流 + 汇总
struct Reporter<'a, F: FnMut(Progress)> {
    emit: &'a mut F,
    total: u64,
    done: u64,
    files_total: u64,
    files_done: u64,
    next: Instant,
}

impl<F: FnMut(Progress)> Reporter<'_, F> {
    fn flush(&mut self, current: &str, force: bool) {
        let now = Instant::now();
        if !force && now < self.next {
            return;
        }
        let pct = if self.total == 0 {
            if self.files_total == 0 { 100 } else { 0 }
        } else {
            ((self.done.min(self.total) * 100) / self.total) as u8
        };
        (self.emit)(Progress {
            done: self.done,
            total: self.total,
            files_done: self.files_done,
            files_total: self.files_total,
            current: current.to_string(),
            percent: pct,
        });
        self.next = now + Duration::from_millis(120);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dpt-shred-test-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn shreds_file_and_folder() {
        let root = scratch("basic");
        let file = root.join("a.txt");
        fs::write(&file, b"hello shredder").unwrap();

        let sub = root.join("sub");
        fs::create_dir_all(sub.join("deep")).unwrap();
        fs::write(sub.join("b.bin"), vec![7u8; 4096]).unwrap();
        fs::write(sub.join("deep").join("c.bin"), vec![9u8; 4096]).unwrap();

        let cancel = AtomicBool::new(false);
        let targets = vec![
            file.to_string_lossy().into_owned(),
            sub.to_string_lossy().into_owned(),
        ];
        let sum = run(&targets, &Options::default(), &cancel, |_| {});

        assert!(!file.exists());
        assert!(!sub.exists());
        assert_eq!(sum.failed, 0, "失败项: {:?}", sum.failed_items);
        assert!(sum.freed >= 4096);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn shreds_exclusively_locked_file() {
        use std::process::{Command, Stdio};

        let root = scratch("locked");
        let file = root.join("locked.txt");
        fs::write(&file, b"secret data that must not survive").unwrap();

        // 用一个独立进程以 FILE_SHARE_NONE 打开文件，模拟被其它程序独占。
        let escaped = file.to_string_lossy().replace('\'', "''");
        let holder_script = root.join("hold.ps1");
        fs::write(
            &holder_script,
            format!(
                "$s=[IO.File]::Open('{}',[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)\nStart-Sleep -Seconds 20\n",
                escaped
            ),
        )
        .unwrap();
        let mut holder = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                holder_script.to_string_lossy().as_ref(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start lock holder");

        let mut locked = false;
        for _ in 0..40 {
            if fs::OpenOptions::new().write(true).open(&file).is_err() {
                locked = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(locked, "helper process did not lock the file");

        let cancel = AtomicBool::new(false);
        let sum = run(
            &[file.to_string_lossy().into_owned()],
            &Options { kill_locks: true },
            &cancel,
            |_| {},
        );

        let _ = holder.kill();
        let _ = holder.wait();
        let _ = fs::remove_dir_all(&root);

        assert!(!file.exists(), "锁定文件未被粉碎");
        assert_eq!(sum.failed, 0, "失败项: {:?}", sum.failed_items);
        assert!(
            sum.killed
                .iter()
                .any(|name| name.to_ascii_lowercase().contains("powershell")),
            "应结束占用进程，实际: {:?}",
            sum.killed
        );
    }

    #[test]
    fn refuses_drive_root() {
        let p = Path::new(r"C:\");
        assert!(guard(p).is_some());
    }
}
