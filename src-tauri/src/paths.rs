//! 运行期数据目录：模型、onnxruntime、外置依赖等大文件。
//!
//! 优先放到应用安装目录下的 `data/`，例如安装到 `D:\tools\dpt` 时：
//! `D:\tools\dpt\data\watermark\v1\...`。
//! 只有安装目录不可写（例如装进受保护目录）时才回退到 LocalAppData。

use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// 应用安装目录下的数据根；不可写时回退到应用 LocalAppData。
pub fn install_data_root(app: &AppHandle) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let data = dir.join("data");
            if std::fs::create_dir_all(&data).is_ok() {
                return data;
            }
        }
    }
    app.path()
        .app_local_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
}
