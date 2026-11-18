//! 大文件扫描与清理：全盘列出超过阈值的大文件，支持逐项勾选删除。

use super::walk::{walk, FileItem, WalkSpec};
use super::{fixed_drives, CleanResult, Progress};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager};

/// 系统目录整体不参与“大文件”扫描：列出来只会诱导用户误删系统文件。
fn skip_prefixes(root: &Path) -> Vec<PathBuf> {
    let mut v = vec![
        root.join("$Recycle.Bin"),
        root.join("System Volume Information"),
        root.join("Recovery"),
        root.join("Boot"),
    ];
    let win = super::expand("%WINDIR%");
    if root.to_string_lossy().to_ascii_uppercase().starts_with("C:\\") {
        v.push(win);
    }
    v
}

fn protected_names() -> Vec<&'static str> {
    vec![
        "pagefile.sys",
        "hiberfil.sys",
        "swapfile.sys",
        "memory.dmp",
        "dumpstack.log.tmp",
        "bootmgr",
        "bootnxt",
        "ntuser.dat",
        "ntuser.dat.log1",
        "ntuser.dat.log2",
        "desktop.ini",
    ]
}

fn is_protected(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if protected_names().contains(&name.as_str()) {
        return true;
    }
    let win = super::expand("%WINDIR%");
    if super::walk::starts_with_ci(path, &win) {
        return true;
    }
    false
}

pub fn scan(
    app: &AppHandle,
    drives: &[String],
    min_bytes: u64,
    cancel: &AtomicBool,
) -> Result<Vec<FileItem>, String> {
    let progress = Progress::new(app.clone(), "large");
    progress.stage("scan");

    let available = fixed_drives();
    let selected: Vec<_> = if drives.is_empty() {
        available.clone()
    } else {
        available
            .into_iter()
            .filter(|d| drives.iter().any(|s| d.letter.eq_ignore_ascii_case(s)))
            .collect()
    };
    if selected.is_empty() {
        return Err("没有可扫描的固定磁盘".to_string());
    }

    let mut all: Vec<FileItem> = Vec::new();
    let mut files_seen = 0u64;
    let mut bytes_seen = 0u64;

    for disk in &selected {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let root = PathBuf::from(&disk.root);
        let spec = WalkSpec {
            root: root.clone(),
            min_size: min_bytes,
            max_files: 4000,
            group_depth: 0,
            threads: super::walk::default_threads(),
            skip: skip_prefixes(&root),
            older_than: None,
            extensions: Vec::new(),
            now: 0,
            age_ranges: Vec::new(),
        };
        progress.stage(&format!("drive {}", disk.letter));
        let out = walk(&spec, cancel, &|f, b| {
            progress.emit(files_seen + f, bytes_seen + b, false)
        });
        files_seen += out.files_seen;
        bytes_seen += out.bytes_seen;
        all.extend(out.files);
        if out.canceled {
            break;
        }
    }

    all.retain(|f| !is_protected(Path::new(&f.path)));
    all.sort_unstable_by(|a, b| b.size.cmp(&a.size));
    all.truncate(4000);
    progress.emit(files_seen, bytes_seen, true);
    Ok(all)
}

/// 删除选中的大文件。只接受本次扫描结果中出现过的路径。
pub fn delete(app: &AppHandle, paths: &[String], cancel: &AtomicBool) -> CleanResult {
    let state = app.state::<super::CleanupState>();
    let cached = match &*state.large.lock().unwrap() {
        Some(list) => list.clone(),
        None => Vec::new(),
    };
    let allowed: HashSet<&str> = cached.iter().map(|f| f.path.as_str()).collect();

    let mut res = CleanResult::default();
    for p in paths {
        if !allowed.contains(p.as_str()) {
            res.failed += 1;
            res.failed_items.push(format!("{} — 不在扫描结果中", p));
            continue;
        }
        let path = Path::new(p);
        if is_protected(path) {
            res.failed += 1;
            res.failed_items.push(format!("{} — 受保护", p));
            continue;
        }
        if cancel.load(Ordering::SeqCst) {
            res.note = "cancelled".to_string();
            break;
        }
        let md = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) => {
                res.fail(path, e);
                continue;
            }
        };
        use std::os::windows::fs::MetadataExt;
        if md.file_attributes() & 0x400 != 0 || !md.is_file() {
            res.failed += 1;
            res.failed_items.push(format!("{} — 不是普通文件", p));
            continue;
        }
        let size = md.len();
        match std::fs::remove_file(path) {
            Ok(_) => {
                res.freed += size;
                res.deleted += 1;
            }
            Err(e) => res.fail(path, e),
        }
    }
    res
}
