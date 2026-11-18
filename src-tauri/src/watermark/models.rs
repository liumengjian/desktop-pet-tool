// 去水印修复模型的本地缓存与下发。
//
// 选型结论：效果优先，采用 LaMa 修复模型（ONNX，约 88MB）。模型只在 Python 侧
// 通过 onnxruntime 加载，主进程不常驻推理。
//
// 下发沿用「依赖外置」那条已跑通的链路：腾讯云 COS 公开直链 + 本地内置 manifest
// + 安装目录 data 缓存 + marker。首次使用时才下载模型，不放进安装包。
//
// manifest.json 只有 1KB，随 exe 编译（见 MANIFEST_JSON），不再单独上传 COS；
// COS 上只保留大模型 inpainting_lama.onnx。
//
// 缓存布局（应用安装目录/data 下，例如 D:\tools\dpt\data）：
//   watermark/<TAG>/models/inpainting_lama.onnx
//   watermark/<TAG>/deps/onnxruntime/...   onnxruntime 叠加目录（按需回退下载）
//   watermark/<TAG>/marker.json
//
// onnxruntime 与「人脸比对」是同一份运行时：优先复用 face/<TAG>/deps，避免重复下载；
// 仅当人脸那套依赖不在本地时，才把同一份 ort 包解压到本模块自己的 deps 目录。

// 也支持「手工放置」：把 onnx 拷进 models 目录后直接可用，不再走网络。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::AppHandle;

/// 模型目录标签：换模型 / 换运行时就换新标签，旧缓存不混用。
const TAG: &str = "v1";
/// COS 上的模型目录（公开读直链，无签名）。
const BASE_URL: &str =
    "https://prina-1413661498.cos.ap-shanghai.myqcloud.com/tools/desktop-pet-tool/watermark-models/pack";

const MODEL_NAME: &str = "inpainting_lama.onnx";

/// 模型清单内置在 exe 里，不再向 COS 请求 manifest.json。
/// 换模型时改这个文件并重新编译，COS 只需要上传新的 onnx。
const MANIFEST_JSON: &str = include_str!("../../../deploy/watermark-models/manifest.json");

/// onnxruntime 包与「人脸比对」共用同一份压缩包，常量记录其清单，供回退下载时校验。
const DEPS_ZIP: &str = "face-deps-v1.zip";
const DEPS_URL: &str =
    "https://prina-1413661498.cos.ap-shanghai.myqcloud.com/tools/desktop-pet-tool/face-models/pack/face-deps-v1.zip";
const DEPS_SHA256: &str = "2d97a7ed24da07f879563eb24885c7840c46b6f6c19be8f4da134e1836d67b42";
const DEPS_BYTES: u64 = 17770995;

/// 单个文件下载超时：88MB 在弱网下也要能下完。
const DOWNLOAD_TIMEOUT_SECS: u64 = 900;

#[derive(Serialize, Deserialize, Clone)]
struct FileSpec {
    name: String,
    sha256: String,
    bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    models: Vec<FileSpec>,
}

#[derive(Serialize, Deserialize)]
struct Marker {
    tag: String,
    at: u64,
}

/// 就绪后的两个路径：修复模型、onnxruntime 依赖目录。
pub struct Models {
    pub model: PathBuf,
    pub deps: PathBuf,
}

fn cache_root(app: &AppHandle) -> PathBuf {
    crate::paths::install_data_root(app)
        .join("watermark")
        .join(TAG)
}

fn models_dir(app: &AppHandle) -> PathBuf {
    cache_root(app).join("models")
}

fn deps_dir(app: &AppHandle) -> PathBuf {
    cache_root(app).join("deps")
}

/// 人脸模块的 ort 依赖目录：优先复用，不重复下载。
fn face_deps_dir(app: &AppHandle) -> PathBuf {
    crate::paths::install_data_root(app)
        .join("face")
        .join("v1")
        .join("deps")
}

fn is_model_ok(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.len() > 1_000_000).unwrap_or(false)
}

fn deps_ready(dir: &Path) -> bool {
    dir.join("onnxruntime").join("__init__.py").exists()
}

/// 已就位的 ort 依赖目录：先复用人脸那套，其次本模块自己的。
fn existing_deps(app: &AppHandle) -> Option<PathBuf> {
    let face = face_deps_dir(app);
    if deps_ready(&face) {
        return Some(face);
    }
    let own = deps_dir(app);
    if deps_ready(&own) {
        return Some(own);
    }
    None
}

fn sha256_file(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("读取文件失败: {}", e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("读取文件失败: {}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn local_manifest() -> Result<Manifest, String> {
    serde_json::from_str::<Manifest>(MANIFEST_JSON)
        .map_err(|e| format!("内置模型清单格式错误: {}", e))
}

/// 流式下载并按已收字节回报进度（0.0–1.0 的本文件进度）。
async fn download(url: &str, dst: &Path, total: u64, mut on: impl FnMut(f32)) -> Result<(), String> {
    use std::io::Write;

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 desktop-pet-tool/1.0")
        .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("初始化下载失败: {}", e))?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载失败: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败，HTTP {}", resp.status().as_u16()));
    }

    let part = dst.with_extension("part");
    let _ = std::fs::remove_file(&part);
    if let Some(parent) = part.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
    }
    let mut file = std::fs::File::create(&part).map_err(|e| format!("创建临时文件失败: {}", e))?;
    let expect = total.max(1);
    let mut got: u64 = 0;
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("下载中断: {}", e))? {
        file.write_all(&chunk)
            .map_err(|e| format!("写入文件失败: {}", e))?;
        got += chunk.len() as u64;
        on(((got as f32) / (expect as f32)).min(1.0));
    }
    drop(file);
    if got == 0 {
        let _ = std::fs::remove_file(&part);
        return Err("下载内容为空".to_string());
    }
    std::fs::rename(&part, dst).map_err(|e| format!("落盘失败: {}", e))?;
    Ok(())
}

fn unzip_to(zip: &Path, dst: &Path) -> Result<(), String> {
    let entries = crate::docs::zip_min::read_zip(zip)?;
    std::fs::create_dir_all(dst).map_err(|e| format!("创建目录失败: {}", e))?;
    for entry in &entries {
        let rel: PathBuf = entry
            .name
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .collect();
        if rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            continue;
        }
        let target = dst.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| format!("创建目录失败: {}", e))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
        }
        std::fs::write(&target, entry.bytes()?).map_err(|e| format!("解压文件失败: {}", e))?;
    }
    Ok(())
}

/// 确保模型与 onnxruntime 就绪。`on_progress` 回调拿到的百分比是整条链路（0–100）。
pub fn ensure(app: &AppHandle, on_progress: &dyn Fn(f32, &str)) -> Result<Models, String> {
    let model = models_dir(app).join(MODEL_NAME);

    // 1. 本地已经齐了（手工放置或上次下载完成）：完全不碰网络
    if is_model_ok(&model) {
        if let Some(deps) = existing_deps(app) {
            on_progress(100.0, "模型就绪");
            return Ok(Models { model, deps });
        }
    }

    let root = cache_root(app);
    std::fs::create_dir_all(models_dir(app)).map_err(|e| format!("创建缓存目录失败: {}", e))?;

    // 2. 读取内置清单（不再请求 COS manifest）
    on_progress(0.0, "检查模型");
    let manifest = local_manifest()?;

    // 3. 修复模型：缺失则下载并校验
    if !is_model_ok(&model) {
        let Some(sp) = manifest.models.iter().find(|m| m.name == MODEL_NAME).cloned() else {
            return Err(format!("模型清单里缺少 {}", MODEL_NAME));
        };
        let url = format!("{}/{}", BASE_URL, sp.name);
        on_progress(0.0, &format!("下载 {}", sp.name));
        let model_ref: &Path = &model;
        tauri::async_runtime::block_on(download(
            &url,
            model_ref,
            sp.bytes,
            |p| on_progress((p * 70.0) as f32, &format!("下载 {}", sp.name)),
        ))?;
        let actual = sha256_file(model_ref)?;
        if !sp.sha256.is_empty() && actual != sp.sha256.to_lowercase() {
            let _ = std::fs::remove_file(model_ref);
            return Err(format!(
                "{} 校验失败（sha256 不一致），可能下载损坏，请重试",
                sp.name
            ));
        }
    }

    // 4. onnxruntime：先复用人脸那套；都不在本地才回退下载同一份 ort 包。
    let deps = match existing_deps(app) {
        Some(d) => d,
        None => {
            on_progress(72.0, "下载运行时依赖");
            let zip = root.join(DEPS_ZIP);
            tauri::async_runtime::block_on(download(
                DEPS_URL,
                &zip,
                DEPS_BYTES,
                |p| on_progress(72.0 + 18.0 * p, DEPS_ZIP),
            ))?;
            let actual = sha256_file(&zip)?;
            if actual != DEPS_SHA256 {
                let _ = std::fs::remove_file(&zip);
                return Err("依赖包校验失败（sha256 不一致），请重试".to_string());
            }
            let staging = root.join("deps-stage");
            let _ = std::fs::remove_dir_all(&staging);
            unzip_to(&zip, &staging)?;
            if !deps_ready(&staging) {
                let _ = std::fs::remove_dir_all(&staging);
                let _ = std::fs::remove_file(&zip);
                return Err("依赖包解压后缺少 onnxruntime".to_string());
            }
            let own = deps_dir(app);
            if own.exists() {
                let _ = std::fs::remove_dir_all(&own);
            }
            std::fs::rename(&staging, &own).map_err(|e| format!("就位依赖目录失败: {}", e))?;
            let _ = std::fs::remove_file(&zip);
            own
        }
    };

    // 5. 写 marker（存在即可，用于排查缓存版本）
    let marker = Marker {
        tag: TAG.to_string(),
        at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    let _ = std::fs::write(
        root.join("marker.json"),
        serde_json::to_vec_pretty(&marker).unwrap_or_default(),
    );

    on_progress(100.0, "模型就绪");
    Ok(Models { model, deps })
}

/// 只查状态，不触发下载：前端用来决定要不要提示「首次使用需下载」。
pub fn status(app: &AppHandle) -> (bool, String) {
    let model = models_dir(app).join(MODEL_NAME);
    let ready = is_model_ok(&model) && existing_deps(app).is_some();
    (ready, cache_root(app).to_string_lossy().to_string())
}
