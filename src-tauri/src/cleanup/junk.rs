//! 垃圾清理：Windows 临时文件 / 更新缓存 / 缩略图 / 着色器 / 转储 / 浏览器缓存 /
//! 回收站 / 旧驱动 / 卸载残留 / 注册表冗余。

use super::walk::{walk, WalkSpec};
use super::{expand, is_admin, purge, run_powershell, CleanResult, Progress};
use crate::cleanup::reg;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tauri::{AppHandle, Manager};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JunkEntry {
    pub path: String,
    pub label: String,
    pub size: u64,
    pub detail: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JunkCategory {
    pub id: String,
    pub bytes: u64,
    pub items: u64,
    pub needs_admin: bool,
    pub needs_confirm: bool,
    pub default_on: bool,
    pub entries: Vec<JunkEntry>,
    pub note: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JunkReport {
    pub categories: Vec<JunkCategory>,
    pub files_scanned: u64,
    pub elapsed_ms: u64,
    pub canceled: bool,
    pub admin: bool,
}

// ============== 规则 ==============

pub enum Target {
    /// 固定文件 / 目录
    Path(&'static str),
    /// 目录下的匹配文件
    Files(&'static str, &'static str),
    /// 目录 + 相对通配路径（每段可含 * ?）
    Dirs(&'static str, &'static str),
}

pub struct JunkRule {
    pub id: &'static str,
    pub needs_admin: bool,
    pub needs_confirm: bool,
    pub default_on: bool,
    pub targets: fn() -> Vec<Target>,
}

fn chrome_targets() -> Vec<Target> {
    let mut v = Vec::new();
    for base in [
        "%LOCALAPPDATA%\\Google\\Chrome\\User Data",
        "%LOCALAPPDATA%\\Microsoft\\Edge\\User Data",
    ] {
        v.push(Target::Dirs(base, "*/Cache"));
        v.push(Target::Dirs(base, "*/Code Cache"));
        v.push(Target::Dirs(base, "*/GPUCache"));
        v.push(Target::Dirs(base, "*/Service Worker/CacheStorage"));
        v.push(Target::Dirs(base, "*/GrShaderCache"));
    }
    v
}

pub fn rules() -> Vec<JunkRule> {
    vec![
        JunkRule {
            id: "temp_user",
            needs_admin: false,
            needs_confirm: false,
            default_on: true,
            targets: || vec![Target::Path("%TEMP%")],
        },
        JunkRule {
            id: "temp_windows",
            needs_admin: true,
            needs_confirm: false,
            default_on: true,
            targets: || vec![Target::Path("%WINDIR%\\Temp")],
        },
        JunkRule {
            id: "update_cache",
            needs_admin: true,
            needs_confirm: false,
            default_on: true,
            targets: || vec![Target::Path("%WINDIR%\\SoftwareDistribution\\Download")],
        },
        JunkRule {
            id: "delivery_opt",
            needs_admin: true,
            needs_confirm: false,
            default_on: true,
            targets: || {
                vec![Target::Path(
                    "%WINDIR%\\ServiceProfiles\\NetworkService\\AppData\\Local\\Microsoft\\Windows\\DeliveryOptimization\\Cache",
                )]
            },
        },
        JunkRule {
            id: "thumbnails",
            needs_admin: false,
            needs_confirm: false,
            default_on: true,
            targets: || {
                vec![
                    Target::Files("%LOCALAPPDATA%\\Microsoft\\Windows\\Explorer", "thumbcache_*.db"),
                    Target::Files("%LOCALAPPDATA%\\Microsoft\\Windows\\Explorer", "iconcache_*.db"),
                ]
            },
        },
        JunkRule {
            id: "shader_cache",
            needs_admin: false,
            needs_confirm: false,
            default_on: true,
            targets: || {
                vec![
                    Target::Path("%LOCALAPPDATA%\\D3DSCache"),
                    Target::Path("%LOCALAPPDATA%\\NVIDIA\\DXCache"),
                    Target::Path("%LOCALAPPDATA%\\AMD\\DxCache"),
                ]
            },
        },
        JunkRule {
            id: "crash_dumps",
            needs_admin: true,
            needs_confirm: false,
            default_on: true,
            targets: || {
                vec![
                    Target::Path("%LOCALAPPDATA%\\CrashDumps"),
                    Target::Path("%WINDIR%\\Minidump"),
                    Target::Path("%WINDIR%\\MEMORY.DMP"),
                ]
            },
        },
        JunkRule {
            id: "error_reports",
            needs_admin: true,
            needs_confirm: false,
            default_on: true,
            targets: || {
                vec![
                    Target::Path("%PROGRAMDATA%\\Microsoft\\Windows\\WER\\ReportArchive"),
                    Target::Path("%PROGRAMDATA%\\Microsoft\\Windows\\WER\\ReportQueue"),
                ]
            },
        },
        JunkRule {
            id: "cbs_logs",
            needs_admin: true,
            needs_confirm: false,
            default_on: true,
            targets: || vec![Target::Path("%WINDIR%\\Logs\\CBS")],
        },
        JunkRule {
            id: "inet_cache",
            needs_admin: false,
            needs_confirm: false,
            default_on: true,
            targets: || {
                vec![Target::Path(
                    "%LOCALAPPDATA%\\Microsoft\\Windows\\INetCache",
                )]
            },
        },
        JunkRule {
            id: "browser_cache",
            needs_admin: false,
            needs_confirm: false,
            default_on: false,
            targets: chrome_targets,
        },
        JunkRule {
            id: "recycle",
            needs_admin: false,
            needs_confirm: true,
            default_on: false,
            targets: Vec::new,
        },
        JunkRule {
            id: "drivers",
            needs_admin: true,
            needs_confirm: true,
            default_on: false,
            targets: Vec::new,
        },
        JunkRule {
            id: "leftovers",
            needs_admin: true,
            needs_confirm: true,
            default_on: false,
            targets: Vec::new,
        },
        JunkRule {
            id: "registry",
            needs_admin: true,
            needs_confirm: true,
            default_on: false,
            targets: Vec::new,
        },
    ]
}

// ============== 扫描 ==============

fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut match_i) = (usize::MAX, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            match_i = ni;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            match_i += 1;
            ni = match_i;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn resolve_dirs(base: &str, pattern: &str) -> Vec<PathBuf> {
    let mut current = vec![expand(base)];
    for seg in pattern.split(|c| c == '/' || c == '\\') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        let wildcard = seg.contains('*') || seg.contains('?');
        let mut next = Vec::new();
        for dir in current {
            if wildcard {
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for entry in rd.flatten() {
                        if !entry.path().is_dir() {
                            continue;
                        }
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if glob_match(seg, &name) {
                            next.push(entry.path());
                        }
                    }
                }
            } else {
                let p = dir.join(seg);
                if p.is_dir() {
                    next.push(p);
                }
            }
        }
        current = next;
        if current.is_empty() {
            break;
        }
    }
    current
}

fn resolve_targets(targets: &[Target]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for t in targets {
        match t {
            Target::Path(p) => {
                let path = expand(p);
                if path.exists() {
                    out.push(path);
                }
            }
            Target::Files(base, pattern) => {
                let dir = expand(base);
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for entry in rd.flatten() {
                        let is_file = entry.file_type().map(|f| f.is_file()).unwrap_or(false);
                        if !is_file {
                            continue;
                        }
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if glob_match(pattern, &name) {
                            out.push(entry.path());
                        }
                    }
                }
            }
            Target::Dirs(base, pattern) => {
                for dir in resolve_dirs(base, pattern) {
                    out.push(dir);
                }
            }
        }
    }
    out
}

/// 统计一个路径（文件或目录）的字节数与文件数
fn measure_path(path: &Path, cancel: &AtomicBool, progress: &Progress) -> (u64, u64) {
    if let Ok(md) = std::fs::symlink_metadata(path) {
        if md.is_file() {
            return (md.len(), 1);
        }
        if !md.is_dir() {
            return (0, 0);
        }
    } else {
        return (0, 0);
    }
    let spec = WalkSpec::new(path.to_path_buf());
    let out = walk(&spec, cancel, &|f, b| progress.emit(f, b, false));
    (out.bytes_seen, out.files_seen)
}

pub fn recycle_size(cancel: &AtomicBool) -> (u64, u64) {
    let mut bytes = 0;
    let mut files = 0;
    for disk in super::fixed_drives() {
        let bin = PathBuf::from(format!("{}\\$Recycle.Bin", disk.letter));
        if !bin.is_dir() {
            continue;
        }
        let spec = WalkSpec::new(bin);
        let out = walk(&spec, cancel, &|_, _| {});
        bytes += out.bytes_seen;
        files += out.files_seen;
    }
    (bytes, files)
}

pub fn scan(app: &AppHandle, cancel: &AtomicBool) -> JunkReport {
    let started = Instant::now();
    let progress = Progress::new(app.clone(), "junk");
    progress.stage("scan");
    let mut report = JunkReport {
        categories: Vec::new(),
        files_scanned: 0,
        elapsed_ms: 0,
        canceled: false,
        admin: is_admin(),
    };

    for rule in rules() {
        if cancel.load(Ordering::SeqCst) {
            report.canceled = true;
            break;
        }
        let mut bytes = 0u64;
        let mut items = 0u64;
        let mut entries: Vec<JunkEntry> = Vec::new();
        let mut note = String::new();

        match rule.id {
            "recycle" => {
                let (b, i) = recycle_size(cancel);
                bytes = b;
                items = i;
            }
            "drivers" => match reg::scan_old_drivers(cancel) {
                Ok((list, size)) => {
                    bytes = size;
                    items = list.len() as u64;
                    entries = list;
                }
                Err(e) => note = e,
            },
            "leftovers" => match reg::scan_leftovers(cancel, &progress) {
                Ok(list) => {
                    bytes = list.iter().map(|e| e.size).sum();
                    items = list.len() as u64;
                    entries = list;
                }
                Err(e) => note = e,
            },
            "registry" => match reg::scan_orphan_entries() {
                Ok(list) => {
                    items = list.len() as u64;
                    entries = list;
                }
                Err(e) => note = e,
            },
            _ => {
                for path in resolve_targets(&(rule.targets)()) {
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let (b, i) = measure_path(&path, cancel, &progress);
                    bytes += b;
                    items += i;
                    report.files_scanned += i;
                }
            }
        }

        report.categories.push(JunkCategory {
            id: rule.id.to_string(),
            bytes,
            items,
            needs_admin: rule.needs_admin,
            needs_confirm: rule.needs_confirm,
            default_on: rule.default_on,
            entries,
            note,
        });
        progress.emit(report.files_scanned, 0, true);
    }

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    report
}

// ============== 清理 ==============

fn category<'a>(report: &'a JunkReport, id: &str) -> Option<&'a JunkCategory> {
    report.categories.iter().find(|c| c.id == id)
}

fn purge_category(_id: &str, targets: &[Target], cancel: &AtomicBool) -> CleanResult {
    let mut res = CleanResult::default();
    for path in resolve_targets(targets) {
        if cancel.load(Ordering::Relaxed) {
            res.note = "cancelled".to_string();
            break;
        }
        // 单文件（缩略图等）直接删；目录保留，只清内容
        res.merge(purge(&path, cancel, false));
    }
    res
}

pub fn empty_recycle_bin() -> (bool, String) {
    let script = r#"$ErrorActionPreference='SilentlyContinue'
foreach ($d in [System.IO.DriveInfo]::GetDrives()) {
  if ($d.DriveType -eq 'Fixed') {
    Clear-RecycleBin -DriveLetter $d.Name.Substring(0,1) -Force -ErrorAction SilentlyContinue
  }
}"#;
    run_powershell(script)
}

fn move_to_recycle(paths: &[String]) -> (Vec<String>, Vec<String>) {
    if paths.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut script = String::from(
        "$ErrorActionPreference='SilentlyContinue'\n\
         Add-Type -AssemblyName Microsoft.VisualBasic\n\
         $p=@(",
    );
    for (i, p) in paths.iter().enumerate() {
        if i > 0 {
            script.push(',');
        }
        script.push('\'');
        script.push_str(&p.replace('\'', "''"));
        script.push('\'');
    }
    script.push_str(")\n");
    script.push_str(
        r#"foreach ($x in $p) {
  try {
    if (Test-Path -LiteralPath $x -PathType Container) {
      [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory($x,'OnlyErrorDialogs','SendToRecycleBin')
    } else {
      [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile($x,'OnlyErrorDialogs','SendToRecycleBin')
    }
    Write-Output ("OK " + $x)
  } catch {
    Write-Output ("ERR " + $x)
  }
}"#,
    );
    let (_, out) = run_powershell(&script);
    let mut ok = Vec::new();
    let mut err = Vec::new();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("OK ") {
            ok.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("ERR ") {
            err.push(rest.to_string());
        }
    }
    (ok, err)
}

pub fn clean(
    app: &AppHandle,
    report: &JunkReport,
    ids: &[String],
    confirm: bool,
) -> Result<CleanResult, String> {
    let state = app.state::<super::CleanupState>();
    let cancel = state.cancel.clone();
    let mut res = CleanResult::default();

    for id in ids {
        let cat = match category(report, id) {
            Some(c) => c,
            None => continue,
        };
        if cat.needs_confirm && !confirm {
            return Err("需要二次确认".to_string());
        }
        if cat.needs_admin && !is_admin() {
            return Err("需要管理员权限".to_string());
        }
        match id.as_str() {
            "recycle" => {
                let before = cat.bytes;
                let (ok, out) = empty_recycle_bin();
                if ok {
                    res.freed += before;
                    res.deleted += cat.items;
                    res.note = "recycle emptied".to_string();
                } else {
                    res.failed += 1;
                    res.failed_items.push(out);
                }
            }
            "drivers" => {
                res.merge(reg::clean_old_drivers(app, &cancel)?);
            }
            "leftovers" => {
                let mut paths = Vec::new();
                for e in &cat.entries {
                    paths.push(e.path.clone());
                }
                let (ok, err) = move_to_recycle(&paths);
                for p in &ok {
                    if let Some(e) = cat.entries.iter().find(|e| &e.path == p) {
                        res.freed += e.size;
                        res.deleted += 1;
                    }
                }
                res.failed += err.len() as u64;
                for p in err.into_iter().take(10) {
                    res.failed_items.push(p);
                }
                res.note = "moved to recycle bin".to_string();
            }
            "registry" => {
                res.merge(reg::clean_orphan_entries(app));
            }
            _ => {
                let targets = (rules()
                    .into_iter()
                    .find(|r| r.id == id)
                    .map(|r| (r.targets)()))
                .unwrap_or_default();
                // 目标在扫描后可能变化，重新解析一次即可；只清规则里写死的路径
                res.merge(purge_category(id, &targets, &cancel));
            }
        }
    }
    Ok(res)
}
