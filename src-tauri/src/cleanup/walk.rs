//! 有界并发的目录遍历：统计大小、收集大文件、按目录聚合。
//!
//! 设计约束（和本项目其它模块一致）：
//! - 不引入新依赖，只用 std；
//! - 线程数固定上限（默认 2），扫描期间 CPU 占用可控，不抢占用户前台工作；
//! - 不跟随目录联接 / 符号链接，避免死循环和越界统计；
//! - 可随时通过 `cancel` 退出，已收集的结果仍可返回。

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileItem {
    pub path: String,
    pub name: String,
    pub dir: String,
    pub size: u64,
    /// 修改时间（Unix 秒）
    pub modified: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GroupItem {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub files: u64,
    /// 根目录直属文件（pagefile.sys、hiberfil.sys 等）不是可下钻目录，
    /// 聚合为一行只读统计，前端不提供点击进入。
    pub root_files: bool,
}

#[derive(Clone)]
pub struct WalkSpec {
    pub root: PathBuf,
    /// >0 时收集不小于该体积的文件；0 表示不收集
    pub min_size: u64,
    pub max_files: usize,
    /// >0 时按目录聚合；1 = 根目录下的第一层
    pub group_depth: usize,
    pub threads: usize,
    /// 跳过的目录前缀（大小写不敏感）
    pub skip: Vec<PathBuf>,
    /// 只统计修改时间不晚于该 Unix 秒的文件；None 表示不限
    pub older_than: Option<u64>,
    /// 只统计这些后缀的文件（小写，含点）；空表示不限
    pub extensions: Vec<String>,
    /// 计算年龄时使用的“现在” Unix 秒；age_ranges 非空时使用
    pub now: u64,
    /// 按文件年龄统计区间：`(最小秒, 最大秒)`，最小含、最大不含；None 表示无上限
    pub age_ranges: Vec<(u64, Option<u64>)>,
}

impl WalkSpec {
    pub fn new(root: PathBuf) -> Self {
        WalkSpec {
            root,
            min_size: 0,
            max_files: 0,
            group_depth: 0,
            threads: default_threads(),
            skip: Vec::new(),
            older_than: None,
            extensions: Vec::new(),
            now: 0,
            age_ranges: Vec::new(),
        }
    }
}

pub struct WalkOut {
    pub files: Vec<FileItem>,
    pub groups: Vec<GroupItem>,
    pub files_seen: u64,
    pub bytes_seen: u64,
    #[allow(dead_code)]
    pub errors: u64,
    pub canceled: bool,
    pub elapsed_ms: u64,
    /// 按 `WalkSpec.age_ranges` 顺序统计的 `(字节, 文件数)`
    pub age_buckets: Vec<(u64, u64)>,
}

/// 扫描线程数：2 是默认值，机器核数少时退到 1。
pub fn default_threads() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    if cores <= 2 {
        1
    } else {
        2
    }
}

pub fn starts_with_ci(path: &Path, prefix: &Path) -> bool {
    let p = path.to_string_lossy().to_lowercase();
    let q = prefix.to_string_lossy().to_lowercase();
    if !p.starts_with(&q) {
        return false;
    }
    if p.len() == q.len() {
        return true;
    }
    let boundary = q.ends_with('\\') || q.ends_with('/');
    boundary || p.as_bytes()[q.len()] == b'\\' || p.as_bytes()[q.len()] == b'/'
}

fn group_path(root: &Path, path: &Path, depth: usize) -> PathBuf {
    let rel = match path.strip_prefix(root) {
        Ok(r) => r,
        Err(_) => return root.to_path_buf(),
    };
    let comps: Vec<Component> = rel.components().collect();
    let mut d = depth;
    // C:\Users\<用户名>\... 比别的顶层目录深一层才有意义
    if root.parent().is_none() && comps.len() > 1 {
        if let Component::Normal(first) = &comps[0] {
            if first.to_string_lossy().eq_ignore_ascii_case("users") {
                d = depth + 1;
            }
        }
    }
    if comps.len() <= d {
        return path.to_path_buf();
    }
    let mut key = root.to_path_buf();
    for c in comps.iter().take(d) {
        key.push(c.as_os_str());
    }
    key
}

fn display_name(path: &Path) -> String {
    match path.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => path.to_string_lossy().into_owned(),
    }
}

/// 遍历 `spec.root`。`on_progress(files, bytes)` 至少每 200ms 触发一次。
pub fn walk(
    spec: &WalkSpec,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(u64, u64) + Sync),
) -> WalkOut {
    let started = Instant::now();
    let stack: Mutex<Vec<PathBuf>> = Mutex::new(vec![spec.root.clone()]);
    let active = AtomicUsize::new(0);
    let cv = Condvar::new();
    let files_seen = AtomicU64::new(0);
    let bytes_seen = AtomicU64::new(0);
    let errors = AtomicU64::new(0);
    let files: Mutex<Vec<FileItem>> = Mutex::new(Vec::new());
    let groups: Mutex<HashMap<PathBuf, (u64, u64)>> = Mutex::new(HashMap::new());
    let age_stats: Mutex<Vec<(u64, u64)>> =
        Mutex::new(vec![(0, 0); spec.age_ranges.len()]);

    let threads = spec.threads.clamp(1, 8);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut last_report = Instant::now();
                let mut local_files: u64 = 0;
                loop {
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let dir = {
                        let mut guard = stack.lock().unwrap();
                        loop {
                            if cancel.load(Ordering::SeqCst) {
                                return;
                            }
                            if let Some(p) = guard.pop() {
                                // 持锁自增，避免“队列空 + 无活跃线程”被误判为结束
                                active.fetch_add(1, Ordering::SeqCst);
                                break p;
                            }
                            if active.load(Ordering::SeqCst) == 0 {
                                return;
                            }
                            let (g, _t) = cv
                                .wait_timeout(guard, Duration::from_millis(40))
                                .unwrap();
                            guard = g;
                        }
                    };

                    let entries = std::fs::read_dir(&dir);
                    if let Ok(entries) = entries {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            let md = match entry.metadata() {
                                Ok(m) => m,
                                Err(_) => {
                                    errors.fetch_add(1, Ordering::Relaxed);
                                    continue;
                                }
                            };
                            use std::os::windows::fs::MetadataExt;
                            if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                                continue;
                            }
                            if md.is_dir() {
                                if spec.skip.iter().any(|s| starts_with_ci(&path, s)) {
                                    continue;
                                }
                                stack.lock().unwrap().push(path);
                                cv.notify_all();
                            } else if md.is_file() {
                                if !spec.extensions.is_empty() {
                                    let name = path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_ascii_lowercase())
                                        .unwrap_or_default();
                                    if !spec.extensions.iter().any(|e| name.ends_with(e)) {
                                        continue;
                                    }
                                }
                                let modified = md
                                    .modified()
                                    .ok()
                                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0);
                                // 按天清理时只统计足够旧的文件；时间未知的一律跳过，避免误删
                                if let Some(cutoff) = spec.older_than {
                                    if modified == 0 || modified > cutoff {
                                        continue;
                                    }
                                }
                                let len = md.len();
                                files_seen.fetch_add(1, Ordering::Relaxed);
                                bytes_seen.fetch_add(len, Ordering::Relaxed);
                                local_files += 1;

                                if !spec.age_ranges.is_empty() && modified != 0 {
                                    let age = spec.now.saturating_sub(modified);
                                    if let Some(i) = spec.age_ranges.iter().position(|(min, max)| {
                                        age >= *min && max.map(|m| age < m).unwrap_or(true)
                                    }) {
                                        let mut stats = age_stats.lock().unwrap();
                                        stats[i].0 += len;
                                        stats[i].1 += 1;
                                    }
                                }

                                if spec.group_depth > 0 {
                                    if let Some(parent) = path.parent() {
                                        let key = group_path(&spec.root, parent, spec.group_depth);
                                        let mut map = groups.lock().unwrap();
                                        let slot = map.entry(key).or_insert((0, 0));
                                        slot.0 += len;
                                        slot.1 += 1;
                                    }
                                }

                                if spec.min_size > 0 && len >= spec.min_size {
                                    let item = FileItem {
                                        path: path.to_string_lossy().into_owned(),
                                        name: display_name(&path),
                                        dir: path
                                            .parent()
                                            .map(|p| p.to_string_lossy().into_owned())
                                            .unwrap_or_default(),
                                        size: len,
                                        modified,
                                    };
                                    let mut list = files.lock().unwrap();
                                    list.push(item);
                                    if spec.max_files > 0 && list.len() > spec.max_files * 3 {
                                        list.sort_unstable_by(|a, b| b.size.cmp(&a.size));
                                        list.truncate(spec.max_files);
                                    }
                                }
                            }
                        }
                    } else {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }

                    {
                        let guard = stack.lock().unwrap();
                        active.fetch_sub(1, Ordering::SeqCst);
                        if active.load(Ordering::SeqCst) == 0 {
                            cv.notify_all();
                        }
                        drop(guard);
                    }

                    if local_files >= 1024 || last_report.elapsed() >= Duration::from_millis(200) {
                        local_files = 0;
                        last_report = Instant::now();
                        on_progress(
                            files_seen.load(Ordering::Relaxed),
                            bytes_seen.load(Ordering::Relaxed),
                        );
                    }
                }
                on_progress(
                    files_seen.load(Ordering::Relaxed),
                    bytes_seen.load(Ordering::Relaxed),
                );
            });
        }
    });

    on_progress(
        files_seen.load(Ordering::Relaxed),
        bytes_seen.load(Ordering::Relaxed),
    );

    let mut file_list = files.into_inner().unwrap();
    file_list.sort_unstable_by(|a, b| b.size.cmp(&a.size));
    if spec.max_files > 0 {
        file_list.truncate(spec.max_files);
    }

    let mut group_list: Vec<GroupItem> = groups
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|(path, (size, count))| {
            let root_files = path == spec.root;
            GroupItem {
                name: if root_files {
                    String::new()
                } else {
                    display_name(&path)
                },
                path: if root_files {
                    String::new()
                } else {
                    path.to_string_lossy().into_owned()
                },
                size,
                files: count,
                root_files,
            }
        })
        .collect();
    group_list.sort_unstable_by(|a, b| b.size.cmp(&a.size));

    WalkOut {
        files: file_list,
        groups: group_list,
        files_seen: files_seen.load(Ordering::Relaxed),
        bytes_seen: bytes_seen.load(Ordering::Relaxed),
        errors: errors.load(Ordering::Relaxed),
        canceled: cancel.load(Ordering::SeqCst),
        elapsed_ms: started.elapsed().as_millis() as u64,
        age_buckets: age_stats.into_inner().unwrap(),
    }
}
