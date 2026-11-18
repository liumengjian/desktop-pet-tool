//! 磁盘清理：垃圾清理 / 大文件 / C 盘专清。
//!
//! 与项目其它模块一致的约定：
//! - 纯 std + windows crate，不新增第三方依赖；
//! - 扫描跑在后台线程、线程数有上限，可随时取消，不阻塞 WebView；
//! - 删除只接受“本次扫描结果里出现过的路径”，后端重新校验白名单，避免前端传任意路径；
//! - 破坏性动作（回收站、驱动、残留、注册表、系统项、迁移）一律要求前端二次确认。

pub mod cache;
pub mod cdrive;
pub mod junk;
pub mod large;
pub mod reg;
pub mod walk;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub const PROGRESS_EVENT: &str = "cleanup-progress";

// ============== 状态 ==============

pub struct CleanupState {
    /// 所有后台任务共享的取消标记；新建任务时复位
    pub cancel: Arc<AtomicBool>,
    pub generation: AtomicU64,
    /// 打开窗口时希望切到的标签页（前端加载完主动来取，避免事件早于监听）
    pub pending_tab: Mutex<String>,
    pub junk: Mutex<Option<junk::JunkReport>>,
    pub large: Mutex<Option<Vec<walk::FileItem>>>,
    pub system: Mutex<Option<cdrive::SystemReport>>,
    pub apps: Mutex<Option<Vec<cdrive::AppEntry>>>,
    pub cache: Mutex<Option<cache::CacheReport>>,
}

impl Default for CleanupState {
    fn default() -> Self {
        CleanupState {
            cancel: Arc::new(AtomicBool::new(false)),
            generation: AtomicU64::new(0),
            pending_tab: Mutex::new("junk".to_string()),
            junk: Mutex::new(None),
            large: Mutex::new(None),
            system: Mutex::new(None),
            apps: Mutex::new(None),
            cache: Mutex::new(None),
        }
    }
}

impl CleanupState {
    pub fn begin(&self) -> u64 {
        self.cancel.store(false, Ordering::SeqCst);
        let g = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        g
    }
}

// ============== 通用结构 ==============

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DiskInfo {
    pub letter: String,
    pub root: String,
    pub total: u64,
    pub free: u64,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CleanResult {
    pub freed: u64,
    pub deleted: u64,
    pub failed: u64,
    pub failed_items: Vec<String>,
    pub note: String,
}

impl CleanResult {
    pub fn merge(&mut self, other: CleanResult) {
        self.freed += other.freed;
        self.deleted += other.deleted;
        self.failed += other.failed;
        for it in other.failed_items {
            if self.failed_items.len() < 20 {
                self.failed_items.push(it);
            }
        }
        if !other.note.is_empty() {
            if self.note.is_empty() {
                self.note = other.note;
            } else {
                self.note.push_str("；");
                self.note.push_str(&other.note);
            }
        }
    }

    pub fn fail(&mut self, path: &Path, err: impl std::fmt::Display) {
        self.failed += 1;
        if self.failed_items.len() < 20 {
            self.failed_items
                .push(format!("{} — {}", path.display(), err));
        }
    }
}

// ============== 路径 / 磁盘 / 权限 ==============

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 展开 %VAR% 形式的环境变量。
pub fn expand(input: &str) -> PathBuf {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('%') {
            let name = &after[..end];
            let value = env_value(name);
            match value {
                Some(v) => out.push_str(&v),
                None => {
                    out.push('%');
                    out.push_str(name);
                    out.push('%');
                }
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
        }
    }
    out.push_str(rest);
    PathBuf::from(out)
}

fn env_value(name: &str) -> Option<String> {
    let key = name.to_ascii_uppercase();
    let keys: [&str; 6] = [
        "TEMP",
        "TMP",
        "LOCALAPPDATA",
        "APPDATA",
        "PROGRAMDATA",
        "USERPROFILE",
    ];
    if keys.contains(&key.as_str()) {
        if let Ok(v) = std::env::var(&key) {
            return Some(v);
        }
    }
    if key == "WINDIR" || key == "SYSTEMROOT" {
        if let Ok(v) = std::env::var("SystemRoot") {
            return Some(v);
        }
        if let Ok(v) = std::env::var("WINDIR") {
            return Some(v);
        }
        return Some("C:\\Windows".to_string());
    }
    if key == "PROGRAMFILES" {
        if let Ok(v) = std::env::var("ProgramFiles") {
            return Some(v);
        }
        return Some("C:\\Program Files".to_string());
    }
    if key == "PROGRAMFILES(X86)" {
        if let Ok(v) = std::env::var("ProgramFiles(x86)") {
            return Some(v);
        }
        return Some("C:\\Program Files (x86)".to_string());
    }
    std::env::var(name).ok()
}

/// 固定磁盘（不含光驱 / U 盘 / 网络盘）。
pub fn fixed_drives() -> Vec<DiskInfo> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW};

    let mut out = Vec::new();
    for c in b'C'..=b'Z' {
        let root = format!("{}:\\", c as char);
        let w = wide(&root);
        let kind = unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) };
        if kind != 3 {
            continue;
        }
        let mut total: u64 = 0;
        let mut free: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(w.as_ptr()),
                None,
                Some(&mut total as *mut u64),
                Some(&mut free as *mut u64),
            )
            .is_ok()
        };
        if !ok || total == 0 {
            continue;
        }
        out.push(DiskInfo {
            letter: format!("{}:", c as char),
            root,
            total,
            free,
        });
    }
    out
}

pub fn is_admin() -> bool {
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
}

pub fn path_on_fixed_drive(path: &Path) -> bool {
    let s = path.to_string_lossy().to_ascii_uppercase();
    if s.len() < 3 {
        return false;
    }
    let root = format!("{}:\\", &s[..1]);
    fixed_drives().iter().any(|d| d.root.eq_ignore_ascii_case(&root))
}

/// 单个目录 / 文件的实际占用（不跟随链接）。
pub fn folder_size(path: &Path, cancel: &AtomicBool) -> u64 {
    if let Ok(md) = std::fs::symlink_metadata(path) {
        if md.is_file() {
            return md.len();
        }
    }
    let spec = walk::WalkSpec {
        root: path.to_path_buf(),
        min_size: 0,
        max_files: 0,
        group_depth: 0,
        threads: walk::default_threads(),
        skip: Vec::new(),
        older_than: None,
        extensions: Vec::new(),
        now: 0,
        age_ranges: Vec::new(),
    };
    walk::walk(&spec, cancel, &|_, _| {}).bytes_seen
}

/// 递归删除目录内容（保留目录本身，`remove_root` 为真时连目录一起尝试删除）。
/// 遇到被占用 / 无权限的文件计入 failed，不影响其余文件。
pub fn purge(path: &Path, cancel: &AtomicBool, remove_root: bool) -> CleanResult {
    let mut res = CleanResult::default();
    let md = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return res,
    };
    use std::os::windows::fs::MetadataExt;
    if md.file_attributes() & 0x400 != 0 {
        return res;
    }
    if md.is_file() {
        let size = md.len();
        match std::fs::remove_file(path) {
            Ok(_) => {
                res.freed += size;
                res.deleted += 1;
            }
            Err(e) => res.fail(path, e),
        }
        return res;
    }
    if !md.is_dir() {
        return res;
    }

    let mut stack: Vec<PathBuf> = vec![path.to_path_buf()];
    let mut visited_dirs: Vec<PathBuf> = Vec::new();
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            res.note = "cancelled".to_string();
            break;
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                res.fail(&dir, e);
                continue;
            }
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    res.fail(&p, e);
                    continue;
                }
            };
            if meta.file_attributes() & 0x400 != 0 {
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
            } else {
                let size = meta.len();
                match std::fs::remove_file(&p) {
                    Ok(_) => {
                        res.freed += size;
                        res.deleted += 1;
                    }
                    Err(e) => res.fail(&p, e),
                }
            }
        }
        visited_dirs.push(dir);
    }
    for d in visited_dirs.iter().rev() {
        let _ = std::fs::remove_dir(d);
    }
    if remove_root {
        let _ = std::fs::remove_dir(path);
    }
    res
}

pub fn reveal(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err("路径不存在".to_string());
    }
    let mut cmd = std::process::Command::new("explorer");
    if p.is_dir() {
        cmd.arg(path);
    } else {
        cmd.arg("/select,").arg(path);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// 隐藏窗口执行命令，返回退出码与输出（合并 stdout / stderr）。
pub fn run_hidden_status(program: &str, args: &[&str]) -> (i32, String) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    match std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            (out.status.code().unwrap_or(-1), text.trim().to_string())
        }
        Err(e) => (-1, e.to_string()),
    }
}

/// 隐藏窗口执行命令，返回是否成功与输出（合并 stdout / stderr）。
pub fn run_hidden(program: &str, args: &[&str]) -> (bool, String) {
    let (code, text) = run_hidden_status(program, args);
    (code == 0, text)
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// 用 PowerShell（-EncodedCommand，避免引号转义问题）执行一段脚本。
pub fn run_powershell(script: &str) -> (bool, String) {
    let utf16: Vec<u8> = script
        .encode_utf16()
        .flat_map(|u| [u as u8, (u >> 8) as u8])
        .collect();
    let encoded = base64(&utf16);
    let (code, text) = run_hidden_status(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-EncodedCommand",
            &encoded,
        ],
    );
    (code == 0, text)
}

// ============== 进度事件 ==============

pub struct Progress {
    app: AppHandle,
    task: &'static str,
    last: Mutex<Instant>,
}

impl Progress {
    pub fn new(app: AppHandle, task: &'static str) -> Self {
        Progress {
            app,
            task,
            last: Mutex::new(Instant::now() - Duration::from_secs(1)),
        }
    }

    pub fn emit(&self, files: u64, bytes: u64, force: bool) {
        {
            let last = self.last.lock().unwrap();
            if !force && last.elapsed() < Duration::from_millis(180) {
                return;
            }
        }
        *self.last.lock().unwrap() = Instant::now();
        let _ = self.app.emit_to(
            "cleanup",
            PROGRESS_EVENT,
            serde_json::json!({
                "task": self.task,
                "files": files,
                "bytes": bytes,
            }),
        );
    }

    pub fn stage(&self, stage: &str) {
        let _ = self.app.emit_to(
            "cleanup",
            PROGRESS_EVENT,
            serde_json::json!({
                "task": self.task,
                "stage": stage,
            }),
        );
    }
}

// ============== 命令 ==============

#[tauri::command]
pub async fn cleanup_open(app: AppHandle, tab: Option<String>) -> Result<(), String> {
    let tab = tab.unwrap_or_else(|| "junk".to_string());
    {
        let state = app.state::<CleanupState>();
        *state.pending_tab.lock().unwrap() = tab.clone();
    }
    if let Some(w) = app.get_webview_window("cleanup") {
        let _ = w.show();
        let _ = w.set_focus();
        let _ = w.emit("cleanup-tab", tab);
        return Ok(());
    }
    let app_thread = app.clone();
    let _ = app.run_on_main_thread(move || {
        match tauri::WebviewWindowBuilder::new(
            &app_thread,
            "cleanup",
            tauri::WebviewUrl::App("cleanup.html".into()),
        )
        .title(crate::i18n::t("磁盘清理", "Disk cleanup"))
        .inner_size(1000.0, 760.0)
        .min_inner_size(900.0, 640.0)
        .resizable(true)
        .center()
        .visible(true)
        .focused(true)
        .build()
        {
            Ok(win) => {
                #[cfg(target_os = "windows")]
                if let Ok(h) = win.hwnd() {
                    crate::set_neutral_titlebar(h.0 as isize);
                }
                let _ = win.set_focus();
            }
            Err(e) => eprintln!("创建清理窗口失败: {e}"),
        }
    });
    Ok(())
}

#[tauri::command]
pub fn cleanup_tab_get(app: AppHandle) -> String {
    let state = app.state::<CleanupState>();
    let tab = state.pending_tab.lock().unwrap().clone();
    tab
}

#[tauri::command]
pub fn cleanup_cancel(app: AppHandle) {
    let state = app.state::<CleanupState>();
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub fn cleanup_disks() -> Vec<DiskInfo> {
    fixed_drives()
}

#[tauri::command]
pub fn cleanup_admin() -> bool {
    is_admin()
}

#[tauri::command]
pub fn cleanup_reveal(path: String) -> Result<(), String> {
    reveal(&path)
}

#[tauri::command]
pub fn cleanup_elevate(app: AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().replace('\'', "''");
    // --elevated 让新实例跳过单实例判定（否则会被当成「已有实例」直接退出）
    let script = format!(
        "Start-Process -FilePath '{}' -Verb RunAs -ArgumentList '--elevated'",
        exe
    );
    let (ok, out) = run_powershell(&script);
    if !ok {
        return Err(if out.is_empty() {
            "提权被取消".to_string()
        } else {
            out
        });
    }
    // 等新实例接过前台后退出当前进程，避免两个托盘图标长期并存
    std::thread::sleep(Duration::from_millis(400));
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub fn cleanup_state_clear(app: AppHandle) {
    let state = app.state::<CleanupState>();
    *state.junk.lock().unwrap() = None;
    *state.large.lock().unwrap() = None;
    *state.system.lock().unwrap() = None;
    *state.apps.lock().unwrap() = None;
    *state.cache.lock().unwrap() = None;
}

async fn blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(f).await {
        Ok(v) => Ok(v),
        Err(e) => Err(format!("任务线程失败: {e}")),
    }
}

#[tauri::command]
pub async fn junk_scan(app: AppHandle) -> Result<junk::JunkReport, String> {
    let handle = app.clone();
    blocking(move || {
        let state = handle.state::<CleanupState>();
        state.begin();
        let cancel = state.cancel.clone();
        let report = junk::scan(&handle, &cancel);
        *handle.state::<CleanupState>().junk.lock().unwrap() = Some(report.clone());
        report
    })
    .await
}

#[tauri::command]
pub async fn junk_clean(
    app: AppHandle,
    ids: Vec<String>,
    confirm: bool,
) -> Result<CleanResult, String> {
    let handle = app.clone();
    blocking(move || {
        let cached = handle
            .state::<CleanupState>()
            .junk
            .lock()
            .unwrap()
            .clone();
        match cached {
            Some(report) => junk::clean(&handle, &report, &ids, confirm),
            None => Err("请先扫描".to_string()),
        }
    })
    .await?
}

#[tauri::command]
pub async fn large_scan(
    app: AppHandle,
    drives: Vec<String>,
    min_mb: u64,
) -> Result<Vec<walk::FileItem>, String> {
    let handle = app.clone();
    blocking(move || {
        let state = handle.state::<CleanupState>();
        state.begin();
        let cancel = state.cancel.clone();
        let list = large::scan(
            &handle,
            &drives,
            min_mb.max(1).saturating_mul(1024 * 1024),
            &cancel,
        )?;
        *handle.state::<CleanupState>().large.lock().unwrap() = Some(list.clone());
        Ok(list)
    })
    .await?
}

#[tauri::command]
pub async fn large_delete(app: AppHandle, paths: Vec<String>) -> Result<CleanResult, String> {
    let handle = app.clone();
    blocking(move || {
        let state = handle.state::<CleanupState>();
        state.cancel.store(false, Ordering::SeqCst);
        let cancel = state.cancel.clone();
        large::delete(&handle, &paths, &cancel)
    })
    .await
}

#[tauri::command]
pub async fn cdrive_analyze(
    app: AppHandle,
    path: Option<String>,
) -> Result<cdrive::AnalyzeResult, String> {
    let handle = app.clone();
    blocking(move || {
        let state = handle.state::<CleanupState>();
        state.begin();
        let cancel = state.cancel.clone();
        cdrive::analyze(&handle, path, &cancel)
    })
    .await?
}

#[tauri::command]
pub async fn cdrive_system(app: AppHandle) -> Result<cdrive::SystemReport, String> {
    let handle = app.clone();
    blocking(move || {
        let state = handle.state::<CleanupState>();
        state.begin();
        let cancel = state.cancel.clone();
        let report = cdrive::system_report(&handle, &cancel);
        *handle.state::<CleanupState>().system.lock().unwrap() = Some(report.clone());
        report
    })
    .await
}

#[tauri::command]
pub async fn cdrive_component_size() -> Result<u64, String> {
    blocking(cdrive::component_store_bytes).await?
}

#[tauri::command]
pub async fn cache_scan(app: AppHandle) -> Result<cache::CacheReport, String> {
    let handle = app.clone();
    blocking(move || {
        let state = handle.state::<CleanupState>();
        state.begin();
        let cancel = state.cancel.clone();
        let report = cache::scan(&handle, &cancel);
        *handle.state::<CleanupState>().cache.lock().unwrap() = Some(report.clone());
        report
    })
    .await
}

#[tauri::command]
pub async fn cache_clean(
    app: AppHandle,
    selections: Vec<cache::CacheSelection>,
    confirm: bool,
) -> Result<CleanResult, String> {
    let handle = app.clone();
    blocking(move || {
        let cached = handle
            .state::<CleanupState>()
            .cache
            .lock()
            .unwrap()
            .clone();
        let report = cached.ok_or_else(|| "请先扫描软件缓存".to_string())?;
        let state = handle.state::<CleanupState>();
        state.cancel.store(false, Ordering::SeqCst);
        let cancel = state.cancel.clone();
        cache::clean(&report, &selections, confirm, &cancel)
    })
    .await?
}

#[tauri::command]
pub async fn cdrive_action(
    app: AppHandle,
    id: String,
    confirm: bool,
) -> Result<CleanResult, String> {
    let handle = app.clone();
    blocking(move || {
        let state = handle.state::<CleanupState>();
        state.cancel.store(false, Ordering::SeqCst);
        let cancel = state.cancel.clone();
        cdrive::action(&handle, &id, confirm, &cancel)
    })
    .await?
}

#[tauri::command]
pub fn apps_list(app: AppHandle) -> Vec<cdrive::AppEntry> {
    cdrive::apps_list(&app)
}

#[tauri::command]
pub async fn apps_sizes(app: AppHandle, paths: Vec<String>) -> Result<Vec<cdrive::AppSize>, String> {
    let handle = app.clone();
    blocking(move || {
        let state = handle.state::<CleanupState>();
        state.cancel.store(false, Ordering::SeqCst);
        let cancel = state.cancel.clone();
        cdrive::app_sizes(&handle, &paths, &cancel)
    })
    .await
}

#[tauri::command]
pub async fn apps_migrate(
    app: AppHandle,
    path: String,
    target_drive: String,
    confirm: bool,
) -> Result<cdrive::MigrateResult, String> {
    let handle = app.clone();
    blocking(move || {
        let state = handle.state::<CleanupState>();
        state.cancel.store(false, Ordering::SeqCst);
        let cancel = state.cancel.clone();
        cdrive::migrate(&handle, &path, &target_drive, confirm, &cancel)
    })
    .await?
}

#[tauri::command]
pub async fn apps_uninstall(
    app: AppHandle,
    path: String,
    confirm: bool,
) -> Result<cdrive::UninstallResult, String> {
    let handle = app.clone();
    blocking(move || {
        let state = handle.state::<CleanupState>();
        state.cancel.store(false, Ordering::SeqCst);
        let cancel = state.cancel.clone();
        cdrive::uninstall(&handle, &path, confirm, &cancel)
    })
    .await?
}
