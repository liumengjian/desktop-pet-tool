// 外置 Python 依赖（site-packages）的下载、校验、解压与本地缓存。
//
// 方案 B：安装时由引导器优先拉取；失败 / 离线则首次转换时补下载；再失败回退自研链路。
// 缓存按版本标签隔离到用户可写数据目录，不写安装目录；下载失败只清理半成品，
// 不污染已提交的可用缓存。
//
// 缓存布局（LocalAppData 下，随 bundleid 目录在卸载时一并清理）：
//   pdfconv/<VERSION>/site-packages/   解压后的依赖目录
//   pdfconv/<VERSION>/marker.json     版本 + 哈希标记，用于判断缓存是否可用
//   pdfconv/<VERSION>/download.zip     下载临时包（提交后删除）

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use super::zip_min;

/// 依赖包下载地址（腾讯云 COS，公开直链）。
const DEPS_URL: &str =
    "https://prina-1413661498.cos.ap-shanghai.myqcloud.com/tools/desktop-pet-tool/v0.5.13-1/site-packages.zip";
/// 依赖版本标签：同时用作缓存目录名；升级依赖就换新标签，旧缓存不混用。
const DEPS_VERSION: &str = "v0.5.13-1";
/// 依赖包 SHA-256（小写十六进制），下载后逐字节比对，防损坏 / 被替换。
const DEPS_SHA256: &str =
    "76f345772198a6a86ec7cc171d33adcf5e95c2105c0a54ff8c8133878ee4b012";

/// 解压后必须存在的关键路径（相对 site-packages 根）。目录项按前缀判断，
/// 文件项按精确路径判断；任一缺失即视为缓存不完整，重新下载。
const REQUIRED_PATHS: &[&str] = &[
    "fitz/__init__.py",
    "cv2/cv2.pyd",
    "pymupdf/_mupdf.pyd",
    "pymupdf/mupdfcpp64.dll",
    "numpy/__init__.py",
    "numpy.libs",
    "docx/__init__.py",
    "pdf2docx/converter.py",
    "lxml/etree.cp313-win_amd64.pyd",
    "fontTools/__init__.py",
];

#[derive(Serialize, Deserialize)]
struct Marker {
    version: String,
    sha256: String,
    at: u64,
}

fn cache_root(app: &AppHandle) -> PathBuf {
    // 放 LocalAppData：体积大且无需随账号漫游；卸载时安装器会清理整个 bundleid 目录。
    app.path()
        .app_local_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("pdfconv")
        .join(DEPS_VERSION)
}

fn site_dir(app: &AppHandle) -> PathBuf {
    cache_root(app).join("site-packages")
}

fn marker_path(app: &AppHandle) -> PathBuf {
    cache_root(app).join("marker.json")
}

/// 关键路径是否齐全。
fn dir_ready(dir: &Path) -> bool {
    REQUIRED_PATHS.iter().all(|rel| dir.join(rel).exists())
}

/// 已提交缓存是否可用：标记哈希匹配且关键文件齐全。
fn cache_usable(app: &AppHandle) -> bool {
    let dir = site_dir(app);
    if !dir_ready(&dir) {
        return false;
    }
    let Ok(bytes) = std::fs::read(marker_path(app)) else {
        return false;
    };
    let Ok(marker) = serde_json::from_slice::<Marker>(&bytes) else {
        return false;
    };
    marker.version == DEPS_VERSION && marker.sha256 == DEPS_SHA256
}

/// 校验条目路径安全：拒绝 `..` 逃逸，统一用 `/` 拆分。
fn safe_join(base: &Path, name: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = name.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    let joined: PathBuf = parts.iter().collect();
    if joined
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        return None;
    }
    Some(base.join(joined))
}

async fn download_bytes() -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 desktop-pet-tool/1.0")
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| format!("初始化下载失败: {}", e))?;
    let resp = client
        .get(DEPS_URL)
        .send()
        .await
        .map_err(|e| format!("下载依赖失败: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("下载依赖失败，HTTP {}", resp.status().as_u16()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取依赖包失败: {}", e))?;
    Ok(bytes.to_vec())
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// 确保外置依赖已就绪，返回可用的 site-packages 目录。
///
/// 已存在有效缓存则直接返回；否则下载 → 校验哈希 → 解压到暂存目录 → 校验关键文件
/// → 原子替换为正式目录 → 写标记。任一阶段失败都会清理半成品并返回 Err，
/// 由上层回退自研链路。
pub fn ensure(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = site_dir(app);
    if cache_usable(app) {
        return Ok(dir);
    }

    let root = cache_root(app);
    std::fs::create_dir_all(&root).map_err(|e| format!("创建缓存目录失败: {}", e))?;

    let zip_path = root.join("download.zip");
    let staging = root.join("stage");

    // 清理上一轮失败残留
    let _ = std::fs::remove_file(&zip_path);
    let _ = std::fs::remove_dir_all(&staging);

    // 1. 下载
    let bytes = tauri::async_runtime::block_on(download_bytes())?;
    if bytes.is_empty() {
        let _ = std::fs::remove_file(&zip_path);
        return Err("依赖包为空".to_string());
    }

    // 2. 哈希校验
    let actual = sha256_hex(&bytes);
    if actual != DEPS_SHA256 {
        let _ = std::fs::remove_file(&zip_path);
        return Err(format!("依赖包校验哈希不一致（可能下载损坏）: {}", actual));
    }

    // 3. 落盘临时 zip 并解压到暂存目录
    std::fs::write(&zip_path, &bytes).map_err(|e| format!("写入临时包失败: {}", e))?;
    let entries = zip_min::read_zip(&zip_path)?;
    std::fs::create_dir_all(&staging).map_err(|e| format!("创建暂存目录失败: {}", e))?;

    for entry in &entries {
        let Some(target) = safe_join(&staging, &entry.name) else {
            let _ = std::fs::remove_dir_all(&staging);
            let _ = std::fs::remove_file(&zip_path);
            return Err("依赖包含非法路径条目".to_string());
        };
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| format!("创建目录失败: {}", e))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
        }
        let data = entry.bytes()?;
        std::fs::write(&target, data).map_err(|e| format!("解压文件失败: {}", e))?;
    }

    // 4. 关键文件齐全校验
    if !dir_ready(&staging) {
        let _ = std::fs::remove_dir_all(&staging);
        let _ = std::fs::remove_file(&zip_path);
        return Err("依赖包解压后缺少关键组件".to_string());
    }

    // 5. 原子替换：旧正式目录先删，再把暂存目录改名就位
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::rename(&staging, &dir).map_err(|e| {
        let _ = std::fs::remove_dir_all(&staging);
        format!("就位依赖目录失败: {}", e)
    })?;

    // 6. 写标记
    let marker = Marker {
        version: DEPS_VERSION.to_string(),
        sha256: DEPS_SHA256.to_string(),
        at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    std::fs::write(
        marker_path(app),
        serde_json::to_vec_pretty(&marker).unwrap_or_default(),
    )
    .map_err(|e| format!("写入依赖标记失败: {}", e))?;

    let _ = std::fs::remove_file(&zip_path);
    Ok(dir)
}
