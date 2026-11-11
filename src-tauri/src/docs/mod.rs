// 文档工具：压缩 / 转换。
//
// 设计约束（用户拍板）：
//   - 单文件硬上限 2GB；PDF 超过阈值走流式重写，不整份载入内存；
//   - 全部能力由本软件纯 Rust 实现，不调用本机 Office / WPS / LibreOffice / Ghostscript；
//   - 输出统一落到「临时文件夹」（与文档复制下载的产物同目录），完成后前端引导打开。

pub mod image_ops;
pub mod office_ops;
pub mod office_write;
pub mod doc_binary;
pub mod from_pdf;
pub mod pdf_extract;
pub mod pdf_font;
pub mod legacy;
pub mod pptx_write;
pub mod pdf_render;
pub mod pdf_ops;
pub mod pdf_stream;
pub mod pdf_text;
pub mod pdf_write;
pub mod text_ops;
pub mod to_pdf;
pub mod zip_min;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2GB 硬上限

#[derive(Default)]
pub struct DocsState {
    pub pending: Mutex<Vec<String>>,
    pub cancel: AtomicBool,
    pub running: AtomicBool,
}

// ---------------- 类型判定 ----------------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub size_label: String,
    pub kind: String, // image / word / sheet / slides / pdf / other
    pub can_compress: bool,
    pub compress_hint: String,
    pub targets: Vec<String>, // 可转换的目标格式
}

fn human_size(bytes: u64) -> String {
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

fn ext_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn classify(path: &Path) -> FileInfo {
    let ext = ext_of(path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    let (kind, can_compress, compress_hint, targets): (&str, bool, &str, Vec<&str>) = match ext
        .as_str()
    {
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "tif" | "tiff" | "webp" => {
            ("image", true, "降低分辨率与重新编码", vec!["jpg", "png", "bmp", "tif", "pdf"])
        }
        "docx" => (
            "word",
            true,
            "重打包并压缩内嵌图片",
            vec!["txt", "html", "md", "pdf"],
        ),
        "doc" => (
            "word",
            false,
            "旧版 .doc 可直接转换，不支持压缩（转成 .docx 后可再压缩）",
            vec!["txt", "md", "html", "docx", "pdf"],
        ),
        "rtf" => (
            "rtf",
            false,
            "富文本文档，可转成常见格式",
            vec!["txt", "md", "html", "docx", "pdf"],
        ),
        "odt" | "ods" | "odp" => {
            let (kind, extra): (&str, Vec<&str>) = match ext.as_str() {
                "ods" => ("odf", vec!["csv", "xlsx"]),
                "odp" => ("odf", vec![]),
                _ => ("odf", vec![]),
            };
            let mut targets = vec!["txt", "html", "docx", "pdf"];
            if ext != "ods" {
                targets.push("md");
            }
            targets.extend(extra);
            (kind, false, "开放文档格式，可转成常见格式", targets)
        }
        "epub" => (
            "epub",
            false,
            "电子书，可转成常见格式",
            vec!["txt", "md", "html", "docx", "pdf"],
        ),
        "xlsx" => (
            "sheet",
            true,
            "重打包并压缩内嵌图片",
            vec!["csv", "txt", "html", "pdf"],
        ),
        "xls" => ("sheet", false, "旧版 .xls 暂不支持", vec![]),
        "pptx" => (
            "slides",
            true,
            "重打包并压缩内嵌图片",
            vec!["txt", "html", "md", "pdf"],
        ),
        "ppt" => ("slides", false, "旧版 .ppt 暂不支持", vec![]),
        "pdf" => (
            "pdf",
            true,
            "剥离私有数据 + 图片重采样",
            vec!["txt", "md", "html", "docx", "xlsx", "pptx", "jpg", "png"],
        ),
        "txt" => ("text", false, "纯文本无需压缩", vec!["docx", "html", "pdf"]),
        "md" => (
            "markdown",
            false,
            "纯文本无需压缩",
            vec!["docx", "html", "txt", "pdf"],
        ),
        "html" | "htm" => ("html", false, "网页文件无需压缩", vec!["txt", "md", "pdf"]),
        "csv" => ("csv", false, "CSV 无需压缩", vec!["xlsx", "txt", "html", "pdf"]),
        _ => ("other", false, "该类型暂不支持", vec![]),
    };

    FileInfo {
        path: path.to_string_lossy().to_string(),
        name,
        size,
        size_label: human_size(size),
        kind: kind.to_string(),
        can_compress,
        compress_hint: compress_hint.to_string(),
        targets: targets.into_iter().map(|s| s.to_string()).collect(),
    }
}

/// 输出目录：统一落到「临时文件夹」（与「文档复制」下载的产物同一个目录），
/// 这样右键菜单的「打开临时文件夹」看得到全部产物，「清理临时文件夹」一次清干净。
/// 配置不可用时回退到 文档/悬浮球文档输出。
pub fn resolve_output_dir(app: &AppHandle) -> PathBuf {
    if let Some(state) = app.try_state::<crate::AppState>() {
        let configured = state.config.lock().unwrap().temp_dir.clone();
        if !configured.is_empty() {
            let dir = PathBuf::from(&configured);
            if std::fs::create_dir_all(&dir).is_ok() {
                return dir;
            }
        }
    }
    let dir = app
        .path()
        .document_dir()
        .or_else(|_| app.path().home_dir())
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("悬浮球文档输出");
    if std::fs::create_dir_all(&dir).is_err() {
        return std::env::temp_dir();
    }
    dir
}

fn unique_path(dir: &Path, stem: &str, suffix: &str, ext: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{}{}{}", stem, suffix, ext));
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{}{}({}){}", stem, suffix, n, ext));
        n += 1;
    }
    candidate
}

// ---------------- 命令 ----------------

#[tauri::command]
pub async fn docs_pick_files(app: AppHandle) -> Result<Vec<String>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        app.dialog()
            .file()
            .add_filter(
                "文档与图片",
                &[
                    "pdf", "docx", "doc", "xlsx", "xls", "pptx", "ppt", "rtf", "odt", "ods",
                    "odp", "epub", "csv", "txt", "md", "html", "htm", "png", "jpg", "jpeg",
                    "bmp", "gif", "tif", "tiff", "webp",
                ],
            )
            .blocking_pick_files()
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    if let Some(list) = picked {
        for f in list {
            if let Ok(p) = f.into_path() {
                out.push(p.to_string_lossy().to_string());
            }
        }
    }
    Ok(out)
}

#[tauri::command]
pub fn docs_take_pending(state: State<'_, DocsState>) -> Vec<String> {
    let mut list = state.pending.lock().unwrap();
    std::mem::take(&mut *list)
}

#[tauri::command]
pub fn docs_stage(state: State<'_, DocsState>, paths: Vec<String>) {
    let mut list = state.pending.lock().unwrap();
    for p in paths {
        if !list.contains(&p) {
            list.push(p);
        }
    }
}

#[tauri::command]
pub fn docs_inspect(paths: Vec<String>) -> Vec<FileInfo> {
    paths
        .iter()
        .map(|p| classify(Path::new(p)))
        .collect::<Vec<_>>()
        .into_iter()
        .filter(|f| f.size <= MAX_FILE_BYTES)
        .collect()
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobRequest {
    pub paths: Vec<String>,
    pub mode: String,           // compress | convert
    pub target: Option<String>, // convert 的目标格式
    pub level: Option<String>,  // small | normal | high
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    index: usize,
    total: usize,
    percent: f32,
    label: String,
    name: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct OutputItem {
    path: String,
    name: String,
    before: String,
    after: String,
    note: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DonePayload {
    ok: bool,
    message: String,
    outdir: String,
    outputs: Vec<OutputItem>,
}

fn emit_progress(app: &AppHandle, payload: ProgressPayload) {
    // 文档交互已并入悬浮球本体，进度事件发给主窗口。
    let _ = app.emit_to("main", "docs-progress", payload);
}

fn emit_mood(app: &AppHandle, mood: &str) {
    let _ = app.emit_to("main", "docs-mood", mood.to_string());
}

#[tauri::command]
pub async fn docs_start(app: AppHandle, state: State<'_, DocsState>, job: JobRequest) -> Result<(), String> {
    if state.running.swap(true, Ordering::SeqCst) {
        return Err("已有任务在进行中".to_string());
    }
    state.cancel.store(false, Ordering::SeqCst);
    emit_mood(&app, "busy");

    let outdir = resolve_output_dir(&app);
    let app_handle = app.clone();

    std::thread::spawn(move || {
        // release 配了 panic=abort，但开发构建里第三方解析器 panic 会带走线程；
        // 统一兜住，保证 docs 窗口一定收到结束事件，不会卡在进度页。
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_job(&app_handle, &job, &outdir)
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
                message: "处理失败：解析文件时出现异常".to_string(),
                outdir: outdir.to_string_lossy().to_string(),
                outputs: Vec::new(),
            },
        };

        let state = app_handle.state::<DocsState>();
        state.running.store(false, Ordering::SeqCst);

        let mood = if payload.ok { "idle" } else { "err" };
        let _ = app_handle.emit_to("main", "docs-done", payload);
        emit_mood(&app_handle, mood);
    });

    Ok(())
}

#[tauri::command]
pub fn docs_cancel(state: State<'_, DocsState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub fn docs_open_outdir(app: AppHandle) -> Result<(), String> {
    let dir = resolve_output_dir(&app);
    let path = dir.to_string_lossy().to_string();
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("打开目录失败: {}", e))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        return Err("仅支持 Windows".to_string());
    }
    Ok(())
}

fn run_job(app: &AppHandle, job: &JobRequest, outdir: &Path) -> Result<DonePayload, String> {
    let total = job.paths.len();
    let mut outputs: Vec<OutputItem> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for (index, raw) in job.paths.iter().enumerate() {
        let st = app.state::<DocsState>();
        if st.cancel.load(Ordering::SeqCst) {
            failures.push("任务已取消".to_string());
            break;
        }
        let src = Path::new(raw);
        let info = classify(src);
        let name = info.name.clone();
        if info.size > MAX_FILE_BYTES {
            failures.push(format!("{} 超过 2GB 上限", name));
            continue;
        }
        if info.size == 0 {
            failures.push(format!("{} 无法读取或为空", name));
            continue;
        }

        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "output".to_string());

        let level = job.level.clone().unwrap_or_else(|| "normal".to_string());
        let target = job.target.clone().unwrap_or_default();

        // 决定目标文件名
        let (suffix, mut out_ext) = if job.mode == "compress" {
            ("_压缩", ext_of(src))
        } else {
            ("", target.clone())
        };
        // PDF 转图片时多页会打包成 zip，文件名也要跟着变，别出现 .png 里装 zip
        if job.mode == "convert"
            && info.kind == "pdf"
            && (target == "jpg" || target == "png")
            && pdf_render::page_count(src).map(|n| n > 1).unwrap_or(false)
        {
            out_ext = "zip".to_string();
        }
        if job.mode == "convert" && target.is_empty() {
            failures.push(format!("{} 未选择目标格式", name));
            continue;
        }
        let dst = unique_path(outdir, &stem, suffix, &format!(".{}", out_ext));

        // 前置能力校验
        if job.mode == "compress" && !info.can_compress {
            failures.push(format!("{}：{}", name, info.compress_hint));
            continue;
        }
        if job.mode == "convert" {
            if !info.targets.iter().any(|t| t == &target) {
                failures.push(format!(
                    "{} → {}：当前版本不支持该格式对",
                    name, target
                ));
                continue;
            }
        }

        let app_for_progress = app.clone();
        let index_for_progress = index;
        let name_for_progress = name.clone();
        // 单文件内部进度折算到整体进度，多文件时进度条不会每个文件都归零重来
        let overall = |pct: f32| {
            ((index_for_progress as f32 + (pct / 100.0).clamp(0.0, 1.0))
                / total.max(1) as f32)
                * 100.0
        };
        let progress = move |percent: f32, label: &str| {
            let pct = ((index_for_progress as f32 + (percent / 100.0).clamp(0.0, 1.0))
                / total.max(1) as f32)
                * 100.0;
            emit_progress(
                &app_for_progress,
                ProgressPayload {
                    index: index_for_progress,
                    total,
                    percent: pct,
                    label: label.to_string(),
                    name: name_for_progress.clone(),
                },
            );
        };

        // 不支持进度回调的路径（图片转换 / 文本提取）也先给出一个起点
        emit_progress(
            app,
            ProgressPayload {
                index,
                total,
                percent: overall(0.0),
                label: if job.mode == "compress" {
                    "开始压缩".to_string()
                } else {
                    "开始转换".to_string()
                },
                name: name.clone(),
            },
        );

        let cancelled = {
            let handle = app.clone();
            move || handle.state::<DocsState>().cancel.load(Ordering::SeqCst)
        };

        let mut extra_note: Option<String> = None;
        let outcome: Result<(), String> = match (job.mode.as_str(), info.kind.as_str()) {
            ("compress", "pdf") => pdf_ops::compress(src, &dst, &level, &cancelled, &progress),
            ("compress", "image") => image_ops::compress(src, &dst, &level, &cancelled),
            ("compress", "word") | ("compress", "sheet") | ("compress", "slides") => {
                office_ops::compress(src, &dst, &level, &cancelled, &progress)
            }
            ("convert", "image") if target == "pdf" => {
                pdf_write::images_to_pdf(&[src.to_path_buf()], &dst, &cancelled)
            }
            ("convert", "image") => image_ops::convert(src, &dst, &target, &cancelled),
            ("convert", "word") if ext_of(src) == "doc" => {
                doc_binary::convert(src, &dst, &target, &cancelled)
            }
            ("convert", "word") => match target.as_str() {
                "txt" => text_ops::docx_to_txt(src, &dst, &cancelled),
                "html" => text_ops::docx_to_html(src, &dst, &cancelled),
                "md" => text_ops::docx_to_md(src, &dst, &cancelled),
                "pdf" => to_pdf::convert("word", src, &dst, &cancelled).map(|_| ()),
                _ => Err(format!("docx → {}：暂不支持", target)),
            },
            ("convert", "sheet") => match target.as_str() {
                "csv" => text_ops::xlsx_to_csv(src, &dst, &cancelled),
                "txt" => text_ops::xlsx_to_txt(src, &dst, &cancelled),
                "html" => text_ops::xlsx_to_html(src, &dst, &cancelled),
                "pdf" => to_pdf::convert("sheet", src, &dst, &cancelled).map(|_| ()),
                _ => Err(format!("xlsx → {}：暂不支持", target)),
            },
            ("convert", "slides") => match target.as_str() {
                "txt" => text_ops::pptx_to_txt(src, &dst, &cancelled),
                "html" => text_ops::pptx_to_html(src, &dst, &cancelled),
                "md" => text_ops::pptx_to_md(src, &dst, &cancelled),
                "pdf" => to_pdf::convert("slides", src, &dst, &cancelled).map(|_| ()),
                _ => Err(format!("pptx → {}：暂不支持", target)),
            },
            ("convert", "pdf") => match target.as_str() {
                "txt" => from_pdf::to_txt(src, &dst, &cancelled),
                "md" => from_pdf::to_md(src, &dst, &cancelled),
                "html" => from_pdf::to_html(src, &dst, &cancelled),
                "docx" => from_pdf::to_docx(src, &dst, &cancelled),
                "xlsx" => from_pdf::to_xlsx(src, &dst, &cancelled),
                "pptx" => from_pdf::to_pptx(src, &dst, 150.0, &cancelled, &mut extra_note),
                "jpg" | "png" => {
                    from_pdf::to_image(src, &dst, &target, 150.0, &cancelled, &mut extra_note)
                }
                _ => Err(format!("pdf → {}：暂不支持", target)),
            },
            ("convert", "text") => match target.as_str() {
                "docx" => office_write::text_to_docx(src, &dst, false, &cancelled),
                "html" => text_ops::txt_to_html(src, &dst, &cancelled),
                "pdf" => to_pdf::convert("text", src, &dst, &cancelled).map(|_| ()),
                _ => Err(format!("txt → {}：暂不支持", target)),
            },
            ("convert", "markdown") => match target.as_str() {
                "docx" => office_write::text_to_docx(src, &dst, true, &cancelled),
                "html" => text_ops::md_to_html(src, &dst, &cancelled),
                "pdf" => to_pdf::convert("markdown", src, &dst, &cancelled).map(|_| ()),
                "txt" => {
                    let md = std::fs::read_to_string(src)
                        .map_err(|e| format!("读取失败: {}", e))?;
                    std::fs::write(&dst, text_ops::md_to_plain(&md))
                        .map_err(|e| format!("写入失败: {}", e))
                }
                _ => Err(format!("md → {}：暂不支持", target)),
            },
            ("convert", "html") => match target.as_str() {
                "txt" => text_ops::html_to_txt(src, &dst, &cancelled),
                "md" => text_ops::html_to_md(src, &dst, &cancelled),
                "pdf" => to_pdf::convert("html", src, &dst, &cancelled).map(|_| ()),
                _ => Err(format!("html → {}：暂不支持", target)),
            },
            ("convert", "rtf") | ("convert", "odf") | ("convert", "epub") => {
                let ext = ext_of(src);
                let blocks = legacy::blocks_for(src, &ext)?;
                legacy::convert_blocks(src, &blocks, &dst, &target, &cancelled)
            }
            ("convert", "csv") => match target.as_str() {
                "xlsx" => office_write::csv_to_xlsx(src, &dst, &cancelled),
                "pdf" => to_pdf::convert("csv", src, &dst, &cancelled).map(|_| ()),
                "txt" => {
                    let raw = std::fs::read(src).map_err(|e| format!("读取失败: {}", e))?;
                    let content = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
                        String::from_utf8_lossy(&raw[3..]).to_string()
                    } else {
                        String::from_utf8_lossy(&raw).to_string()
                    };
                    std::fs::write(&dst, content.replace("\r\n", "\n"))
                        .map_err(|e| format!("写入失败: {}", e))
                }
                "html" => {
                    let raw = std::fs::read(src).map_err(|e| format!("读取失败: {}", e))?;
                    let content = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
                        String::from_utf8_lossy(&raw[3..]).to_string()
                    } else {
                        String::from_utf8_lossy(&raw).to_string()
                    };
                    let html = format!(
                        "<!DOCTYPE html>\n<html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><title>{}</title></head><body><pre>{}</pre></body></html>\n",
                        src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                        content
                            .replace('&', "&amp;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;")
                    );
                    std::fs::write(&dst, html).map_err(|e| format!("写入失败: {}", e))
                }
                _ => Err(format!("csv → {}：暂不支持", target)),
            },
            _ => Err("当前版本不支持该类型".to_string()),
        };

        match outcome {
            Ok(()) => {
                let after = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
                if after == 0 {
                    let _ = std::fs::remove_file(&dst);
                    failures.push(format!("{}：输出为空", name));
                    continue;
                }
                // 压缩不倒退：比原文件还大就丢弃，避免误导
                if job.mode == "compress" && after >= info.size {
                    let _ = std::fs::remove_file(&dst);
                    failures.push(format!(
                        "{}：已是最小（{} → {}），未生成新文件",
                        name,
                        info.size_label,
                        human_size(after)
                    ));
                    continue;
                }
                let note = if job.mode == "compress" {
                    let ratio = after as f64 / info.size as f64 * 100.0;
                    // 文字/表格类文件本来就只有 XML，重打包收益天然接近 0；
                    // 如实标出来，避免用户以为「更小」档没生效。
                    if ratio >= 88.0 {
                        format!("{:.0}% · 已接近极限（以文字为主，无更多可压图片）", ratio)
                    } else {
                        format!("{:.0}%", ratio)
                    }
                } else {
                    extra_note.take().unwrap_or_default()
                };
                outputs.push(OutputItem {
                    path: dst.to_string_lossy().to_string(),
                    name: dst
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    before: info.size_label.clone(),
                    after: human_size(after),
                    note,
                });
            }
            Err(err) => {
                let _ = std::fs::remove_file(&dst);
                if err == "已取消" {
                    failures.push("任务已取消".to_string());
                    break;
                }
                failures.push(format!("{}：{}", name, err));
            }
        }
    }

    let ok = !outputs.is_empty();
    let message = if ok {
        if failures.is_empty() {
            format!("完成 {} 个文件", outputs.len())
        } else {
            format!("完成 {} 个文件，{} 个未处理", outputs.len(), failures.len())
        }
    } else if failures.is_empty() {
        "没有可处理的文件".to_string()
    } else {
        failures.join("\n")
    };

    Ok(DonePayload {
        ok,
        message,
        outdir: outdir.to_string_lossy().to_string(),
        outputs,
    })
}
