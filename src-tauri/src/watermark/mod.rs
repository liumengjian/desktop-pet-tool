// 图片水印去除：选一张图 → 前端涂/框出水印区域 → LaMa 模型修复补全背景。
//
// 推理都在随包 Python sidecar 里做（onnxruntime 加载 ONNX 模型），本模块只负责：
// 选图、校验、把涂抹笔画落成 mask 描述文件、拉起 sidecar、逐行转进度、收尾。
//
// 为什么不在主进程做推理：LaMa 常驻内存 + 单张 CPU 推理秒级，放进 Tauri 主进程
// 会把悬浮球本身拖卡；sidecar 跑完就退，进程级隔离。

pub mod models;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

/// 单次任务超时（秒）：单张图 CPU 推理留足余量。
const TIMEOUT_SECS: u64 = 300;

pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp"];

#[derive(Default)]
pub struct WatermarkState {
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

/// 画布显示用缩略图：dataURL + 原图 / 缩略图尺寸，笔画坐标据此换算回原图。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThumbInfo {
    data_url: String,
    orig_w: u32,
    orig_h: u32,
    w: u32,
    h: u32,
}

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 手写 base64：避免为一个缩略图编码再引入 base64 crate。
fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_TABLE[((n >> 18) & 63) as usize] as char);
        out.push(B64_TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64_TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64_TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// 一笔涂抹或一个框选。坐标都是原图像素。
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Stroke {
    /// "brush" | "rect"
    kind: String,
    /// 画笔折线点：[[x, y], ...]
    #[serde(default)]
    points: Vec<Vec<i32>>,
    /// 画笔半径（像素）
    #[serde(default)]
    radius: i32,
    /// 矩形 [x1, y1, x2, y2]
    #[serde(default)]
    rect: Vec<i32>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EraseJob {
    image: String,
    #[serde(default)]
    strokes: Vec<Stroke>,
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
struct DonePayload {
    ok: bool,
    message: String,
    outdir: String,
    outputs: Vec<OutputItem>,
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
        "wm-progress",
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

/// 选一张待处理图片。选择框期间不做裁剪收起的判断，由前端屏蔽外部点击。
#[tauri::command]
pub async fn wm_pick(app: AppHandle) -> Result<Vec<PickItem>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        app.dialog()
            .file()
            .add_filter(
                tr("图片", "Images").as_str(),
                &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp"],
            )
            .set_title(tr("选择一张图片", "Choose an image"))
            .blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;

    let Some(item) = picked else {
        return Ok(Vec::new());
    };
    let path = item.to_string();
    let p = Path::new(&path);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let ok = IMAGE_EXTS.contains(&ext_of(p).as_str());
    Ok(vec![PickItem {
        path,
        name,
        size_label: human_size(size),
        ok,
    }])
}

/// 读取图片并返回画布显示用的 JPEG 缩略图（dataURL）。原图仍由 sidecar 按路径读取。
#[tauri::command]
pub fn wm_thumbnail(path: String) -> Result<ThumbInfo, String> {
    let p = Path::new(&path);
    let img = image::open(p).map_err(|e| format!("无法解码图片: {}", e))?;
    let orig_w = img.width();
    let orig_h = img.height();
    let fit = if orig_w.max(orig_h) <= 1024 {
        img
    } else {
        img.resize(
            1024,
            1024,
            image::imageops::FilterType::Lanczos3,
        )
    };
    let bytes = crate::docs::image_ops::encode_jpeg_bytes(&fit, 85)?;
    let b64 = base64_encode(&bytes);
    Ok(ThumbInfo {
        data_url: format!("data:image/jpeg;base64,{}", b64),
        orig_w,
        orig_h,
        w: fit.width(),
        h: fit.height(),
    })
}

/// 校验前端带来的路径（来自拖放或历史记录）：过滤掉不存在的、非图片的。
#[tauri::command]
pub fn wm_inspect(paths: Vec<String>) -> Vec<PickItem> {
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
pub fn wm_status(app: AppHandle) -> (bool, String) {
    models::status(&app)
}

#[tauri::command]
pub async fn wm_start(
    app: AppHandle,
    state: State<'_, WatermarkState>,
    job: EraseJob,
) -> Result<(), String> {
    if state.running.swap(true, Ordering::SeqCst) {
        return Err(tr("已有去水印任务在进行中", "A watermark task is already running"));
    }
    if job.image.is_empty() {
        state.running.store(false, Ordering::SeqCst);
        return Err(tr("请先选择图片", "Please choose an image"));
    }
    if job.strokes.is_empty() {
        state.running.store(false, Ordering::SeqCst);
        return Err(tr("请先涂抹或框选要去除的水印区域", "Paint or box the watermark area first"));
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
            },
            Err(_) => DonePayload {
                ok: false,
                message: tr("去水印失败：处理图片时出现异常", "Failed: image processing threw an error"),
                outdir: outdir.to_string_lossy().to_string(),
                outputs: Vec::new(),
            },
        };

        let state = app_handle.state::<WatermarkState>();
        state.running.store(false, Ordering::SeqCst);
        let _ = app_handle.emit_to("main", "wm-done", payload);
    });

    Ok(())
}

#[tauri::command]
pub fn wm_cancel(state: State<'_, WatermarkState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub fn wm_open_outdir(app: AppHandle, paths: Option<Vec<String>>) -> Result<(), String> {
    let dir = crate::docs::resolve_output_dir(&app);
    crate::reveal::open_folder_selecting(&dir, paths.as_deref().unwrap_or(&[]))
}

// ---------------- 任务 ----------------

fn write_strokes(dir: &Path, stamp: &str, strokes: &[Stroke]) -> Result<PathBuf, String> {
    let path = dir.join(format!("wm-strokes-{}.json", stamp));
    std::fs::write(
        &path,
        serde_json::to_vec(strokes).map_err(|e| format!("序列化笔画失败: {}", e))?,
    )
    .map_err(|e| format!("写临时文件失败: {}", e))?;
    Ok(path)
}

fn run_job(app: &AppHandle, job: &EraseJob, outdir: &Path) -> Result<DonePayload, String> {
    emit_progress(app, 0.0, &tr("准备中", "Preparing"), "");

    // 模型与 onnxruntime：首次使用会下载，进度直接进同一条进度条
    let ready = models::ensure(app, &|pct, label| {
        // 下载阶段占前 20%，推理阶段从 20% 起
        emit_progress(app, pct * 0.2, label, "");
    })?;

    let root = crate::docs::sidecar_pdf::resolve_root(app)
        .ok_or_else(|| tr("内置图像引擎未随包安装", "The image engine is not installed"))?;
    let site = crate::docs::sidecar_deps::ensure(app)
        .map_err(|e| format!("{}: {}", tr("图像依赖未就绪", "Image deps not ready"), e))?;

    let py = root.join("python").join("python.exe");
    let script = root.join("scripts").join("watermark_erase.py");
    if !script.exists() {
        return Err(tr("缺少修复脚本 watermark_erase.py", "Missing watermark_erase.py").to_string());
    }

    let src = Path::new(&job.image);
    if !src.is_file() {
        return Err(tr("图片不存在", "Image file is missing").to_string());
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
    let strokes_file = write_strokes(&tmp, &stamp, &job.strokes)?;

    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".to_string());
    let out = unique_path(outdir, &format!("{}_removed", stem), ".png");

    let state = app.state::<WatermarkState>();
    if state.cancel.load(Ordering::SeqCst) {
        let _ = std::fs::remove_file(&strokes_file);
        return Err(tr("已取消", "Cancelled"));
    }

    let mut cmd = Command::new(&py);
    cmd.current_dir(root.join("python"));
    cmd.arg(&script)
        .arg("--src")
        .arg(src)
        .arg("--strokes")
        .arg(&strokes_file)
        .arg("--out")
        .arg(&out)
        .arg("--model")
        .arg(&ready.model);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.env("PYTHONIOENCODING", "utf-8");
    cmd.env("PYTHONUNBUFFERED", "1");
    cmd.env("PDFCONV_SITE_PACKAGES", &site);
    cmd.env("WM_DEPS", &ready.deps);
    cmd.env_remove("PYTHONPATH");
    crate::proc::hide_console(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{}: {}", tr("启动修复引擎失败", "Failed to start engine"), e))?;

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
        let st = app.state::<WatermarkState>();
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
            Err(e) => return Err(format!("{}: {}", tr("等待修复进程失败", "Wait failed"), e)),
        }
    }

    if let Ok(msg) = reader.join() {
        final_msg = msg;
    }
    let _ = err_thread.join();
    let _ = std::fs::remove_file(&strokes_file);

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
            tr("修复未产出结果", "No result produced").to_string()
        };
        return Err(reason);
    }

    if !out.exists() {
        return Err(tr("修复产物缺失", "Output file is missing").to_string());
    }

    let outputs = vec![OutputItem {
        path: out.to_string_lossy().to_string(),
        name: out
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        note: tr("已去除水印", "Watermark removed").to_string(),
    }];

    let message = format!(
        "{} {}",
        tr("完成：", "Done:"),
        out.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    );

    Ok(DonePayload {
        ok: true,
        message,
        outdir: outdir.to_string_lossy().to_string(),
        outputs,
    })
}
