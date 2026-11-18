//! 注册表相关：已安装应用、卸载残留、注册表冗余、旧驱动包。
//!
//! 只用 windows crate 的 Registry API，不引入额外依赖；
//! 删除前一律先 `reg export` 备份到应用配置目录，失败则不动。

use super::junk::JunkEntry;
use super::walk::starts_with_ci;
use super::{folder_size, is_admin, run_hidden, CleanResult, Progress};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::Manager;
use windows::core::PCWSTR;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteTreeW, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY,
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    REG_EXPAND_SZ, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RegApp {
    pub key: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub location: String,
    pub icon: String,
    pub uninstall: String,
}

// ============== Registry 基础封装 ==============

fn open_key(root: HKEY, path: &str, extra: REG_SAM_FLAGS) -> Option<HKEY> {
    let w = super::wide(path);
    let mut hk = HKEY::default();
    let res = unsafe {
        RegOpenKeyExW(root, PCWSTR(w.as_ptr()), None, KEY_READ | extra, &mut hk)
    };
    if res.is_err() {
        None
    } else {
        Some(hk)
    }
}

fn subkeys(hk: HKEY) -> Vec<String> {
    let mut out = Vec::new();
    let mut idx = 0u32;
    loop {
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let res = unsafe {
            RegEnumKeyExW(
                hk,
                idx,
                Some(windows::core::PWSTR(buf.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if res.is_err() {
            break;
        }
        out.push(String::from_utf16_lossy(&buf[..len as usize]));
        idx += 1;
    }
    out
}

fn query_string(hk: HKEY, name: &str) -> Option<String> {
    let wn = super::wide(name);
    let mut buf = [0u16; 4096];
    let mut ty = REG_VALUE_TYPE::default();
    let mut cb = (buf.len() * 2) as u32;
    let res = unsafe {
        RegQueryValueExW(
            hk,
            PCWSTR(wn.as_ptr()),
            None,
            Some(&mut ty),
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut cb),
        )
    };
    if res.is_err() || (ty != REG_SZ && ty != REG_EXPAND_SZ) {
        return None;
    }
    let n = ((cb as usize) / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..n]);
    let s = s.trim_end_matches('\0').to_string();
    if s.is_empty() {
        None
    } else {
        Some(super::expand(&s).to_string_lossy().into_owned())
    }
}

/// 读取单个字符串值；供其它清理模块解析应用自定义数据目录。
pub(super) fn read_string_value(root: HKEY, path: &str, name: &str) -> Option<String> {
    let hk = open_key(root, path, REG_SAM_FLAGS(0))?;
    let value = query_string(hk, name);
    unsafe {
        let _ = RegCloseKey(hk);
    }
    value
}

fn query_u32(hk: HKEY, name: &str) -> Option<u32> {
    let wn = super::wide(name);
    let mut value: u32 = 0;
    let mut cb = 4u32;
    let res = unsafe {
        RegQueryValueExW(
            hk,
            PCWSTR(wn.as_ptr()),
            None,
            None,
            Some(&mut value as *mut u32 as *mut u8),
            Some(&mut cb),
        )
    };
    if res.is_err() {
        None
    } else {
        Some(value)
    }
}

fn delete_key(root: HKEY, subkey: &str) -> bool {
    let w = super::wide(subkey);
    unsafe { RegDeleteTreeW(root, PCWSTR(w.as_ptr())).is_ok() }
}

// ============== 已安装应用 ==============

fn uninstall_roots() -> Vec<(HKEY, &'static str, &'static str, REG_SAM_FLAGS)> {
    vec![
        (
            HKEY_LOCAL_MACHINE,
            "HKLM",
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            KEY_WOW64_64KEY,
        ),
        (
            HKEY_LOCAL_MACHINE,
            "HKLM",
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            KEY_WOW64_32KEY,
        ),
        (
            HKEY_CURRENT_USER,
            "HKCU",
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            REG_SAM_FLAGS(0),
        ),
    ]
}

pub fn installed_apps() -> Vec<RegApp> {
    let mut apps: Vec<RegApp> = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();

    for (hive, hive_name, path, flags) in uninstall_roots() {
        let root = match open_key(hive, path, flags) {
            Some(h) => h,
            None => continue,
        };
        for sub in subkeys(root) {
            // 跳过更新补丁与系统组件
            if sub.starts_with("KB") || sub.contains("Updates\\") {
                continue;
            }
            let child = match open_key(root, &sub, flags) {
                Some(h) => h,
                None => continue,
            };
            let name = query_string(child, "DisplayName");
            let system_component = query_u32(child, "SystemComponent").unwrap_or(0);
            let parent = query_string(child, "ParentKeyName");
            let release = query_string(child, "ReleaseType").unwrap_or_default();
            if name.is_none()
                || system_component == 1
                || parent.is_some()
                || release.eq_ignore_ascii_case("Update")
                || release.eq_ignore_ascii_case("Hotfix")
                || release.eq_ignore_ascii_case("Security Update")
            {
                unsafe { let _ = RegCloseKey(child); }
                continue;
            }
            let name = name.unwrap();
            let location = query_string(child, "InstallLocation").unwrap_or_default();
            let icon = query_string(child, "DisplayIcon").unwrap_or_default();
            let version = query_string(child, "DisplayVersion").unwrap_or_default();
            let publisher = query_string(child, "Publisher").unwrap_or_default();
            let uninstall = query_string(child, "UninstallString").unwrap_or_default();
            unsafe { let _ = RegCloseKey(child); }

            let key = format!("{}\\{}\\{}", hive_name, path, sub);
            let dedupe = (name.to_lowercase(), location.to_lowercase());
            if seen.contains(&dedupe) {
                continue;
            }
            seen.push(dedupe);
            apps.push(RegApp {
                key,
                name,
                version,
                publisher,
                location,
                icon,
                uninstall,
            });
        }
        unsafe { let _ = RegCloseKey(root); }
    }
    apps
}

/// 从 DisplayIcon 里取出可执行文件路径（去掉 ,0 之类的图标序号与引号）。
pub fn icon_file(icon: &str) -> Option<PathBuf> {
    let mut s = icon.trim().trim_matches('"');
    if s.is_empty() {
        return None;
    }
    if let Some(idx) = s.rfind(',') {
        // 只处理 “路径,0” 这种尾巴
        let tail = &s[idx + 1..];
        if tail.chars().all(|c| c.is_ascii_digit() || c == '-') {
            s = s[..idx].trim();
        }
    }
    let s = s.trim_matches('"');
    if s.len() < 3 || !s.as_bytes()[1].eq(&b':') {
        return None;
    }
    Some(PathBuf::from(s))
}

fn app_location(app: &RegApp) -> Option<PathBuf> {
    if !app.location.trim().is_empty() {
        let p = PathBuf::from(app.location.trim().trim_matches('"'));
        if p.exists() {
            return Some(p);
        }
    }
    if let Some(icon) = icon_file(&app.icon) {
        if let Some(parent) = icon.parent() {
            if parent.exists() {
                return Some(parent.to_path_buf());
            }
        }
    }
    None
}

/// 供 C 盘「应用迁移」用：安装目录在 C 盘的已安装应用
pub fn apps_on_c() -> Vec<(RegApp, PathBuf)> {
    let mut out = Vec::new();
    for app in installed_apps() {
        if let Some(loc) = app_location(&app) {
            let is_c = loc
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("C:\\");
            if is_c && !loc.to_string_lossy().eq_ignore_ascii_case("C:\\") {
                out.push((app, loc));
            }
        }
    }
    out
}

// ============== 卸载残留 ==============

fn system_dir_names() -> Vec<&'static str> {
    vec![
        "common files",
        "windowsapps",
        "microsoft",
        "windows defender",
        "windows nt",
        "internet explorer",
        "package cache",
        "modifiablewindowsapps",
        "uninstall information",
        "desktop.ini",
        "config.msi",
        "installer",
        "windows mail",
        "windows media player",
        "windows photo viewer",
        "windows sidebar",
        "winsxs",
        "system32",
        "temp",
    ]
}

/// 浅层检查：目录里是否有 exe、是否有卸载器
fn shallow_probe(dir: &Path) -> (bool, bool) {
    let mut has_exe = false;
    let mut has_uninstaller = false;
    let mut stack: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];
    let mut visited = 0usize;
    while let Some((cur, depth)) = stack.pop() {
        if visited > 400 {
            break;
        }
        visited += 1;
        let rd = match std::fs::read_dir(&cur) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.ends_with(".exe") {
                if name.starts_with("unins") || name.starts_with("uninstall") || name.starts_with("uninst") {
                    has_uninstaller = true;
                } else {
                    has_exe = true;
                }
            }
            if depth < 2 && entry.path().is_dir() {
                if let Ok(md) = entry.metadata() {
                    use std::os::windows::fs::MetadataExt;
                    if md.file_attributes() & 0x400 != 0 {
                        continue;
                    }
                }
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    (has_exe, has_uninstaller)
}

pub fn scan_leftovers(cancel: &AtomicBool, progress: &Progress) -> Result<Vec<JunkEntry>, String> {
    let apps = installed_apps();
    let mut prefixes: Vec<PathBuf> = Vec::new();
    for app in &apps {
        if let Some(loc) = app_location(&app) {
            prefixes.push(loc);
        }
        if let Some(icon) = icon_file(&app.icon) {
            if let Some(parent) = icon.parent() {
                prefixes.push(parent.to_path_buf());
            }
        }
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(v) = std::env::var(key) {
            roots.push(PathBuf::from(v));
        }
    }
    if let Ok(v) = std::env::var("ProgramData") {
        roots.push(PathBuf::from(v));
    }

    let mut out: Vec<JunkEntry> = Vec::new();
    let started = Instant::now();
    let mut scanned = 0u64;

    for root in roots {
        if cancel.load(Ordering::SeqCst) || started.elapsed() > Duration::from_secs(40) {
            break;
        }
        let rd = match std::fs::read_dir(&root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            if cancel.load(Ordering::SeqCst) || out.len() >= 40 {
                break;
            }
            if started.elapsed() > Duration::from_secs(40) {
                break;
            }
            let path = entry.path();
            let md = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            use std::os::windows::fs::MetadataExt;
            if md.file_attributes() & 0x400 != 0 || !md.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if system_dir_names().contains(&name.to_lowercase().as_str()) {
                continue;
            }
            // 被任何已安装应用的安装目录覆盖（或反过来覆盖了已安装目录的父目录）都跳过
            if prefixes.iter().any(|p| {
                starts_with_ci(&path, p) || starts_with_ci(p, &path)
            }) {
                continue;
            }
            let (has_exe, has_uninstaller) = shallow_probe(&path);
            if !has_exe || has_uninstaller {
                continue;
            }
            let size = folder_size(&path, cancel);
            if size < 8 * 1024 * 1024 {
                continue;
            }
            scanned += size;
            progress.emit(scanned, size, false);
            out.push(JunkEntry {
                path: path.to_string_lossy().into_owned(),
                label: name,
                size,
                detail: "no uninstall info".to_string(),
            });
            if out.len() >= 40 {
                break;
            }
        }
    }
    out.sort_unstable_by(|a, b| b.size.cmp(&a.size));
    Ok(out)
}

// ============== 注册表冗余 ==============

const UNINSTALL_MARKER: &str = "\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\";

fn within_uninstall(key: &str) -> bool {
    key.to_lowercase().contains(&UNINSTALL_MARKER.to_lowercase())
}

pub fn scan_orphan_entries() -> Result<Vec<JunkEntry>, String> {
    let mut out = Vec::new();
    for app in installed_apps() {
        // MSI 应用的图标/目录经常落在 Windows Installer 缓存里，容易误判，跳过
        if app.uninstall.to_lowercase().contains("msiexec") {
            continue;
        }
        let icon = icon_file(&app.icon);
        let loc = if app.location.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(app.location.trim().trim_matches('"')))
        };
        if icon.is_none() && loc.is_none() {
            continue;
        }
        let icon_exists = icon.as_ref().map(|p| p.exists()).unwrap_or(false);
        let loc_exists = loc.as_ref().map(|p| p.exists()).unwrap_or(false);
        if !icon_exists && !loc_exists {
            let missing = loc
                .as_ref()
                .or(icon.as_ref())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(JunkEntry {
                path: app.key.clone(),
                label: app.name.clone(),
                size: 0,
                detail: missing,
            });
        }
    }
    Ok(out)
}

fn backup_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let base = app.path().app_config_dir().ok()?;
    let dir = base.join("cleanup-backup");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn export_key(key: &str, file: &Path) -> bool {
    let (ok, _) = run_hidden(
        "reg",
        &[
            "export",
            key,
            &file.to_string_lossy(),
            "/y",
        ],
    );
    ok
}

pub fn clean_orphan_entries(app: &tauri::AppHandle) -> CleanResult {
    let mut res = CleanResult::default();
    let entries = match scan_orphan_entries() {
        Ok(e) => e,
        Err(e) => {
            res.note = e;
            return res;
        }
    };
    let dir = match backup_dir(app) {
        Some(d) => d,
        None => {
            res.note = "无法创建备份目录".to_string();
            return res;
        }
    };
    let stamp = format!(
        "reg-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let mut index = 0;

    for e in entries {
        if !within_uninstall(&e.path) {
            continue;
        }
        let (hive, sub) = if let Some(rest) = e.path.strip_prefix("HKLM\\") {
            (HKEY_LOCAL_MACHINE, rest.to_string())
        } else if let Some(rest) = e.path.strip_prefix("HKCU\\") {
            (HKEY_CURRENT_USER, rest.to_string())
        } else {
            continue;
        };
        let backup = dir.join(format!("{}-{}.reg", stamp, index));
        index += 1;
        if !export_key(&e.path, &backup) {
            res.failed += 1;
            res.failed_items.push(format!("{} — 备份失败，已跳过", e.label));
            continue;
        }
        if delete_key(hive, &sub) {
            res.deleted += 1;
        } else {
            res.failed += 1;
            res.failed_items.push(format!("{} — 删除失败", e.label));
        }
    }
    res.note = format!("backup: {}", dir.to_string_lossy());
    res
}

// ============== 旧驱动 ==============

#[derive(Default, Clone, Debug)]
struct DriverPkg {
    published: String,
    original: String,
    version: String,
}

fn find_version(line: &str) -> Option<String> {
    // 日期形如 09/12/2024，同一行逗号后面是版本号
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i + 9 < bytes.len() {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2] == '/'
            && bytes[i + 3].is_ascii_digit()
            && bytes[i + 4].is_ascii_digit()
            && bytes[i + 5] == '/'
            && bytes[i + 6].is_ascii_digit()
            && bytes[i + 7].is_ascii_digit()
            && bytes[i + 8].is_ascii_digit()
            && bytes[i + 9].is_ascii_digit()
        {
            let rest: String = bytes[i..].iter().collect();
            let after = rest.split(',').nth(1)?;
            let ver: String = after
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ' ')
                .collect();
            let ver = ver.trim().to_string();
            if !ver.is_empty() {
                return Some(ver);
            }
        }
        i += 1;
    }
    None
}

fn parse_packages(text: &str) -> Vec<DriverPkg> {
    let mut out = Vec::new();
    let mut cur = DriverPkg::default();
    let mut infs = 0;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            if !cur.published.is_empty() && !cur.original.is_empty() {
                out.push(cur.clone());
                cur = DriverPkg::default();
                infs = 0;
            }
            continue;
        }
        if l.to_ascii_lowercase().ends_with(".inf") {
            if infs == 0 {
                cur.published = l.to_ascii_lowercase();
            } else if infs == 1 {
                cur.original = l.to_ascii_lowercase();
            }
            infs += 1;
            continue;
        }
        if cur.version.is_empty() {
            if let Some(v) = find_version(l) {
                cur.version = v;
            }
        }
    }
    if !cur.published.is_empty() && !cur.original.is_empty() {
        out.push(cur);
    }
    out
}

fn pnputil_enum() -> Result<Vec<DriverPkg>, String> {
    let (ok, text) = run_hidden("pnputil", &["/enum-drivers"]);
    if !ok {
        return Err(if text.is_empty() {
            "pnputil 枚举失败（需要管理员权限）".to_string()
        } else {
            format!("pnputil 枚举失败：{}", first_line(&text))
        });
    }
    let pkgs = parse_packages(&text);
    if pkgs.is_empty() {
        return Err("没有读到第三方驱动包（可能系统里确实没有）".to_string());
    }
    Ok(pkgs)
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

fn used_inf_names() -> Vec<String> {
    let mut out = Vec::new();
    let root = match open_key(
        HKEY_LOCAL_MACHINE,
        "SYSTEM\\CurrentControlSet\\Control\\Class",
        KEY_WOW64_64KEY,
    ) {
        Some(h) => h,
        None => return out,
    };
    for guid in subkeys(root) {
        let class = match open_key(root, &guid, KEY_WOW64_64KEY) {
            Some(h) => h,
            None => continue,
        };
        for inst in subkeys(class) {
            let child = match open_key(class, &inst, KEY_WOW64_64KEY) {
                Some(h) => h,
                None => continue,
            };
            for name in ["InfPath", "DriverInfPath"] {
                if let Some(v) = query_string(child, name) {
                    out.push(v.to_ascii_lowercase());
                }
            }
            unsafe { let _ = RegCloseKey(child); }
        }
        unsafe { let _ = RegCloseKey(class); }
    }
    unsafe { let _ = RegCloseKey(root); }
    out
}

struct FolderInfo {
    base: String,
    version: String,
    path: PathBuf,
}

fn driver_folders() -> Vec<FolderInfo> {
    let root = super::expand("%WINDIR%\\System32\\DriverStore\\FileRepository");
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(&root) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let base = match name.split(".inf_").next() {
            Some(b) => b.to_string(),
            None => continue,
        };
        if base.is_empty() || !name.contains(".inf_") {
            continue;
        }
        let dir = entry.path();
        if let Ok(rd2) = std::fs::read_dir(&dir) {
            for f in rd2.flatten() {
                let fname = f.file_name().to_string_lossy().to_lowercase();
                if !fname.ends_with(".inf") {
                    continue;
                }
                if let Ok(bytes) = std::fs::read(f.path()) {
                    let text = String::from_utf8_lossy(&bytes);
                    let mut version = String::new();
                    for line in text.lines() {
                        if line.trim_start().to_ascii_lowercase().starts_with("driverver") {
                            if let Some(v) = find_version(line) {
                                version = v;
                            }
                            break;
                        }
                    }
                    out.push(FolderInfo {
                        base,
                        version,
                        path: dir.clone(),
                    });
                    break;
                }
            }
        }
    }
    out
}

pub fn scan_old_drivers(cancel: &AtomicBool) -> Result<(Vec<JunkEntry>, u64), String> {
    if !is_admin() {
        return Err("需要管理员权限才能枚举驱动包".to_string());
    }
    let pkgs = pnputil_enum()?;
    let used = used_inf_names();
    let folders = driver_folders();
    let mut out = Vec::new();
    let mut total = 0u64;
    let mut size_cache: Vec<(String, u64)> = Vec::new();

    for pkg in pkgs {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        if used.contains(&pkg.published) {
            continue;
        }
        let base = pkg.original.trim_end_matches(".inf").to_string();
        let folder = folders
            .iter()
            .find(|f| f.base == base && (pkg.version.is_empty() || f.version == pkg.version))
            .or_else(|| folders.iter().find(|f| f.base == base));
        let size = match folder {
            Some(f) => {
                let key = f.path.to_string_lossy().into_owned();
                if let Some(c) = size_cache.iter().find(|(k, _)| k == &key) {
                    c.1
                } else {
                    let s = folder_size(&f.path, cancel);
                    size_cache.push((key, s));
                    s
                }
            }
            None => 0,
        };
        total += size;
        out.push(JunkEntry {
            path: pkg.published.clone(),
            label: pkg.original.clone(),
            size,
            detail: if pkg.version.is_empty() {
                "unused".to_string()
            } else {
                format!("v{} · unused", pkg.version)
            },
        });
    }
    out.sort_unstable_by(|a, b| b.size.cmp(&a.size));
    Ok((out, total))
}

pub fn clean_old_drivers(app: &tauri::AppHandle, cancel: &AtomicBool) -> Result<CleanResult, String> {
    if !is_admin() {
        return Err("需要管理员权限".to_string());
    }
    let (list, _) = scan_old_drivers(cancel)?;
    let dir = backup_dir(app).ok_or("无法创建驱动备份目录")?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut res = CleanResult::default();
    let backup_root = dir.join(format!("drivers-{}", stamp));
    let _ = std::fs::create_dir_all(&backup_root);

    for e in list {
        if cancel.load(Ordering::SeqCst) {
            res.note = "cancelled".to_string();
            break;
        }
        let published = e.path.clone();
        let target = backup_root.join(published.replace('.', "_"));
        let _ = std::fs::create_dir_all(&target);
        // 先导出备份，失败也只是少一份保险，不阻断（pnputil 本身可回装）
        let _ = run_hidden(
            "pnputil",
            &[
                "/export-driver",
                &published,
                &target.to_string_lossy(),
            ],
        );
        let (ok, out) = run_hidden(
            "pnputil",
            &["/delete-driver", &published, "/uninstall"],
        );
        if ok {
            res.deleted += 1;
            res.freed += e.size;
        } else {
            res.failed += 1;
            if res.failed_items.len() < 10 {
                res.failed_items
                    .push(format!("{} — {}", e.label, first_line(&out)));
            }
        }
    }
    res.note = format!("backup: {}", backup_root.to_string_lossy());
    Ok(res)
}
