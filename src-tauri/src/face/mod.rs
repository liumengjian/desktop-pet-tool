// 人脸比对：两组照片各自提特征，跨组一一对应，最后出一份 Word（每页一对）。
//
// 流程都在随包 Python 里做（insightface 那套 ONNX 模型只在 Python 侧加载），
// 本模块只负责：选图、校验、拉起 sidecar、逐行转进度、收尾。
//
// 为什么不在主进程做推理：SCRFD + ResNet50 常驻内存 300–500MB，且单张 CPU 推理
// 0.3–1s，放进 Tauri 主进程会把悬浮球本身拖卡；sidecar 跑完就退，进程级隔离。

pub mod models;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

/// 单次任务超时（秒）：50 对人 ≈ 100 张，单张 0.3–1s，留足重试余量。
const TIMEOUT_SECS: u64 = 900;

pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp"];

#[derive(Default)]
pub struct FaceState {
    pub cancel: AtomicBool,
    pub running: AtomicBool,
}

fn tr(zh: &str, en: &str) -> String {
    crate::i18n::t(zh, en)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PickItem {
    path: String,
    name: String,
    size_label: String,
    ok: bool,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MatchJob {
    group_a: Vec<String>,
    group_b: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    percent: f32,
    label: String,
    name: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct OutputItem {
    path: String,
    name: String,
    note: String,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct Summary {
    pairs: usize,
    gray: usize,
    unmatched_a: usize,
    unmatched_b: usize,
    skipped: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DonePayload {
    ok: bool,
    message: String,
    outdir: String,
    outputs: Vec<OutputItem>,
    summary: Summary,
}

/// sidecar 的每一行 JSON：进度行与终行共用一套字段，缺失的按键默认值。
#[derive(Deserialize, Default)]
#[allow(dead_code)]
struct LineMsg {
    #[serde(default)]
    t: String,
    #[serde(default)]
    pct: f32,
    #[serde(default)]
    label: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    out: String,
    #[serde(default)]
    pairs: usize,
    #[serde(default)]
    gray: usize,
    #[serde(default, rename = "unmatchedA")]
    unmatched_a: usize,
    #[serde(default, rename = "unmatchedB")]
    unmatched_b: usize,
    #[serde(default)]
    skipped: usize,
}

fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.0} KB", (b / KB).max(1.0))
    }
}

fn emit_progress(app: &AppHandle, percent: f32, label: &str, name: &str) {
    let _ = app.emit_to(
        "main",
        "face-progress",
        ProgressPayload {
            percent: percent.clamp(0.0, 100.0),
            label: label.to_string(),
            name: name.to_string(),
        },
    );
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{}{}", stem, ext));
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{}({}){}", stem, n, ext));
        n += 1;
    }
    candidate
}

// ---------------- 命令 ----------------

/// 选一组照片（可多选）。选择框期间不做裁剪收起的判断，由前端屏蔽外部点击。
#[tauri::command]
pub async fn face_pick_group(app: AppHandle) -> Result<Vec<PickItem>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        app.dialog()
            .file()
            .add_filter(
                tr("照片", "Photos").as_str(),
                &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp"],
            )
            .set_title(tr("选择一组照片", "Choose photos for this group"))
            .blocking_pick_files()
    })
    .await
    .map_err(|e| e.to_string())?;

    let Some(list) = picked else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for item in list {
        let path = item.to_string();
        let p = Path::new(&path);
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let ok = IMAGE_EXTS.contains(&ext_of(p).as_str());
        out.push(PickItem {
            path,
            name,
            size_label: human_size(size),
            ok,
        });
    }
    Ok(out)
}

/// 校验前端带来的路径（来自拖放或历史记录）：过滤掉不存在的、非图片的。
#[tauri::command]
pub fn face_inspect(paths: Vec<String>) -> Vec<PickItem> {
    paths
        .into_iter()
        .filter_map(|path| {
            let p = Path::new(&path);
            if !p.is_file() {
                return None;
            }
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            let ok = IMAGE_EXTS.contains(&ext_of(p).as_str());
            Some(PickItem {
                path,
                name,
                size_label: human_size(size),
                ok,
            })
        })
        .collect()
}

#[tauri::command]
pub fn face_status(app: AppHandle) -> (bool, String) {
    models::status(&app)
}

#[tauri::command]
pub async fn face_start(
    app: AppHandle,
    state: State<'_, FaceState>,
    job: MatchJob,
) -> Result<(), String> {
    if state.running.swap(true, Ordering::SeqCst) {
        return Err(tr("已有比对任务在进行中", "A matching task is already running"));
    }
    if job.group_a.is_empty() || job.group_b.is_empty() {
        state.running.store(false, Ordering::SeqCst);
        return Err(tr(
            "两组都需要至少一张照片",
            "Each group needs at least one photo",
        ));
    }
    state.cancel.store(false, Ordering::SeqCst);

    let outdir = crate::docs::resolve_output_dir(&app);
    let app_handle = app.clone();
    let job_owned = job.clone();

    std::thread::spawn(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_job(&app_handle, &job_owned, &outdir)
        }));

        let payload = match outcome {
            Ok(Ok(p)) => p,
            Ok(Err(message)) => DonePayload {
                ok: false,
                message,
                outdir: outdir.to_string_lossy().to_string(),
                outputs: Vec::new(),
                summary: Summary::default(),
            },
            Err(_) => DonePayload {
                ok: false,
                message: tr("比对失败：处理照片时出现异常", "Failed: photo processing threw an error"),
                outdir: outdir.to_string_lossy().to_string(),
                outputs: Vec::new(),
                summary: Summary::default(),
            },
        };

        let state = app_handle.state::<FaceState>();
        state.running.store(false, Ordering::SeqCst);
        let _ = app_handle.emit_to("main", "face-done", payload);
    });

    Ok(())
}

#[tauri::command]
pub fn face_cancel(state: State<'_, FaceState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub fn face_open_outdir(app: AppHandle) -> Result<(), String> {
    let dir = crate::docs::resolve_output_dir(&app);
    let path = dir.to_string_lossy().to_string();
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("{}{}", tr("打开目录失败: ", "Failed to open the folder: "), e))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        return Err(tr("仅支持 Windows", "Windows only"));
    }
    Ok(())
}

// ---------------- 任务 ----------------

fn write_list(dir: &Path, tag: &str, paths: &[String]) -> Result<PathBuf, String> {
    let path = dir.join(format!("face-{}.json", tag));
    std::fs::write(
        &path,
        serde_json::to_vec(paths).map_err(|e| format!("序列化路径失败: {}", e))?,
    )
    .map_err(|e| format!("写临时文件失败: {}", e))?;
    Ok(path)
}

fn run_job(app: &AppHandle, job: &MatchJob, outdir: &Path) -> Result<DonePayload, String> {
    emit_progress(app, 0.0, &tr("准备中", "Preparing"), "");

    // 模型与 onnxruntime：首次使用会下载约 190MB，进度直接进同一条进度条
    let ready = models::ensure(app, &|pct, label| {
        // 下载阶段占前 20%，推理阶段从 20% 起
        emit_progress(app, pct * 0.2, label, "");
    })?;

    let root = crate::docs::sidecar_pdf::resolve_root(app)
        .ok_or_else(|| tr("内置照片引擎未随包安装", "The photo engine is not installed"))?;
    let site = crate::docs::sidecar_deps::ensure(app)
        .map_err(|e| format!("{}: {}", tr("图像依赖未就绪", "Image deps not ready"), e))?;

    let py = root.join("python").join("python.exe");
    let script = root.join("scripts").join("face_match.py");
    if !script.exists() {
        return Err(tr("缺少比对脚本 face_match.py", "Missing face_match.py").to_string());
    }

    let tmp = std::env::temp_dir();
    let stamp = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let list_a = write_list(&tmp, &format!("a-{}", stamp), &job.group_a)?;
    let list_b = write_list(&tmp, &format!("b-{}", stamp), &job.group_b)?;

    let out = unique_path(outdir, &tr("人脸比对", "Face matching"), ".docx");

    let state = app.state::<FaceState>();
    if state.cancel.load(Ordering::SeqCst) {
        let _ = std::fs::remove_file(&list_a);
        let _ = std::fs::remove_file(&list_b);
        return Err(tr("已取消", "Cancelled"));
    }

    let mut cmd = Command::new(&py);
    cmd.current_dir(root.join("python"));
    cmd.arg(&script)
        .arg("--a")
        .arg(&list_a)
        .arg("--b")
        .arg(&list_b)
        .arg("--out")
        .arg(&out)
        .arg("--det")
        .arg(&ready.det)
        .arg("--rec")
        .arg(&ready.rec)
        .arg("--group-a")
        .arg(tr("第一组", "Group A"))
        .arg("--group-b")
        .arg(tr("第二组", "Group B"));
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.env("PYTHONIOENCODING", "utf-8");
    cmd.env("PYTHONUNBUFFERED", "1");
    cmd.env("PDFCONV_SITE_PACKAGES", &site);
    cmd.env("FACE_DEPS", &ready.deps);
    cmd.env_remove("PYTHONPATH");

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{}: {}", tr("启动比对引擎失败", "Failed to start engine"), e))?;

    // stdout：逐行解析进度 / 终行
    let mut final_msg: Option<LineMsg> = None;
    let stdout = child.stdout.take();
    let app_reader = app.clone();
    let reader = std::thread::spawn(move || {
        let mut last: Option<LineMsg> = None;
        if let Some(out) = stdout {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                let Ok(msg) = serde_json::from_str::<LineMsg>(&line) else {
                    continue;
                };
                if msg.t == "p" {
                    // 推理阶段映射到 20–99%
                    let pct = 20.0 + msg.pct * 0.8;
                    emit_progress(&app_reader, pct, &msg.label, &msg.name);
                } else if msg.t == "done" {
                    last = Some(msg);
                }
            }
        }
        last
    });

    // stderr：只留尾部用于报错
    let stderr = child.stderr.take();
    let err_buf = std::sync::Arc::new(Mutex::new(String::new()));
    let err_sink = err_buf.clone();
    let err_thread = std::thread::spawn(move || {
        if let Some(err) = stderr {
            for line in BufReader::new(err).lines() {
                let Ok(line) = line else { break };
                let mut buf = err_sink.lock().unwrap();
                buf.push_str(&line);
                buf.push('\n');
                // 防止异常刷屏
                if buf.len() > 4000 {
                    let cut = buf.len() - 2000;
                    buf.drain(..cut);
                }
            }
        }
    });

    let start = Instant::now();
    let mut cancelled = false;
    loop {
        let st = app.state::<FaceState>();
        if st.cancel.load(Ordering::SeqCst) {
            cancelled = true;
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(TIMEOUT_SECS) {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => return Err(format!("{}: {}", tr("等待比对进程失败", "Wait failed"), e)),
        }
    }

    if let Ok(msg) = reader.join() {
        final_msg = msg;
    }
    let _ = err_thread.join();
    let _ = std::fs::remove_file(&list_a);
    let _ = std::fs::remove_file(&list_b);

    if cancelled {
        let _ = std::fs::remove_file(&out);
        return Err(tr("已取消", "Cancelled"));
    }

    let msg = final_msg.unwrap_or_default();
    if msg.status != "ok" {
        let detail = msg.message.clone();
        let stderr_tail = err_buf.lock().unwrap().clone();
        let reason = if !detail.is_empty() {
            detail
        } else if !stderr_tail.trim().is_empty() {
            stderr_tail
                .trim()
                .lines()
                .last()
                .unwrap_or("")
                .to_string()
        } else {
            tr("比对未产出结果", "No result produced").to_string()
        };
        return Err(reason);
    }

    if !out.exists() {
        return Err(tr("比对产物缺失", "Output file is missing").to_string());
    }

    let outputs = vec![OutputItem {
        path: out.to_string_lossy().to_string(),
        name: out
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        note: format!("{} {}", msg.pairs, tr("组配对", "pairs")),
    }];

    let summary = Summary {
        pairs: msg.pairs,
        gray: msg.gray,
        unmatched_a: msg.unmatched_a,
        unmatched_b: msg.unmatched_b,
        skipped: msg.skipped,
    };

    let mut message = format!(
        "{} {} {}",
        tr("完成：共配对", "Done:"),
        msg.pairs,
        tr("组", "pairs")
    );
    if msg.gray > 0 {
        message.push_str(&format!(
            "，{} {} {}",
            tr("其中", "of which"),
            msg.gray,
            tr("组待人工复核", "need review")
        ));
    }
    if msg.unmatched_a > 0 || msg.unmatched_b > 0 {
        message.push_str(&format!(
            "，{} {}",
            tr("未匹配", "unmatched"),
            msg.unmatched_a + msg.unmatched_b
        ));
    }
    if msg.skipped > 0 {
        message.push_str(&format!(
            "，{} {}",
            tr("跳过", "skipped"),
            msg.skipped
        ));
    }

    Ok(DonePayload {
        ok: true,
        message,
        outdir: outdir.to_string_lossy().to_string(),
        outputs,
        summary,
    })
}
