//! 文件粉碎：资源管理器右键菜单「文件粉碎」→ 悬浮球气泡确认 → 覆盖写 + 删除。
//!
//! 设计要点（与项目既有工具保持一致）：
//! - 右键菜单只把路径入队并弹出确认气泡，**绝不直接删**；用户点「开始粉碎」才动手。
//! - 删除前用随机数据覆盖一遍再删文件，目录自底向上递归删。
//! - 文件被其它进程占用时，用 Restart Manager 找出占用进程；用户允许后结束它们再重试；
//!   仍删不掉的（多为需要管理员权限的系统文件）登记为「重启后删除」。
//! - 遍历 / 覆盖 / 删除都在后台线程跑，按时间节流推进度，可随时取消。

pub mod engine;
pub mod lock;
pub mod shell;

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, State};

fn tr(zh: &str, en: &str) -> String {
    crate::i18n::t(zh, en)
}

/// 待处理路径 + 任务运行状态
#[derive(Default)]
pub struct ShredState {
    /// 右键菜单 / 开机参数带进来的路径，等前端取走
    pub pending: Mutex<Vec<String>>,
    pub running: Arc<AtomicBool>,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ShredItem {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub files: u64,
    pub exists: bool,
    /// 不可粉碎的原因（路径不存在 / 是磁盘根目录 …）
    pub error: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    pub paths: Vec<String>,
    /// 被占用时是否结束占用进程（前端默认勾选）
    #[serde(default = "default_true")]
    pub kill_locks: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    percent: u8,
    done: u64,
    total: u64,
    files_done: u64,
    files_total: u64,
    current: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DonePayload {
    canceled: bool,
    ok_files: u64,
    ok_dirs: u64,
    failed: u64,
    freed: u64,
    killed: Vec<String>,
    failed_items: Vec<String>,
    reboot: Vec<String>,
    elapsed_ms: u64,
}

// ============== 入队 / 取队 ==============

/// 右键菜单与其它实例转交的共同入口：入队 + 让悬浮球弹出确认气泡。
pub fn stage_and_open(app: &AppHandle, paths: Vec<String>) {
    let filtered: Vec<String> = paths
        .into_iter()
        .filter(|p| Path::new(p).exists())
        .collect();
    if filtered.is_empty() {
        return;
    }
    {
        let state = app.state::<ShredState>();
        let mut pending = state.pending.lock().unwrap();
        for p in filtered {
            if !pending.iter().any(|x| x.eq_ignore_ascii_case(&p)) {
                pending.push(p);
            }
        }
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
        let _ = w.emit("shred-open", ());
    }
}

/// 命令行里 `--shred` 后面的路径（资源管理器右键会为每个选中项各拉起一次）。
pub fn paths_from_args() -> Vec<String> {
    let mut out = Vec::new();
    let mut expecting = false;
    for (i, arg) in std::env::args().enumerate() {
        if i == 0 {
            continue;
        }
        if arg == "--shred" {
            expecting = true;
            continue;
        }
        if !expecting {
            continue;
        }
        // 其它内部开关不当作路径
        if arg.starts_with("--") {
            continue;
        }
        if Path::new(&arg).exists() {
            out.push(arg);
        }
    }
    out
}

#[tauri::command]
pub fn shred_take_pending(state: State<'_, ShredState>) -> Vec<String> {
    std::mem::take(&mut *state.pending.lock().unwrap())
}

/// 测量待粉碎目标。遍历可能很慢（大文件夹），丢到阻塞线程里跑，别卡界面。
#[tauri::command]
pub async fn shred_inspect(paths: Vec<String>) -> Result<Vec<ShredItem>, String> {
    tauri::async_runtime::spawn_blocking(move || inspect(paths))
        .await
        .map_err(|e| e.to_string())
}

fn inspect(paths: Vec<String>) -> Vec<ShredItem> {
    let mut out: Vec<ShredItem> = Vec::new();
    for raw in paths {
        let path = std::path::PathBuf::from(&raw);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| raw.clone());
        if out.iter().any(|x| x.path.eq_ignore_ascii_case(&raw)) {
            continue;
        }
        if !path.exists() {
            out.push(ShredItem {
                path: raw,
                name,
                is_dir: false,
                size: 0,
                files: 0,
                exists: false,
                error: tr("路径不存在", "Path not found"),
            });
            continue;
        }
        if let Some(reason) = engine::guard(&path) {
            out.push(ShredItem {
                path: raw,
                name,
                is_dir: path.is_dir(),
                size: 0,
                files: 0,
                exists: true,
                error: reason,
            });
            continue;
        }
        let (is_dir, size, files, err) = engine::measure(&path);
        out.push(ShredItem {
            path: raw,
            name,
            is_dir,
            size,
            files,
            exists: true,
            error: err.unwrap_or_default(),
        });
    }
    out
}

#[tauri::command]
pub fn shred_start(app: AppHandle, req: StartRequest) -> Result<(), String> {
    let state = app.state::<ShredState>();
    if state.running.swap(true, Ordering::SeqCst) {
        return Err(tr("已有粉碎任务在运行", "A shred job is already running"));
    }
    state.cancel.store(false, Ordering::SeqCst);

    let paths: Vec<String> = req
        .paths
        .into_iter()
        .filter(|p| Path::new(p).exists())
        .collect();
    if paths.is_empty() {
        state.running.store(false, Ordering::SeqCst);
        return Err(tr("没有可粉碎的路径", "Nothing to shred"));
    }

    let opts = engine::Options {
        kill_locks: req.kill_locks,
    };
    let app_handle = app.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let state = app_handle.state::<ShredState>();
        let cancel = state.cancel.clone();
        let running = state.running.clone();

        let emit_app = app_handle.clone();
        let summary = engine::run(&paths, &opts, &cancel, |p| {
            let _ = emit_app.emit(
                "shred-progress",
                ProgressPayload {
                    percent: p.percent,
                    done: p.done,
                    total: p.total,
                    files_done: p.files_done,
                    files_total: p.files_total,
                    current: p.current,
                },
            );
        });

        let _ = app_handle.emit(
            "shred-done",
            DonePayload {
                canceled: summary.canceled,
                ok_files: summary.ok_files,
                ok_dirs: summary.ok_dirs,
                failed: summary.failed,
                freed: summary.freed,
                killed: summary.killed,
                failed_items: summary.failed_items,
                reboot: summary.reboot,
                elapsed_ms: started.elapsed().as_millis() as u64,
            },
        );
        running.store(false, Ordering::SeqCst);
    });
    Ok(())
}

#[tauri::command]
pub fn shred_cancel(app: AppHandle) {
    let state = app.state::<ShredState>();
    state.cancel.store(true, Ordering::SeqCst);
}

/// 启动时注册右键菜单（安装目录变了也会自动改过来）；失败只记日志。
pub fn ensure_shell_menu() {
    match shell::ensure_registered() {
        Ok(()) => crate::diag("文件粉碎右键菜单已注册"),
        Err(e) => crate::diag(&format!("注册文件粉碎右键菜单失败: {e}")),
    }
}
