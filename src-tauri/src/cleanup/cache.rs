//! 软件缓存清理：按应用分组，并在每个缓存项内按“7 天前 / 30 天前 / 90 天前 /
//! 半年前 / 更早”分桶勾选。
//!
//! 安全边界：
//! - 只清理规则里写死的缓存、日志、临时目录，不碰聊天数据库与会话目录；
//! - 聊天图片 / 视频 / 文件属于用户历史内容，单独列为高风险项，默认不勾选；
//! - 删除时重新解析规则路径，只删所选时间桶内的普通文件，跳过目录联接；
//! - 扫描 / 删除都可取消，线程数与其它清理任务一致（最多 2）。

use super::walk::{default_threads, walk, WalkSpec};
use super::{expand, CleanResult, Progress};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;

const SAMPLE_FILES: usize = 10;
const DAY_SECS: u64 = 86_400;

/// 时间桶：`min_age` 含、`max_age` 不含；None 表示无上限。
struct CacheBucketDef {
    id: &'static str,
    min_age: u64,
    max_age: Option<u64>,
}

const CACHE_BUCKETS: [CacheBucketDef; 5] = [
    CacheBucketDef { id: "d7", min_age: 7 * DAY_SECS, max_age: Some(30 * DAY_SECS) },
    CacheBucketDef { id: "d30", min_age: 30 * DAY_SECS, max_age: Some(90 * DAY_SECS) },
    CacheBucketDef { id: "d90", min_age: 90 * DAY_SECS, max_age: Some(180 * DAY_SECS) },
    CacheBucketDef { id: "d180", min_age: 180 * DAY_SECS, max_age: Some(365 * DAY_SECS) },
    CacheBucketDef { id: "old", min_age: 365 * DAY_SECS, max_age: None },
];

fn bucket_index(id: &str) -> Option<usize> {
    CACHE_BUCKETS.iter().position(|b| b.id == id)
}

fn in_bucket(index: usize, age: u64) -> bool {
    CACHE_BUCKETS
        .get(index)
        .map(|b| age >= b.min_age && b.max_age.map(|m| age < m).unwrap_or(true))
        .unwrap_or(false)
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntry {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub modified: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CacheBucket {
    pub id: String,
    pub bytes: u64,
    pub files: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CacheCategory {
    pub id: String,
    pub app: String,
    pub risky: bool,
    pub default_on: bool,
    pub detected: bool,
    pub bytes: u64,
    pub files: u64,
    pub entries: Vec<CacheEntry>,
    pub buckets: Vec<CacheBucket>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CacheReport {
    /// 扫描时的时间基准；清理沿用同一个基准，避免边界漂移
    pub scanned_at: u64,
    pub categories: Vec<CacheCategory>,
    pub files_scanned: u64,
    pub elapsed_ms: u64,
    pub canceled: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CacheSelection {
    pub id: String,
    pub buckets: Vec<String>,
}

pub struct CacheRule {
    pub id: &'static str,
    pub app: &'static str,
    pub risky: bool,
    pub default_on: bool,
    pub roots: &'static [&'static str],
}

/// Conda 的 pkgs 目录里，解压后的包文件可能被环境硬链接；
/// 只清压缩包缓存，避免删除后破坏现有环境。
fn rule_extensions(id: &str) -> &'static [&'static str] {
    match id {
        "conda_cache" => &[".conda", ".tar.bz2", ".tar.gz", ".zip"],
        _ => &[],
    }
}

/// 常用软件缓存目录。路径按“先确认目录存在、再扫描”的方式处理，
/// 未安装 / 路径不存在的软件不会出现在结果里。
pub fn rules() -> Vec<CacheRule> {
    vec![
        // 微信：FileStorage\Cache 是缓存；图片 / 视频 / 文件是聊天内容，单独高风险
        CacheRule {
            id: "wechat_cache",
            app: "wechat",
            risky: false,
            default_on: true,
            roots: &[
                "%USERPROFILE%\\Documents\\WeChat Files\\*\\FileStorage\\Cache",
                "%USERPROFILE%\\Documents\\xwechat_files\\*\\cache",
                "%USERPROFILE%\\Documents\\xwechat_files\\*\\temp",
                "%WECHAT_DATA%\\xwechat_files\\*\\cache",
                "%WECHAT_DATA%\\xwechat_files\\*\\temp",
                "%APPDATA%\\Tencent\\WeChat\\log",
                "%APPDATA%\\Tencent\\xwechat\\log",
            ],
        },
        CacheRule {
            id: "wechat_files",
            app: "wechat",
            risky: true,
            default_on: false,
            roots: &[
                "%USERPROFILE%\\Documents\\WeChat Files\\*\\FileStorage\\Image",
                "%USERPROFILE%\\Documents\\WeChat Files\\*\\FileStorage\\Video",
                "%USERPROFILE%\\Documents\\WeChat Files\\*\\FileStorage\\File",
                "%USERPROFILE%\\Documents\\xwechat_files\\*\\msg\\attach",
                "%USERPROFILE%\\Documents\\xwechat_files\\*\\msg\\file",
                "%USERPROFILE%\\Documents\\xwechat_files\\*\\msg\\video",
                "%WECHAT_DATA%\\xwechat_files\\*\\msg\\attach",
                "%WECHAT_DATA%\\xwechat_files\\*\\msg\\file",
                "%WECHAT_DATA%\\xwechat_files\\*\\msg\\video",
            ],
        },
        // QQ / QQNT
        CacheRule {
            id: "qq_cache",
            app: "qq",
            risky: false,
            default_on: true,
            roots: &[
                "%APPDATA%\\QQ\\Cache",
                "%APPDATA%\\QQ\\Code Cache",
                "%APPDATA%\\QQ\\GPUCache",
                "%APPDATA%\\QQ\\log",
                "%APPDATA%\\QQ\\Crashpad",
                "%APPDATA%\\QQ\\Partitions\\*\\Cache",
                "%APPDATA%\\QQ\\Partitions\\*\\Code Cache",
                "%APPDATA%\\QQ\\Partitions\\*\\GPUCache",
                "%APPDATA%\\Tencent\\QQ\\STemp",
                "%USERPROFILE%\\Documents\\Tencent Files\\*\\nt_qq\\nt_temp",
                "%USERPROFILE%\\Documents\\Tencent Files\\*\\nt_qq\\log",
                "%USERPROFILE%\\Documents\\Tencent Files\\*\\nt_qq\\log-cache",
                "%USERPROFILE%\\Documents\\Tencent Files\\nt_qq\\global\\nt_temp",
            ],
        },
        CacheRule {
            id: "qq_files",
            app: "qq",
            risky: true,
            default_on: false,
            roots: &[
                "%USERPROFILE%\\Documents\\Tencent Files\\*\\FileRecv",
                "%USERPROFILE%\\Documents\\Tencent Files\\*\\nt_qq\\nt_data\\File",
                "%USERPROFILE%\\Documents\\Tencent Files\\*\\nt_qq\\nt_data\\Pic",
                "%USERPROFILE%\\Documents\\Tencent Files\\*\\nt_qq\\nt_data\\Video",
            ],
        },
        // 企业微信：日志、CEF / 网页缓存、更新包可以清；聊天文件、图片、视频、语音
        // 是历史内容，单独列为高风险。数据目录可能是自定义盘符，用注册表解析。
        CacheRule {
            id: "wecom_cache",
            app: "wecom",
            risky: false,
            default_on: true,
            roots: &[
                "%APPDATA%\\Tencent\\WXWork\\Log",
                "%APPDATA%\\Tencent\\WXWork\\radium\\cache",
                "%APPDATA%\\Tencent\\WXWork\\radium\\web\\profiles\\*\\Cache",
                "%LOCALAPPDATA%\\wxworkweb\\Cache",
                "%WECOM_DATA%\\qtCef\\Cache",
                "%WECOM_DATA%\\qtCef\\Code Cache",
                "%WECOM_DATA%\\qtCef\\GPUCache",
                "%WECOM_DATA%\\qtCef\\DawnCache",
                "%WECOM_DATA%\\qtCef\\DawnWebGPUCache",
                "%WECOM_DATA%\\qtCef\\DawnGraphiteCache",
                "%WECOM_DATA%\\qtCef\\Service Worker\\CacheStorage",
                "%WECOM_DATA%\\qtCef\\Service Worker\\ScriptCache",
                "%WECOM_DATA%\\*\\WXWorkCefCache",
                "%WECOM_DATA%\\Default\\Cache",
                "%WECOM_DATA%\\Default\\Code Cache",
                "%WECOM_DATA%\\Default\\GPUCache",
                "%WECOM_DATA%\\GrShaderCache",
                "%WECOM_DATA%\\ShaderCache",
                "%WECOM_DATA%\\GraphiteDawnCache",
            ],
        },
        CacheRule {
            id: "wecom_files",
            app: "wecom",
            risky: true,
            default_on: false,
            roots: &[
                "%USERPROFILE%\\Documents\\WXWork\\*\\Cache\\File",
                "%WECOM_DATA%\\*\\Cache\\File",
                "%WECOM_DATA%\\*\\Cache\\Image",
                "%WECOM_DATA%\\*\\Cache\\Video",
                "%WECOM_DATA%\\*\\Cache\\Voice",
            ],
        },
        // 飞书 / 钉钉
        CacheRule {
            id: "feishu_cache",
            app: "feishu",
            risky: false,
            default_on: true,
            roots: &[
                "%LOCALAPPDATA%\\Feishu\\cache",
                "%LOCALAPPDATA%\\Feishu\\Cache",
                "%APPDATA%\\Feishu\\cache",
                "%APPDATA%\\Feishu\\Cache",
                "%LOCALAPPDATA%\\Lark\\cache",
                "%APPDATA%\\Lark\\cache",
            ],
        },
        CacheRule {
            id: "dingtalk_cache",
            app: "dingtalk",
            risky: false,
            default_on: true,
            roots: &[
                "%APPDATA%\\DingTalk\\cache",
                "%APPDATA%\\DingTalk\\Cache",
                "%APPDATA%\\DingTalk\\log",
                "%LOCALAPPDATA%\\DingTalk\\cache",
                "%LOCALAPPDATA%\\DingTalk\\Cache",
                "%LOCALAPPDATA%\\DingTalk\\log",
            ],
        },
        // WPS：缓存可以清；backup 里可能有自动备份，单独列为高风险
        CacheRule {
            id: "wps_cache",
            app: "wps",
            risky: false,
            default_on: true,
            roots: &[
                "%APPDATA%\\Kingsoft\\office6\\cache",
                "%LOCALAPPDATA%\\Kingsoft\\office6\\cache",
                "%LOCALAPPDATA%\\Kingsoft\\WPS Office\\*\\cache",
            ],
        },
        CacheRule {
            id: "wps_backup",
            app: "wps",
            risky: true,
            default_on: false,
            roots: &["%APPDATA%\\Kingsoft\\office6\\backup"],
        },
        // 剪映 / CapCut：只清缓存、Code Cache、GPU 缓存、日志和崩溃转储；
        // 草稿工程（Projects / Drafts）不在规则里，避免误删。
        CacheRule {
            id: "capcut_cache",
            app: "capcut",
            risky: false,
            default_on: true,
            roots: &[
                "%LOCALAPPDATA%\\CapCut\\User Data\\Cache",
                "%LOCALAPPDATA%\\CapCut\\User Data\\Code Cache",
                "%LOCALAPPDATA%\\CapCut\\User Data\\GPUCache",
                "%LOCALAPPDATA%\\CapCut\\User Data\\Temp",
                "%LOCALAPPDATA%\\CapCut\\User Data\\Log",
                "%LOCALAPPDATA%\\CapCut\\User Data\\Crashpad",
                "%LOCALAPPDATA%\\JianyingPro\\User Data\\Cache",
                "%LOCALAPPDATA%\\JianyingPro\\User Data\\Code Cache",
                "%LOCALAPPDATA%\\JianyingPro\\User Data\\GPUCache",
                "%LOCALAPPDATA%\\JianyingPro\\User Data\\Temp",
                "%LOCALAPPDATA%\\JianyingPro\\User Data\\Log",
                "%LOCALAPPDATA%\\JianyingPro\\User Data\\Crashpad",
            ],
        },
        // PR / PS 缓存与渲染临时文件
        CacheRule {
            id: "premiere_cache",
            app: "premiere",
            risky: false,
            default_on: true,
            roots: &[
                "%APPDATA%\\Adobe\\Common\\Media Cache",
                "%APPDATA%\\Adobe\\Common\\Media Cache Files",
                "%LOCALAPPDATA%\\Adobe\\Common\\Media Cache",
                "%LOCALAPPDATA%\\Adobe\\Common\\Media Cache Files",
            ],
        },
        CacheRule {
            id: "premiere_autosave",
            app: "premiere",
            risky: true,
            default_on: false,
            roots: &["%USERPROFILE%\\Documents\\Adobe\\Premiere Pro\\*\\Auto-Save"],
        },
        CacheRule {
            id: "photoshop_cache",
            app: "photoshop",
            risky: false,
            default_on: true,
            roots: &[
                "%APPDATA%\\Adobe\\CameraRaw\\Cache",
                "%LOCALAPPDATA%\\Adobe\\CameraRaw\\Cache",
                "%LOCALAPPDATA%\\Adobe\\Adobe Photoshop*\\Cache",
                "%APPDATA%\\Adobe\\Adobe Photoshop*\\Logs",
                "%LOCALAPPDATA%\\Adobe\\Adobe Photoshop*\\Logs",
            ],
        },
        CacheRule {
            id: "photoshop_autorecover",
            app: "photoshop",
            risky: true,
            default_on: false,
            roots: &["%APPDATA%\\Adobe\\Adobe Photoshop*\\AutoRecover"],
        },
        // 开发工具
        CacheRule {
            id: "npm_cache",
            app: "npm",
            risky: false,
            default_on: true,
            roots: &[
                "%LOCALAPPDATA%\\npm-cache",
                "%APPDATA%\\npm-cache",
                "%USERPROFILE%\\npm-cache",
                "%USERPROFILE%\\.npm\\_cacache",
            ],
        },
        CacheRule {
            id: "conda_cache",
            app: "conda",
            risky: false,
            default_on: true,
            roots: &[
                "%USERPROFILE%\\.conda\\pkgs",
                "%USERPROFILE%\\anaconda3\\pkgs",
                "%LOCALAPPDATA%\\anaconda3\\pkgs",
                "%USERPROFILE%\\miniconda3\\pkgs",
                "%LOCALAPPDATA%\\miniconda3\\pkgs",
                "%LOCALAPPDATA%\\conda\\conda\\pkgs",
            ],
        },
        CacheRule {
            id: "pip_cache",
            app: "pip",
            risky: false,
            default_on: true,
            roots: &["%LOCALAPPDATA%\\pip\\Cache", "%APPDATA%\\pip\\Cache"],
        },
        CacheRule {
            id: "jetbrains_cache",
            app: "jetbrains",
            risky: false,
            default_on: true,
            roots: &[
                "%LOCALAPPDATA%\\JetBrains\\*\\caches",
                "%LOCALAPPDATA%\\JetBrains\\*\\log",
                "%LOCALAPPDATA%\\JetBrains\\*\\tmp",
            ],
        },
        CacheRule {
            id: "vs_cache",
            app: "visualstudio",
            risky: false,
            default_on: true,
            roots: &[
                "%LOCALAPPDATA%\\Microsoft\\VisualStudio\\*\\ComponentModelCache",
                "%LOCALAPPDATA%\\Microsoft\\VisualStudio\\*\\logs",
                "%LOCALAPPDATA%\\Microsoft\\VSCommon\\*\\logs",
                "%LOCALAPPDATA%\\Temp\\VS*",
            ],
        },
        // AI / Agent 工具：只碰 cache / tmp / logs，不碰 sessions、attachments、projects
        CacheRule {
            id: "codex_cache",
            app: "codex",
            risky: false,
            default_on: true,
            roots: &[
                "%USERPROFILE%\\.codex\\cache",
                "%USERPROFILE%\\.codex\\.tmp",
                "%USERPROFILE%\\.codex\\tmp",
                "%USERPROFILE%\\.codex\\log",
            ],
        },
        CacheRule {
            id: "claude_cache",
            app: "claude",
            risky: false,
            default_on: true,
            roots: &[
                "%USERPROFILE%\\.claude\\cache",
                "%USERPROFILE%\\.claude\\logs",
                "%USERPROFILE%\\.claude\\shell-snapshots",
            ],
        },
        CacheRule {
            id: "doubao_cache",
            app: "doubao",
            risky: false,
            default_on: true,
            roots: &[
                "%LOCALAPPDATA%\\DoubaoWork\\User Data\\Default\\Cache",
                "%LOCALAPPDATA%\\DoubaoWork\\User Data\\Default\\Code Cache",
                "%LOCALAPPDATA%\\DoubaoWork\\User Data\\Default\\GPUCache",
                "%LOCALAPPDATA%\\DoubaoWork\\User Data\\Default\\DawnGraphiteCache",
                "%LOCALAPPDATA%\\DoubaoWork\\User Data\\Default\\DawnWebGPUCache",
                "%LOCALAPPDATA%\\DoubaoWork\\User Data\\Default\\GraphiteDawnCache",
                "%LOCALAPPDATA%\\DoubaoWork\\User Data\\Default\\GrShaderCache",
                "%LOCALAPPDATA%\\DoubaoWork\\User Data\\Default\\ShaderCache",
                "%LOCALAPPDATA%\\DoubaoWork\\User Data\\Crashpad",
                "%LOCALAPPDATA%\\DoubaoWork\\User Data\\component_crx_cache",
                "%LOCALAPPDATA%\\DoubaoWork\\User Data\\extensions_crx_cache",
                "%LOCALAPPDATA%\\DoubaoWork\\User Data\\client_local_file\\temp_file",
                "%APPDATA%\\Doubao\\cache",
                "%APPDATA%\\Doubao\\Cache",
            ],
        },
        CacheRule {
            id: "workbuddy_cache",
            app: "workbuddy",
            risky: false,
            default_on: true,
            roots: &[
                "%USERPROFILE%\\.workbuddy\\cache",
                "%USERPROFILE%\\.workbuddy\\logs",
                "%USERPROFILE%\\.workbuddy\\app\\cache",
                "%USERPROFILE%\\.workbuddy\\app\\CodeCache",
                "%USERPROFILE%\\.workbuddy\\app\\Crashpad",
                "%USERPROFILE%\\.workbuddy\\app\\session\\Cache",
                "%USERPROFILE%\\.workbuddy\\app\\session\\Code Cache",
                "%USERPROFILE%\\.workbuddy\\app\\session\\GPUCache",
            ],
        },
    ]
}

fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ni;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// 微信 4.x 允许把聊天数据放到任意盘；安装目录和微信配置里会保留这个根目录。
/// 配置目录下每个 `.ini` 文件内容是数据根（例如 `D:\data`），再拼 `xwechat_files`。
fn wechat_data_dir() -> Option<String> {
    let appdata = std::env::var("APPDATA").ok()?;
    let config = PathBuf::from(appdata)
        .join("Tencent")
        .join("xwechat")
        .join("config");
    let rd = std::fs::read_dir(config).ok()?;
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("ini") {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let text = text.trim().trim_start_matches('\u{feff}').trim();
        if text.is_empty() {
            continue;
        }
        let trimmed = text.trim_end_matches('\\').trim_end_matches('/');
        let base = PathBuf::from(trimmed);
        let root = if base
            .file_name()
            .map(|n| n.to_string_lossy().eq_ignore_ascii_case("xwechat_files"))
            .unwrap_or(false)
        {
            base.parent().map(Path::to_path_buf).unwrap_or(base)
        } else {
            base
        };
        if root.join("xwechat_files").is_dir() {
            return Some(root.to_string_lossy().into_owned());
        }
    }
    None
}

/// 企业微信允许把数据目录放到其它盘（设置里“文件存储位置”）。
/// 注册表 `HKCU\Software\Tencent\WXWork\DataLocationPath` 保存这个根目录，
/// 例如 `D:\APPDATA\WXWork`；没有自定义时退回默认的 `%APPDATA%\Tencent\WXWork`。
fn wecom_data_dir() -> Option<String> {
    if let Some(raw) = super::reg::read_string_value(
        HKEY_CURRENT_USER,
        r"Software\Tencent\WXWork",
        "DataLocationPath",
    ) {
        let trimmed = raw.trim_end_matches(|c| c == '\\' || c == '/');
        let path = PathBuf::from(trimmed);
        if path.is_dir() {
            return Some(path.to_string_lossy().into_owned());
        }
    }
    let appdata = std::env::var("APPDATA").ok()?;
    let path = PathBuf::from(appdata).join("Tencent").join("WXWork");
    if path.is_dir() {
        Some(path.to_string_lossy().into_owned())
    } else {
        None
    }
}

/// 解析带 `*` / `?` 的环境变量路径，只返回实际存在的目录。
fn resolve_roots(patterns: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for raw in patterns {
        let full = if raw.contains("%WECOM_DATA%") {
            match wecom_data_dir() {
                Some(base) => raw.replace("%WECOM_DATA%", &base),
                None => continue,
            }
        } else if raw.contains("%WECHAT_DATA%") {
            match wechat_data_dir() {
                Some(base) => raw.replace("%WECHAT_DATA%", &base),
                None => continue,
            }
        } else {
            (*raw).to_string()
        };
        let full = expand(&full).to_string_lossy().into_owned();
        let mut parts: Vec<String> = Vec::new();
        let mut prefix = String::new();
        let mut start = 0usize;
        if full.len() >= 2 && full.as_bytes()[1] == b':' {
            // 盘符后面保留分隔符，避免 "C:" + "Users" 变成相对路径
            prefix = format!("{}\\", &full[..2]);
            start = 2;
        }
        for (i, ch) in full.char_indices().skip(start) {
            if ch == '\\' || ch == '/' {
                if i > start {
                    parts.push(full[start..i].to_string());
                }
                start = i + 1;
            }
        }
        if start < full.len() {
            parts.push(full[start..].to_string());
        }
        if parts.is_empty() {
            continue;
        }
        let mut dirs = vec![PathBuf::from(prefix)];
        for seg in parts {
            if seg.is_empty() {
                continue;
            }
            if seg.contains('*') || seg.contains('?') {
                let mut next = Vec::new();
                for dir in dirs {
                    if let Ok(rd) = std::fs::read_dir(&dir) {
                        for entry in rd.flatten() {
                            if !entry.path().is_dir() {
                                continue;
                            }
                            let name = entry.file_name().to_string_lossy().into_owned();
                            if glob_match(&seg, &name) {
                                next.push(entry.path());
                            }
                        }
                    }
                }
                dirs = next;
            } else {
                dirs = dirs.into_iter().map(|d| d.join(&seg)).collect();
            }
            if dirs.is_empty() {
                break;
            }
        }
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            let key = dir.to_string_lossy().to_ascii_lowercase();
            if seen.insert(key) {
                out.push(dir);
            }
        }
    }
    out
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn modified_unix(md: &std::fs::Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn is_protected_name(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "db" | "sqlite" | "sqlite3" | "db-wal" | "db-shm" | "sqlite-wal" | "sqlite-shm"
            | "db-journal" | "sqlite-journal" | "mdb" | "accdb"
    )
}

pub fn scan(app: &AppHandle, cancel: &AtomicBool) -> CacheReport {
    let now = now_unix();
    let started = Instant::now();
    let progress = Progress::new(app.clone(), "cache");
    progress.stage("scan");

    let mut categories = Vec::new();
    let mut files_scanned = 0u64;
    let mut total_bytes = 0u64;
    let mut canceled = false;

    for rule in rules() {
        if cancel.load(Ordering::SeqCst) {
            canceled = true;
            break;
        }
        let roots = resolve_roots(rule.roots);
        if roots.is_empty() {
            continue;
        }
        let mut bucket_bytes = [0u64; CACHE_BUCKETS.len()];
        let mut bucket_files = [0u64; CACHE_BUCKETS.len()];
        let mut entries: Vec<CacheEntry> = Vec::new();
        let exts: Vec<String> = rule_extensions(rule.id)
            .iter()
            .map(|s| s.to_string())
            .collect();

        for root in roots {
            if cancel.load(Ordering::SeqCst) {
                canceled = true;
                break;
            }
            let spec = WalkSpec {
                root: root.clone(),
                min_size: 1,
                max_files: SAMPLE_FILES,
                group_depth: 0,
                threads: default_threads(),
                skip: Vec::new(),
                older_than: Some(now.saturating_sub(CACHE_BUCKETS[0].min_age)),
                extensions: exts.clone(),
                now,
                age_ranges: CACHE_BUCKETS.iter().map(|b| (b.min_age, b.max_age)).collect(),
            };
            let out = walk(&spec, cancel, &|f, b| {
                progress.emit(files_scanned + f, total_bytes + b, false)
            });
            for (i, (bytes, files)) in out.age_buckets.iter().enumerate() {
                if i < CACHE_BUCKETS.len() {
                    bucket_bytes[i] = bucket_bytes[i].saturating_add(*bytes);
                    bucket_files[i] = bucket_files[i].saturating_add(*files);
                }
            }
            files_scanned = files_scanned.saturating_add(out.files_seen);
            for f in out.files {
                entries.push(CacheEntry {
                    path: f.path,
                    name: f.name,
                    size: f.size,
                    modified: f.modified,
                });
            }
            if out.canceled {
                canceled = true;
            }
        }
        if canceled {
            break;
        }
        let buckets: Vec<CacheBucket> = CACHE_BUCKETS
            .iter()
            .enumerate()
            .map(|(i, b)| CacheBucket {
                id: b.id.to_string(),
                bytes: bucket_bytes[i],
                files: bucket_files[i],
            })
            .collect();
        let bytes = buckets.iter().map(|b| b.bytes).sum();
        let files = buckets.iter().map(|b| b.files).sum();
        total_bytes = total_bytes.saturating_add(bytes);
        entries.sort_unstable_by(|a, b| b.size.cmp(&a.size));
        entries.truncate(SAMPLE_FILES);
        categories.push(CacheCategory {
            id: rule.id.to_string(),
            app: rule.app.to_string(),
            risky: rule.risky,
            default_on: rule.default_on,
            detected: true,
            bytes,
            files,
            entries,
            buckets,
        });
    }

    progress.emit(files_scanned, total_bytes, true);
    CacheReport {
        scanned_at: now,
        categories,
        files_scanned,
        elapsed_ms: started.elapsed().as_millis() as u64,
        canceled,
    }
}

/// 删除单个目录下落在所选时间桶内的普通文件；目录本身和联接点保留。
fn purge_age(
    root: &Path,
    now: u64,
    selected: &[usize],
    exts: &[String],
    cancel: &AtomicBool,
) -> CleanResult {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    let mut res = CleanResult::default();
    if selected.is_empty() {
        return res;
    }
    if let Ok(md) = std::fs::symlink_metadata(root) {
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !md.is_dir() {
            res.note = "skip reparse root".to_string();
            return res;
        }
    } else {
        return res;
    }
    let mut stack = vec![root.to_path_buf()];
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
            let path = entry.path();
            let md = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    res.fail(&path, e);
                    continue;
                }
            };
            if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                continue;
            }
            if md.is_dir() {
                stack.push(path);
                continue;
            }
            if !md.is_file() || is_protected_name(&path) {
                continue;
            }
            if !exts.is_empty() {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if !exts.iter().any(|e| name.ends_with(e)) {
                    continue;
                }
            }
            let modified = modified_unix(&md);
            if modified == 0 || modified > now {
                continue;
            }
            let age = now.saturating_sub(modified);
            if !selected.iter().any(|i| in_bucket(*i, age)) {
                continue;
            }
            let size = md.len();
            match std::fs::remove_file(&path) {
                Ok(_) => {
                    res.freed += size;
                    res.deleted += 1;
                }
                Err(e) => res.fail(&path, e),
            }
        }
    }
    res
}

pub fn clean(
    report: &CacheReport,
    selections: &[CacheSelection],
    confirm: bool,
    cancel: &AtomicBool,
) -> Result<CleanResult, String> {
    if !confirm {
        return Err("需要二次确认".to_string());
    }
    if selections.is_empty() {
        return Err("没有选择要清理的缓存".to_string());
    }
    let mut targets: std::collections::HashMap<String, (PathBuf, Vec<String>, Vec<usize>)> =
        std::collections::HashMap::new();

    for sel in selections {
        let cat = report
            .categories
            .iter()
            .find(|c| c.id == sel.id)
            .ok_or_else(|| "清理项已变化，请重新扫描".to_string())?;
        let mut indexes = Vec::new();
        let mut seen = HashSet::new();
        for id in &sel.buckets {
            let i = bucket_index(id).ok_or_else(|| "清理时间范围无效，请重新扫描".to_string())?;
            if !cat.buckets.iter().any(|b| b.id == *id) {
                return Err("清理项已变化，请重新扫描".to_string());
            }
            if seen.insert(i) {
                indexes.push(i);
            }
        }
        if indexes.is_empty() {
            continue;
        }
        let rule = rules()
            .into_iter()
            .find(|r| r.id == sel.id)
            .ok_or_else(|| "清理项已变化，请重新扫描".to_string())?;
        let exts: Vec<String> = rule_extensions(rule.id)
            .iter()
            .map(|s| s.to_string())
            .collect();
        for root in resolve_roots(rule.roots) {
            let key = root.to_string_lossy().to_ascii_lowercase();
            targets
                .entry(key)
                .or_insert_with(|| (root, exts.clone(), Vec::new()))
                .2
                .extend(indexes.iter().copied());
        }
    }

    let mut res = CleanResult::default();
    for (_, (root, exts, mut indexes)) in targets {
        indexes.sort_unstable();
        indexes.dedup();
        res.merge(purge_age(&root, report.scanned_at, &indexes, &exts, cancel));
        if cancel.load(Ordering::Relaxed) {
            break;
        }
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::{bucket_index, in_bucket, glob_match, DAY_SECS};

    #[test]
    fn glob_matches_cache_segments() {
        assert!(glob_match("Cache", "cache"));
        assert!(glob_match("Cache*", "Cache_Data"));
        assert!(glob_match("Adobe Photoshop*", "Adobe Photoshop 2024"));
        assert!(!glob_match("Cache", "Code Cache"));
        assert!(glob_match("VS*", "VS2022"));
    }

    #[test]
    fn age_bucket_boundaries_are_exclusive_at_upper_bound() {
        let d7 = bucket_index("d7").unwrap();
        let d30 = bucket_index("d30").unwrap();
        let old = bucket_index("old").unwrap();
        assert!(in_bucket(d7, 7 * DAY_SECS));
        assert!(!in_bucket(d7, 30 * DAY_SECS));
        assert!(in_bucket(d30, 30 * DAY_SECS));
        assert!(!in_bucket(d30, 90 * DAY_SECS));
        assert!(in_bucket(old, 365 * DAY_SECS));
        assert!(in_bucket(old, 10 * 365 * DAY_SECS));
        assert!(!in_bucket(old, 365 * DAY_SECS - 1));
    }
}
