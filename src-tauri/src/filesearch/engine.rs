// 文件查找的纯逻辑：关键词模糊匹配、共享路径规范化、目录遍历。
//
// 刻意不依赖 tauri / serde，方便用 `rustc --test engine.rs` 直接跑单测
// （Tauri 的测试二进制在本机运行时受 WebView2Loader 影响，起不来）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.0} KB", (b / KB).max(1.0))
    }
}

// ---------------- 关键词匹配 ----------------

#[derive(Clone)]
pub struct SearchQuery {
    full: String,
    tokens: Vec<String>,
    compact: Vec<char>,
}

impl SearchQuery {
    pub fn new(raw: &str) -> Self {
        let full = raw.trim().to_lowercase();
        let tokens = full
            .split_whitespace()
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .collect();
        let compact = full.chars().filter(|c| !c.is_whitespace()).collect();
        SearchQuery {
            full,
            tokens,
            compact,
        }
    }

    pub fn score(&self, name: &str) -> Option<u8> {
        let lower = name.to_lowercase();
        if lower.contains(&self.full) {
            return Some(if lower.starts_with(&self.full) { 100 } else { 92 });
        }
        // 多个关键词（空格分隔）：全部出现即算命中，顺序不限
        if self.tokens.len() > 1 && self.tokens.iter().all(|t| lower.contains(t.as_str())) {
            return Some(78);
        }
        // 顺序模糊：关键词的字符依次出现在文件名里（"年度报告" → 2024年度总结报告）
        if self.compact.len() >= 2 && is_subsequence(&self.compact, &lower) {
            return Some(60);
        }
        None
    }
}

fn is_subsequence(needle: &[char], haystack: &str) -> bool {
    let mut chars = haystack.chars();
    for want in needle {
        loop {
            match chars.next() {
                Some(c) if c == *want => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// 规范化用户填的共享路径：去掉首尾空白与引号，盘符补成 `Z:\`，去掉多余结尾反斜杠，
/// 顺手把 `//服务器/共享` 这种正斜杠写法转成 Windows 的反斜杠。
pub fn normalize_network_root(raw: &str) -> String {
    let mut s = raw.trim().trim_matches('"').trim().to_string();
    if s.starts_with("//") {
        s = s.replace('/', "\\");
    }
    if s.len() == 2 && s.ends_with(':') {
        s.push('\\');
    }
    // UNC 根不能把 \\server 砍成 \\server；只去掉多余的结尾反斜杠
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    s
}

/// 判断路径是不是「只有服务器名」的 UNC 根（`\\server`）。
/// 这种路径本身不能直接遍历，要先枚举服务器上的共享。
pub fn is_server_root(path: &str) -> bool {
    match path.strip_prefix("\\\\") {
        Some(rest) => !rest.is_empty() && !rest.contains('\\'),
        None => false,
    }
}

// ---------------- 目录遍历 ----------------

/// 只看确定没有用户文件的目录：系统目录、回收站、包缓存、版本库。
const SKIP_DIRS: &[&str] = &[
    "windows",
    "winsxs",
    "system volume information",
    "$recycle.bin",
    "recovery",
    "perflogs",
    "node_modules",
    ".git",
];

pub fn should_skip_dir(name: &str) -> bool {
    let lower = name.to_lowercase();
    SKIP_DIRS.iter().any(|d| lower == *d)
}

#[cfg(target_os = "windows")]
fn is_reparse_point(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    // 只对目录调用，代价可控；不加这一步可能在 junction 上原地打转。
    entry
        .metadata()
        .map(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        .unwrap_or(false)
}

#[cfg(not(target_os = "windows"))]
fn is_reparse_point(_entry: &std::fs::DirEntry) -> bool {
    false
}

/// 命中的文件（尚未序列化，交给上层补 UI 需要的字段）
pub struct RawHit {
    pub path: String,
    pub name: String,
    pub dir: String,
    pub size: u64,
    pub size_label: String,
    pub modified: u64,
    pub score: u8,
}

/// 遍历过程中给上层的事件：命中文件，或某个目录读不了
pub enum WalkEvent {
    Hit(RawHit),
    Error(String),
}

/// 深度优先遍历若干根目录；回调返回 false 表示上层不需要更多结果，立即收手。
pub fn walk_roots_into<F>(
    roots: Vec<String>,
    query: SearchQuery,
    cancel: &AtomicBool,
    scanned: &AtomicU64,
    enough: &AtomicBool,
    mut on_event: F,
) where
    F: FnMut(WalkEvent) -> bool,
{
    for root in roots {
        // 网络共享盘（SMB）上几乎不会有 junction；少一次 metadata 往返，扫描快一倍
        let is_network = root.starts_with("\\\\");
        let mut stack: Vec<PathBuf> = vec![PathBuf::from(&root)];
        while let Some(dir) = stack.pop() {
            if cancel.load(Ordering::Relaxed) || enough.load(Ordering::Relaxed) {
                return;
            }
            let entries = match std::fs::read_dir(&dir) {
                Ok(list) => list,
                Err(e) => {
                    let event = WalkEvent::Error(format!("{}（{}）", dir.to_string_lossy(), e));
                    if !on_event(event) {
                        return;
                    }
                    continue;
                }
            };
            for entry in entries {
                if cancel.load(Ordering::Relaxed) || enough.load(Ordering::Relaxed) {
                    return;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                scanned.fetch_add(1, Ordering::Relaxed);
                let file_type = match entry.file_type() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                if file_type.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if should_skip_dir(&name) || (!is_network && is_reparse_point(&entry)) {
                        continue;
                    }
                    stack.push(entry.path());
                    continue;
                }
                // 符号链接指向的文件也跳过：只报告真实文件
                if !file_type.is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let score = match query.score(&name) {
                    Some(s) => s,
                    None => continue,
                };
                let path = entry.path();
                let meta = entry.metadata().ok();
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let modified = meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let dir_label = path
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                let hit = RawHit {
                    path: path.to_string_lossy().to_string(),
                    name,
                    dir: dir_label,
                    size,
                    size_label: human_size(size),
                    modified,
                    score,
                };
                if !on_event(WalkEvent::Hit(hit)) {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_match_scores() {
        // 前缀命中排最前
        assert_eq!(SearchQuery::new("报告").score("报告2024.docx"), Some(100));
        // 中间包含
        assert_eq!(SearchQuery::new("报告").score("年度报告.docx"), Some(92));
        // 多个关键词全部出现，顺序不限
        assert_eq!(
            SearchQuery::new("年度 报告").score("2024年度经营报告.docx"),
            Some(78)
        );
        // 顺序模糊：字都出现，但中间夹着别的字
        assert_eq!(
            SearchQuery::new("年度报告").score("2024年度总结报告.docx"),
            Some(60)
        );
        // 大小写不敏感
        assert_eq!(SearchQuery::new("REPORT").score("Q4_report.pdf"), Some(92));
        // 无关文件不命中
        assert_eq!(SearchQuery::new("报告").score("图片.png"), None);
        // 单字不做顺序模糊，避免一次命中整盘
        assert_eq!(SearchQuery::new("报").score("图片.png"), None);
    }

    #[test]
    fn network_root_normalization() {
        assert_eq!(normalize_network_root("Z:"), "Z:\\");
        assert_eq!(normalize_network_root("Z:\\"), "Z:\\");
        assert_eq!(
            normalize_network_root("\\\\server\\share\\"),
            "\\\\server\\share"
        );
        assert_eq!(
            normalize_network_root("  \\\\server\\share  "),
            "\\\\server\\share"
        );
        assert_eq!(
            normalize_network_root("\"\\\\server\\share\""),
            "\\\\server\\share"
        );
        // UNC 服务器根只去掉多余的反斜杠，不能砍成空
        assert_eq!(normalize_network_root("\\\\server"), "\\\\server");
        assert_eq!(normalize_network_root("\\\\server\\"), "\\\\server");
        // 正斜杠写法也认
        assert_eq!(
            normalize_network_root("//server/share/子目录"),
            "\\\\server\\share\\子目录"
        );
    }

    #[test]
    fn server_root_detection() {
        assert!(is_server_root("\\\\DS423"));
        assert!(is_server_root("\\\\192.168.1.10"));
        assert!(!is_server_root("\\\\DS423\\共享"));
        assert!(!is_server_root("Z:\\"));
        assert!(!is_server_root("DS423"));
    }

    #[test]
    fn skip_dirs_are_case_insensitive() {
        assert!(should_skip_dir("Windows"));
        assert!(should_skip_dir("$Recycle.Bin"));
        assert!(!should_skip_dir("报告"));
    }

    #[test]
    fn walk_finds_matches_and_skips_vcs_dirs() {
        let stamp = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("dpt-fs-test-{}-{}", std::process::id(), stamp));
        std::fs::create_dir_all(root.join("keep")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("keep").join("季度报告.txt"), b"x").unwrap();
        std::fs::write(root.join(".git").join("季度报告.txt"), b"x").unwrap();
        std::fs::write(root.join("无关.txt"), b"x").unwrap();

        let cancel = AtomicBool::new(false);
        let scanned = AtomicU64::new(0);
        let enough = AtomicBool::new(false);
        let mut found: Vec<RawHit> = Vec::new();
        walk_roots_into(
            vec![root.to_string_lossy().to_string()],
            SearchQuery::new("报告"),
            &cancel,
            &scanned,
            &enough,
            |event| {
                if let WalkEvent::Hit(hit) = event {
                    found.push(hit);
                }
                true
            },
        );

        assert_eq!(
            found.len(),
            1,
            "只应命中 keep 下的文件: {:?}",
            found.iter().map(|h| h.path.clone()).collect::<Vec<_>>()
        );
        assert!(found[0].path.contains("keep"));
        assert!(scanned.load(Ordering::Relaxed) >= 3);
        let _ = std::fs::remove_dir_all(&root);
    }
}
