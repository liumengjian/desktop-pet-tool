//! C 盘专清：空间占用分析、系统级清理项、已安装应用迁移。

use super::walk::{walk, GroupItem, WalkSpec};
use super::{fixed_drives, folder_size, is_admin, run_hidden, CleanResult, Progress};
use crate::cleanup::reg;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeResult {
    pub path: String,
    pub parent: Option<String>,
    pub groups: Vec<GroupItem>,
    pub total: u64,
    pub scanned: u64,
    pub elapsed_ms: u64,
    pub canceled: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SystemItem {
    pub id: String,
    pub bytes: u64,
    pub available: bool,
    pub detail: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SystemReport {
    pub items: Vec<SystemItem>,
    pub reclaimable: u64,
    pub disks: Vec<super::DiskInfo>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppEntry {
    pub key: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub path: String,
    pub size: u64,
    pub uninstall: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UninstallResult {
    pub ok: bool,
    pub path: String,
    pub freed: u64,
    pub failed: u64,
    pub note: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppSize {
    pub path: String,
    pub size: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MigrateResult {
    pub ok: bool,
    pub path: String,
    pub target: String,
    pub bytes: u64,
    pub note: String,
}

fn emit_migrate_progress(app: &AppHandle, path: &str, stage: &str, copied: u64, total: u64) {
    let _ = app.emit_to(
        "cleanup",
        super::PROGRESS_EVENT,
        serde_json::json!({
            "task": "migrate",
            "path": path,
            "stage": stage,
            "bytes": copied,
            "total": total,
        }),
    );
}

fn emit_uninstall_progress(app: &AppHandle, path: &str, stage: &str, copied: u64, total: u64) {
    let _ = app.emit_to(
        "cleanup",
        super::PROGRESS_EVENT,
        serde_json::json!({
            "task": "uninstall",
            "path": path,
            "stage": stage,
            "bytes": copied,
            "total": total,
        }),
    );
}

fn emit_system_progress(app: &AppHandle, id: &str, stage: &str, copied: u64, total: u64) {
    let _ = app.emit_to(
        "cleanup",
        super::PROGRESS_EVENT,
        serde_json::json!({
            "task": "system-action",
            "id": id,
            "stage": stage,
            "bytes": copied,
            "total": total,
        }),
    );
}

/// 删除/清空类操作：轮询目标剩余大小，换算已处理字节。
fn monitor_folder_progress(
    app: AppHandle,
    id: String,
    path: PathBuf,
    total: u64,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            let current = folder_size(&path, &stop);
            let done = total.saturating_sub(current).min(total);
            emit_system_progress(&app, &id, "delete", done, total);
            thread::sleep(Duration::from_millis(500));
        }
    })
}

fn validate_dir(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err("目录不存在".to_string());
    }
    if !super::path_on_fixed_drive(path) {
        return Err("只支持固定磁盘上的目录".to_string());
    }
    Ok(())
}

/// 迁移只允许“应用自己的目录”，共享目录或系统目录一律不动。
fn migration_safe(dir: &Path) -> bool {
    let low = dir
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase();
    const BLOCK: &[&str] = &[
        "c:\\windows",
        "c:\\program files",
        "c:\\program files (x86)",
        "c:\\programdata",
        "c:\\users",
        "c:\\users\\public",
        "c:\\users\\default",
        "c:\\",
    ];
    if BLOCK.contains(&low.as_str()) {
        return false;
    }
    if low.starts_with("c:\\windows\\") {
        return false;
    }
    if low.ends_with("\\appdata")
        || low.ends_with("\\appdata\\local")
        || low.ends_with("\\appdata\\locallow")
        || low.ends_with("\\appdata\\roaming")
    {
        return false;
    }
    let comps: Vec<_> = dir.components().collect();
    // C:\Users\<name> 是用户目录本身，不是应用（Prefix + Root + Users + name）
    if comps.len() == 4 && comps[2].as_os_str().to_string_lossy().eq_ignore_ascii_case("Users") {
        return false;
    }
    comps.len() >= 2
}

pub fn analyze(app: &AppHandle, path: Option<String>, cancel: &AtomicBool) -> Result<AnalyzeResult, String> {
    let root = match path {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from("C:\\"),
    };
    validate_dir(&root)?;
    let progress = Progress::new(app.clone(), "cdrive");
    progress.stage("analyze");

    let spec = WalkSpec {
        root: root.clone(),
        min_size: 0,
        max_files: 0,
        group_depth: 1,
        threads: super::walk::default_threads(),
        skip: vec![
            root.join("$Recycle.Bin"),
            root.join("System Volume Information"),
        ],
        older_than: None,
        extensions: Vec::new(),
        now: 0,
        age_ranges: Vec::new(),
    };
    let out = walk(&spec, cancel, &|f, b| progress.emit(f, b, false));
    let parent = root.parent().map(|p| p.to_string_lossy().into_owned());
    Ok(AnalyzeResult {
        path: root.to_string_lossy().into_owned(),
        parent,
        groups: out.groups,
        total: out.bytes_seen,
        scanned: out.files_seen,
        elapsed_ms: out.elapsed_ms,
        canceled: out.canceled,
    })
}

fn file_size(path: &Path) -> u64 {
    std::fs::symlink_metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// 查询卷影副本（系统还原点）实际占用。查询失败时返回 (false, 0)。
fn shadow_storage_bytes() -> (bool, u64) {
    let script = "(Get-CimInstance -ClassName Win32_ShadowStorage -ErrorAction Stop | Measure-Object -Property UsedSpace -Sum).Sum";
    let (ok, out) = super::run_powershell(script);
    if !ok {
        return (false, 0);
    }
    let bytes = out
        .trim()
        .parse::<f64>()
        .ok()
        .map(|v| if v > 0.0 { v as u64 } else { 0 })
        .unwrap_or(0);
    (true, bytes)
}

/// 从 DISM 输出里取组件存储实际大小。
/// 输出顺序固定为“报告大小 / 实际大小 / …”，取第二个体积；
/// 数字与 GB/MB 单位都是 ASCII，不受控制台中文编码影响。
fn parse_component_store_size(text: &str) -> Option<u64> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut sizes = Vec::new();
    for i in 0..tokens.len().saturating_sub(1) {
        let unit = match tokens[i + 1] {
            "GB" => 1024u64.pow(3),
            "MB" => 1024u64.pow(2),
            _ => continue,
        };
        let raw = tokens[i].replace(',', "");
        if let Ok(v) = raw.parse::<f64>() {
            if v > 0.0 {
                sizes.push((v * unit as f64) as u64);
            }
        }
    }
    // 第一项是资源管理器报告的逻辑大小，第二项才是实际占用
    sizes.get(1).copied().or_else(|| sizes.first().copied())
}

pub fn component_store_bytes() -> Result<u64, String> {
    if !is_admin() {
        return Err("需要管理员权限".to_string());
    }
    let (ok, out) = run_hidden(
        "dism",
        &["/online", "/cleanup-image", "/analyzecomponentstore"],
    );
    if !ok {
        return Err(out.trim().to_string());
    }
    parse_component_store_size(&out).ok_or_else(|| "DISM 未返回组件存储大小".to_string())
}

pub fn system_report(app: &AppHandle, cancel: &AtomicBool) -> SystemReport {    let progress = Progress::new(app.clone(), "system");
    progress.stage("scan");
    let windir = super::expand("%WINDIR%");
    let mut items = Vec::new();

    let hiber = file_size(&windir.join("hiberfil.sys"));
    items.push(SystemItem {
        id: "hiberfil".to_string(),
        bytes: hiber,
        available: hiber > 0,
        detail: if is_admin() {
            String::new()
        } else {
            "admin".to_string()
        },
    });

    let page = file_size(&PathBuf::from("C:\\pagefile.sys")) + file_size(&PathBuf::from("C:\\swapfile.sys"));
    items.push(SystemItem {
        id: "pagefile".to_string(),
        bytes: page,
        available: false,
        detail: "system managed".to_string(),
    });

    let old = PathBuf::from("C:\\Windows.old");
    let old_size = if old.is_dir() {
        folder_size(&old, cancel)
    } else {
        0
    };
    items.push(SystemItem {
        id: "windows_old".to_string(),
        bytes: old_size,
        available: old_size > 0,
        detail: if is_admin() {
            String::new()
        } else {
            "admin".to_string()
        },
    });

    let (recycle_bytes, _) = crate::cleanup::junk::recycle_size(cancel);
    items.push(SystemItem {
        id: "recycle".to_string(),
        bytes: recycle_bytes,
        available: recycle_bytes > 0,
        detail: "confirm".to_string(),
    });

    // 系统还原点占用：Win32_ShadowStorage 的 UsedSpace 之和（进程需有权限）
    let (vss_ok, vss_bytes) = shadow_storage_bytes();
    items.push(SystemItem {
        id: "restore".to_string(),
        bytes: vss_bytes,
        available: vss_bytes > 0,
        detail: if vss_ok { String::new() } else { "vss".to_string() },
    });
    items.push(SystemItem {
        id: "winsxs".to_string(),
        bytes: 0,
        available: true,
        detail: if is_admin() {
            "dism".to_string()
        } else {
            "admin".to_string()
        },
    });

    let reclaimable = items
        .iter()
        .filter(|i| i.id != "pagefile" && i.id != "restore" && i.id != "winsxs")
        .map(|i| i.bytes)
        .sum();

    progress.emit(0, 0, true);
    SystemReport {
        items,
        reclaimable,
        disks: fixed_drives(),
    }
}

pub fn action(
    app: &AppHandle,
    id: &str,
    confirm: bool,
    cancel: &AtomicBool,
) -> Result<CleanResult, String> {
    if !confirm {
        return Err("需要二次确认".to_string());
    }
    let mut res = CleanResult::default();
    let windir = super::expand("%WINDIR%");
    emit_system_progress(app, id, "start", 0, 0);
    match id {
        "hiberfil" => {
            if !is_admin() {
                return Err("需要管理员权限".to_string());
            }
            let before = file_size(&windir.join("hiberfil.sys"));
            emit_system_progress(app, id, "run", 0, before);
            let (ok, out) = run_hidden("powercfg", &["-h", "off"]);
            if ok {
                res.freed = before;
                res.deleted = if before > 0 { 1 } else { 0 };
                res.note = "hibernation disabled".to_string();
            } else {
                res.failed = 1;
                res.failed_items.push(out);
            }
        }
        "windows_old" => {
            if !is_admin() {
                return Err("需要管理员权限".to_string());
            }
            let dir = PathBuf::from("C:\\Windows.old");
            if !dir.is_dir() {
                return Err("没有 Windows.old".to_string());
            }
            let before = folder_size(&dir, cancel);
            emit_system_progress(app, id, "delete", 0, before);
            let stop = Arc::new(AtomicBool::new(false));
            let monitor = monitor_folder_progress(
                app.clone(),
                id.to_string(),
                dir.clone(),
                before,
                stop.clone(),
            );
            let cmd = format!(
                "takeown /f \"{0}\" /r /d y >nul 2>&1 & icacls \"{0}\" /grant *S-1-5-32-544:F /t /q >nul 2>&1 & rd /s /q \"{0}\"",
                dir.to_string_lossy()
            );
            let (_, out) = run_hidden("cmd", &["/c", &cmd]);
            stop.store(true, Ordering::SeqCst);
            let _ = monitor.join();
            if dir.exists() {
                res.failed = 1;
                res.failed_items.push(if out.is_empty() {
                    "删除未完成（可能被占用）".to_string()
                } else {
                    out
                });
            } else {
                res.freed = before;
                res.deleted = 1;
            }
        }
        "recycle" => {
            let (before, _) = crate::cleanup::junk::recycle_size(cancel);
            emit_system_progress(app, id, "delete", 0, before);
            let stop = Arc::new(AtomicBool::new(false));
            let app_for_monitor = app.clone();
            let id_for_monitor = id.to_string();
            let stop_for_monitor = stop.clone();
            let monitor = thread::spawn(move || {
                while !stop_for_monitor.load(Ordering::SeqCst) {
                    let (remaining, _) = crate::cleanup::junk::recycle_size(&stop_for_monitor);
                    let done = before.saturating_sub(remaining).min(before);
                    emit_system_progress(&app_for_monitor, &id_for_monitor, "delete", done, before);
                    thread::sleep(Duration::from_millis(500));
                }
            });
            let (ok, out) = crate::cleanup::junk::empty_recycle_bin();
            stop.store(true, Ordering::SeqCst);
            let _ = monitor.join();
            if ok {
                res.freed = before;
                res.note = "recycle emptied".to_string();
            } else {
                res.failed = 1;
                res.failed_items.push(out);
            }
        }
        "restore" => {
            if !is_admin() {
                return Err("需要管理员权限".to_string());
            }
            emit_system_progress(app, id, "run", 0, 0);
            let (ok, out) = run_hidden(
                "vssadmin",
                &["delete", "shadows", "/for=C:", "/all", "/quiet"],
            );
            if ok {
                res.note = "restore points deleted".to_string();
            } else {
                res.failed = 1;
                res.failed_items.push(out);
            }
        }
        "winsxs" => {
            if !is_admin() {
                return Err("需要管理员权限".to_string());
            }
            emit_system_progress(app, id, "run", 0, 0);
            let (ok, out) = run_hidden(
                "dism",
                &[
                    "/online",
                    "/cleanup-image",
                    "/startcomponentcleanup",
                ],
            );
            if ok {
                res.note = "component cleanup done".to_string();
            } else {
                res.failed = 1;
                res.failed_items.push(out);
            }
        }
        _ => return Err("未知操作".to_string()),
    }
    emit_system_progress(
        app,
        id,
        if res.failed > 0 { "error" } else { "done" },
        res.freed,
        res.freed,
    );
    Ok(res)
}

// ============== 应用 ==============

pub fn apps_list(app: &AppHandle) -> Vec<AppEntry> {
    let mut out: Vec<AppEntry> = Vec::new();
    for (app_info, loc) in reg::apps_on_c() {
        if !loc.is_dir() || !migration_safe(&loc) {
            continue;
        }
        // 已经迁移过（原路径是目录联接）就不再列出，避免重复迁移
        if loc
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            continue;
        }
        if out
            .iter()
            .any(|a| a.path.eq_ignore_ascii_case(&loc.to_string_lossy()))
        {
            continue;
        }
        // 目录本身只做展示，大小等用户点“计算大小”再算，避免一开页就狂扫盘
        out.push(AppEntry {
            key: app_info.key,
            name: app_info.name,
            version: app_info.version,
            publisher: app_info.publisher,
            path: loc.to_string_lossy().into_owned(),
            size: 0,
            uninstall: app_info.uninstall,
        });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let state = app.state::<super::CleanupState>();
    *state.apps.lock().unwrap() = Some(out.clone());
    out
}

pub fn app_sizes(
    app: &AppHandle,
    paths: &[String],
    cancel: &AtomicBool,
) -> Vec<AppSize> {
    let state = app.state::<super::CleanupState>();
    let allowed: Vec<String> = state
        .apps
        .lock()
        .unwrap()
        .as_ref()
        .map(|list| list.iter().map(|a| a.path.clone()).collect())
        .unwrap_or_default();
    let progress = Progress::new(app.clone(), "apps");
    progress.stage("size");

    let mut out = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        if !allowed.contains(p) {
            continue;
        }
        let size = folder_size(Path::new(p), cancel);
        out.push(AppSize {
            path: p.clone(),
            size,
        });
        progress.emit((i + 1) as u64, size, true);
    }
    out
}

fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let t = cleaned.trim().trim_matches('.').to_string();
    if t.is_empty() {
        "app".to_string()
    } else {
        t
    }
}

pub fn migrate(
    app: &AppHandle,
    path: &str,
    target_drive: &str,
    confirm: bool,
    cancel: &AtomicBool,
) -> Result<MigrateResult, String> {
    if !confirm {
        return Err("需要二次确认".to_string());
    }
    if !is_admin() {
        return Err("需要管理员权限".to_string());
    }
    let state = app.state::<super::CleanupState>();
    let allowed = state
        .apps
        .lock()
        .unwrap()
        .as_ref()
        .map(|list| list.iter().any(|a| a.path == path))
        .unwrap_or(false);
    if !allowed {
        return Err("只支持列表中的应用目录".to_string());
    }

    let src = PathBuf::from(path);
    if !src.is_dir() {
        return Err("目录不存在".to_string());
    }
    if !migration_safe(&src) {
        return Err("该目录是系统或共享目录，不支持迁移".to_string());
    }
    if src
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("该目录已经是目录联接，已迁移过".to_string());
    }
    let letter = target_drive
        .trim_end_matches(':')
        .trim_end_matches('\\')
        .to_ascii_uppercase();
    if letter.len() != 1 || !letter.as_bytes()[0].is_ascii_alphabetic() {
        return Err("目标磁盘无效".to_string());
    }
    if src.to_string_lossy().to_ascii_uppercase().starts_with(&format!("{}:", letter)) {
        return Err("目标磁盘与源目录相同".to_string());
    }
    let disk = fixed_drives()
        .into_iter()
        .find(|d| d.letter.eq_ignore_ascii_case(&format!("{}:", letter)))
        .ok_or("目标磁盘不可用")?;

    let bytes = folder_size(&src, cancel);
    if disk.free < bytes + 512 * 1024 * 1024 {
        return Err("目标磁盘剩余空间不足".to_string());
    }

    // 用应用显示名做目标目录名：Edge / Copilot / Chrome 的安装目录都叫 Application，
    // 只按源目录名会在 AppMoved 下撞成同一个目标。
    let app_name = state
        .apps
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|list| list.iter().find(|a| a.path == path).map(|a| a.name.clone()))
        .filter(|n| !n.trim().is_empty());
    let name = app_name
        .as_deref()
        .map(sanitize)
        .unwrap_or_else(|| {
            src.file_name()
                .map(|n| sanitize(&n.to_string_lossy()))
                .unwrap_or_else(|| "app".to_string())
        });
    let dst_root = PathBuf::from(format!("{}:\\AppMoved", letter));
    let dst = dst_root.join(&name);
    if dst.exists() {
        return Err(format!("目标已存在：{}", dst.to_string_lossy()));
    }
    std::fs::create_dir_all(&dst_root).map_err(|e| e.to_string())?;

    let src_str = src.to_string_lossy().into_owned();
    let dst_str = dst.to_string_lossy().into_owned();
    emit_migrate_progress(app, &src_str, "start", 0, bytes);

    let stop = Arc::new(AtomicBool::new(false));
    let monitor = {
        let app_for_monitor = app.clone();
        let path_for_monitor = src_str.clone();
        let dst_for_monitor = dst.clone();
        let stop_for_monitor = stop.clone();
        thread::spawn(move || {
            while !stop_for_monitor.load(Ordering::SeqCst) {
                let copied = folder_size(&dst_for_monitor, &stop_for_monitor).min(bytes);
                emit_migrate_progress(
                    &app_for_monitor,
                    &path_for_monitor,
                    "copy",
                    copied,
                    bytes,
                );
                thread::sleep(Duration::from_millis(500));
            }
        })
    };

    let (code, out) = super::run_hidden_status(
        "robocopy",
        &[
            &src_str, &dst_str, "/E", "/MOVE", "/R:1", "/W:2", "/NFL", "/NDL", "/NJH", "/NJS", "/NP",
        ],
    );
    stop.store(true, Ordering::SeqCst);
    let _ = monitor.join();
    // robocopy 的 0-7 都算成功（1=复制了文件，2/4/6=有额外项）
    if code >= 8 {
        emit_migrate_progress(app, &src_str, "error", 0, bytes);
        // 失败时尽量把已搬走的内容搬回来
        let _ = super::run_hidden_status(
            "robocopy",
            &[
                &dst_str, &src_str, "/E", "/MOVE", "/R:1", "/W:2", "/NFL", "/NDL", "/NJH", "/NJS",
                "/NP",
            ],
        );
        return Ok(MigrateResult {
            ok: false,
            path: src_str,
            target: dst_str,
            bytes: 0,
            note: format!("移动失败（robocopy {code}）：{}", last_line(&out)),
        });
    }

    emit_migrate_progress(app, &src_str, "removing", bytes, bytes);
    if src.exists() {
        let _ = std::fs::remove_dir(&src);
    }
    if src.exists() {
        emit_migrate_progress(app, &src_str, "error", bytes, bytes);
        return Ok(MigrateResult {
            ok: false,
            path: src_str,
            target: dst_str,
            bytes: 0,
            note: "源目录仍有残留内容，已保留原目录".to_string(),
        });
    }

    emit_migrate_progress(app, &src_str, "linking", bytes, bytes);
    let (link_ok, link_out) = run_hidden("cmd", &["/c", "mklink", "/J", &src_str, &dst_str]);
    if !link_ok || !src.exists() {
        // 回滚：把目录搬回原位
        if src.exists() {
            let _ = std::fs::remove_dir(&src);
        }
        let _ = super::run_hidden_status("robocopy", &[&dst_str, &src_str, "/E", "/MOVE", "/R:1", "/W:2", "/NFL", "/NDL", "/NJH", "/NJS", "/NP"]);
        emit_migrate_progress(app, &src_str, "error", 0, bytes);
        return Ok(MigrateResult {
            ok: false,
            path: src_str,
            target: dst_str,
            bytes: 0,
            note: format!("创建目录联接失败，已回滚：{}", link_out),
        });
    }

    emit_migrate_progress(app, &src_str, "done", bytes, bytes);
    Ok(MigrateResult {
        ok: true,
        path: src_str,
        target: dst_str,
        bytes,
        note: "junction created".to_string(),
    })
}

fn normalize_uninstall_command(raw: &str) -> String {
    let mut cmd = raw.trim().to_string();
    let lower = cmd.to_ascii_lowercase();
    // MSI 的 UninstallString 常写成 MsiExec.exe /I{GUID}，/I 会进入维护界面；
    // 一键卸载改为 /X{GUID}，仍然由同一个 Windows Installer 完成卸载。
    if lower.contains("msiexec") {
        if let Some(pos) = lower.find(" /i") {
            let next_is_word = cmd
                .as_bytes()
                .get(pos + 3)
                .map(|b| b.is_ascii_alphabetic())
                .unwrap_or(false);
            if pos + 3 <= cmd.len() && !next_is_word {
                cmd.replace_range(pos + 1..pos + 3, "/X");
            }
        }
    }
    cmd
}

fn registry_key_exists(key: &str) -> bool {
    super::run_hidden_status("reg", &["query", key]).0 == 0
}

pub fn uninstall(
    app: &AppHandle,
    path: &str,
    confirm: bool,
    cancel: &AtomicBool,
) -> Result<UninstallResult, String> {
    if !confirm {
        return Err("需要二次确认".to_string());
    }
    let state = app.state::<super::CleanupState>();
    let entry = state
        .apps
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|list| list.iter().find(|a| a.path == path).cloned())
        .ok_or("只支持列表中的应用")?;
    if entry.uninstall.trim().is_empty() {
        return Err("该应用没有提供卸载程序".to_string());
    }

    let target = PathBuf::from(path);
    let before = folder_size(&target, cancel);
    // 已迁移应用的目录联接：记住真实目标，卸载器结束后要一并清理。
    let migrated_target = target
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
        .then(|| std::fs::canonicalize(&target).ok())
        .flatten()
        .filter(|p| {
            p.to_string_lossy()
                .to_ascii_lowercase()
                .contains(":\\appmoved\\")
        });
    emit_uninstall_progress(app, path, "uninstall", 0, before);

    let command = normalize_uninstall_command(&entry.uninstall);
    let (_, out) = super::run_hidden_status("cmd", &["/c", &command]);

    // 卸载器可能异步退出；以注册表卸载项消失作为卸载完成的依据。
    let mut gone = false;
    for _ in 0..20 {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        if !registry_key_exists(&entry.key) {
            gone = true;
            break;
        }
        thread::sleep(Duration::from_millis(1000));
    }

    if !gone {
        emit_uninstall_progress(app, path, "error", 0, before);
        let detail = last_line(&out);
        return Ok(UninstallResult {
            ok: false,
            path: path.to_string(),
            freed: 0,
            failed: 0,
            note: if detail.is_empty() {
                "卸载程序已结束，但系统仍检测到卸载项（可能被取消）".to_string()
            } else {
                format!("卸载程序已结束，但系统仍检测到卸载项：{detail}")
            },
        });
    }

    // 卸载成功后清理安装目录；用户数据目录（AppData、文档等）不动。
    let mut freed = 0;
    let mut failed = 0;
    let mut note = "uninstalled".to_string();
    let mut cleanup_migrated = true;
    if target.symlink_metadata().is_ok() {
        let is_link = target
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_link {
            // 已迁移应用的目录联接：先删联接，再清理目标盘中的真实目录。
            if let Err(e) = std::fs::remove_dir(&target) {
                failed = 1;
                cleanup_migrated = false;
                note = format!("卸载完成，但目录联接清理失败：{e}");
            }
        } else {
            emit_uninstall_progress(app, path, "cleanup", before, before);
            let res = crate::cleanup::purge(&target, cancel, true);
            freed = res.freed;
            failed = res.failed;
            if failed > 0 {
                note = format!("卸载完成，但安装目录残留清理失败 {failed} 项");
            } else {
                note = "uninstalled and leftovers cleaned".to_string();
            }
        }
    }
    if cleanup_migrated {
        if let Some(real_target) = migrated_target {
            if real_target.exists() {
                emit_uninstall_progress(app, path, "cleanup", before, before.max(freed));
                let res = crate::cleanup::purge(&real_target, cancel, true);
                freed += res.freed;
                failed += res.failed;
                if res.failed > 0 {
                    note = format!("卸载完成，但迁移目录残留清理失败 {} 项", res.failed);
                } else if freed > 0 {
                    note = "uninstalled and leftovers cleaned".to_string();
                }
            }
        }
    }

    emit_uninstall_progress(app, path, "done", freed, before.max(freed));
    Ok(UninstallResult {
        ok: true,
        path: path.to_string(),
        freed,
        failed,
        note,
    })
}
