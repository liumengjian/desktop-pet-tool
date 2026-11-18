// 人脸模型的本地缓存与下发。
//
// 选型结论：只比对 1:1，不需要 3D 关键点 / 106 点 / 年龄性别，因此只要两个文件：
//   det_10g.onnx   SCRFD-10GF 检测器          ≈ 17MB
//   w600k_r50.onnx ResNet50@WebFace600K 识别  ≈ 170MB
// 整包 buffalo_l 是 326MB，其中 137MB 用不上，不下发。
//
// 下发沿用「依赖外置」那条已跑通的链路：腾讯云 COS 公开直链 + manifest 里的
// sha256 白名单 + 安装目录 data 缓存 + marker。首次使用时才下载，不放进安装包。
//
// 缓存布局（应用安装目录/data 下，例如 D:\tools\dpt\data）：
//   face/<TAG>/models/det_10g.onnx
//   face/<TAG>/models/w600k_r50.onnx
//   face/<TAG>/deps/onnxruntime/...     onnxruntime 独立叠加目录（不动 pdfconv 的 site-packages）
//   face/<TAG>/marker.json
//
// 也支持「手工放置」：把两个 onnx 依次拷进 models 目录后直接可用，不再走网络。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::AppHandle;

/// 模型目录标签：换模型 / 换运行时就换新标签，旧缓存不混用。
const TAG: &str = "v1";
/// COS 上的模型目录（公开读直链，无签名）。
const BASE_URL: &str =
    "https://prina-1413661498.cos.ap-shanghai.myqcloud.com/tools/desktop-pet-tool/face-models/pack";

const DET_NAME: &str = "det_10g.onnx";
const REC_NAME: &str = "w600k_r50.onnx";
const DEPS_ZIP: &str = "face-deps-v1.zip";

/// 单个文件下载超时：170MB 在 0.5MB/s 的弱网下也要能下完。
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
    #[serde(default)]
    deps: Option<FileSpec>,
}

#[derive(Serialize, Deserialize)]
struct Marker {
    tag: String,
    at: u64,
}

/// 就绪后的三个路径：检测模型、识别模型、onnxruntime 依赖目录。
pub struct Models {
    pub det: PathBuf,
    pub rec: PathBuf,
    pub deps: PathBuf,
}

fn cache_root(app: &AppHandle) -> PathBuf {
    crate::paths::install_data_root(app)
        .join("face")
        .join(TAG)
}

fn models_dir(app: &AppHandle) -> PathBuf {
    cache_root(app).join("models")
}

fn deps_dir(app: &AppHandle) -> PathBuf {
    cache_root(app).join("deps")
}

fn is_file_ok(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.len() > 1_000_000).unwrap_or(false)
}

fn deps_ready(dir: &Path) -> bool {
    dir.join("onnxruntime").join("__init__.py").exists()
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

async fn fetch_manifest() -> Result<Manifest, String> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 desktop-pet-tool/1.0")
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("初始化下载失败: {}", e))?;
    let url = format!("{}/manifest.json", BASE_URL);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("拉取模型清单失败: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("拉取模型清单失败，HTTP {}", resp.status().as_u16()));
    }
    // reqwest 未启用 json 特性，这里手动解析，避免为一个清单多拉一个 feature
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取模型清单失败: {}", e))?;
    serde_json::from_str::<Manifest>(&text).map_err(|e| format!("模型清单格式错误: {}", e))
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
    let det = models_dir(app).join(DET_NAME);
    let rec = models_dir(app).join(REC_NAME);
    let deps = deps_dir(app);

    // 1. 本地已经齐了（手工放置或上次下载完成）：完全不碰网络
    if is_file_ok(&det) && is_file_ok(&rec) && deps_ready(&deps) {
        on_progress(100.0, "模型就绪");
        return Ok(Models { det, rec, deps });
    }

    let root = cache_root(app);
    std::fs::create_dir_all(models_dir(app)).map_err(|e| format!("创建缓存目录失败: {}", e))?;

    // 2. 取清单
    on_progress(0.0, "检查模型");
    let manifest = match tauri::async_runtime::block_on(fetch_manifest()) {
        Ok(m) => m,
        Err(e) => {
            // 本地文件都在、只缺 onnxruntime 时给出明确的修复指引，而不是一句网络错误
            if is_file_ok(&det) && is_file_ok(&rec) {
                return Err(format!(
                    "{}；模型文件已在本地，但 onnxruntime 依赖目录缺失：{}。请重新联网下载依赖，或把依赖包解压到该目录。",
                    e,
                    deps.display()
                ));
            }
            return Err(format!(
                "{}。首次使用需要联网下载约 190MB 模型；也可以手动把 {} / {} 放进 {} 后重试。",
                e,
                DET_NAME,
                REC_NAME,
                models_dir(app).display()
            ));
        }
    };

    let spec = |name: &str| manifest.models.iter().find(|m| m.name == name).cloned();
    let det_spec = spec(DET_NAME);
    let rec_spec = spec(REC_NAME);

    // 3. 两个模型：缺失则下载并校验
    let model_items: Vec<(PathBuf, Option<FileSpec>)> =
        vec![(det.clone(), det_spec), (rec.clone(), rec_spec)];
    for (idx, (path, sp)) in model_items.iter().enumerate() {
        if is_file_ok(path) {
            continue;
        }
        let Some(sp) = sp else {
            return Err(format!("模型清单里缺少 {}", path.file_name().unwrap_or_default().to_string_lossy()));
        };
        let url = format!("{}/{}", BASE_URL, sp.name);
        let base = if idx == 0 { 0.0 } else { 45.0 };
        let span = 45.0;
        on_progress(base as f32, &format!("下载 {}", sp.name));
        let path_ref: &Path = path;
        tauri::async_runtime::block_on(download(
            &url,
            path_ref,
            sp.bytes,
            |p| on_progress((base + span * p) as f32, &format!("下载 {}", sp.name)),
        ))?;
        let actual = sha256_file(path_ref)?;
        if !sp.sha256.is_empty() && actual != sp.sha256.to_lowercase() {
            let _ = std::fs::remove_file(path_ref);
            return Err(format!(
                "{} 校验失败（sha256 不一致），可能下载损坏，请重试",
                sp.name
            ));
        }
    }

    // 4. onnxruntime 叠加依赖
    if !deps_ready(&deps) {
        let Some(sp) = manifest.deps.clone() else {
            return Err("模型清单里缺少 onnxruntime 依赖包".to_string());
        };
        on_progress(90.0, &format!("下载 {}", sp.name));
        let zip = root.join(DEPS_ZIP);
        tauri::async_runtime::block_on(download(
            &format!("{}/{}", BASE_URL, sp.name),
            &zip,
            sp.bytes,
            |p| on_progress(90.0 + 9.0 * p, &format!("下载 {}", sp.name)),
        ))?;
        let actual = sha256_file(&zip)?;
        if !sp.sha256.is_empty() && actual != sp.sha256.to_lowercase() {
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
        if deps.exists() {
            let _ = std::fs::remove_dir_all(&deps);
        }
        std::fs::rename(&staging, &deps).map_err(|e| format!("就位依赖目录失败: {}", e))?;
        let _ = std::fs::remove_file(&zip);
    }

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
    Ok(Models { det, rec, deps })
}

/// 只查状态，不触发下载：前端用来决定要不要提示「首次使用需下载」。
pub fn status(app: &AppHandle) -> (bool, String) {
    let det = models_dir(app).join(DET_NAME);
    let rec = models_dir(app).join(REC_NAME);
    let deps = deps_dir(app);
    let ready = is_file_ok(&det) && is_file_ok(&rec) && deps_ready(&deps);
    (ready, cache_root(app).to_string_lossy().to_string())
}
