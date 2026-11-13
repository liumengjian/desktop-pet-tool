// 随包 Python sidecar：保留 PDF 页面图形并定位可编辑文字。
//
// 目录约定（相对随包根 binaries/pdfconv/）：
//   python/python.exe          embeddable 运行时 + Lib/site-packages
//   scripts/convert.py         转换入口（写 report.json）
//
// 退出码约定（见 scripts/convert.py）：
//   0 成功；2 扫描件/无文字层；1 其他失败。
// 本模块负责拉起进程、解析 report、校验产物；失败时向上返回原因。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Manager};

use super::sidecar_deps;

const SIDECAR_SUBDIR: &str = "binaries/pdfconv";
const PY_REL: &str = "python/python.exe";
const SCRIPT_REL: &str = "scripts/convert.py";
/// 单文件转换超时（秒）。复杂大表给足时间，超时按失败回退自研。
const TIMEOUT_SECS: u64 = 600;

#[derive(Deserialize, Default)]
struct SidecarReport {
    #[serde(default)]
    status: String,
    #[serde(default)]
    pages: i32,
    #[serde(default)]
    tables: i32,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    message: String,
}

/// 定位随包 sidecar 根目录；打包后取 resource_dir，开发态从 exe 回溯工程目录。
fn resolve_root(app: &AppHandle) -> Option<PathBuf> {
    // 候选 1：打包安装后的资源目录
    if let Ok(res) = app.path().resource_dir() {
        let cand = res.join(SIDECAR_SUBDIR);
        if cand.join(PY_REL).exists() {
            return Some(cand);
        }
    }
    // 候选 2：开发态从可执行文件目录向上回溯，找 src-tauri/binaries/pdfconv
    if let Ok(exe) = std::env::current_exe() {
        let mut cur: Option<&Path> = exe.parent();
        while let Some(p) = cur {
            let cand = p.join("src-tauri").join(SIDECAR_SUBDIR);
            if cand.join(PY_REL).exists() {
                return Some(cand);
            }
            let cand2 = p.join(SIDECAR_SUBDIR);
            if cand2.join(PY_REL).exists() {
                return Some(cand2);
            }
            cur = p.parent();
        }
    }
    None
}

fn read_report(path: &Path) -> SidecarReport {
    let Ok(bytes) = std::fs::read(path) else {
        return SidecarReport::default();
    };
    serde_json::from_slice::<SidecarReport>(&bytes).unwrap_or_default()
}

/// 调起 sidecar 转换。Ok 表示产物可用；Err 携带原因（含已取消、扫描件、启动失败等），
/// 由上层决定是否回退自研链路。
pub fn convert(
    app: &AppHandle,
    src: &Path,
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
    note: &mut Option<String>,
) -> Result<(), String> {
    let root = resolve_root(app).ok_or_else(|| "内置转换引擎未随包安装".to_string())?;
    // 外置 site-packages：缺失则从 OSS 下载并校验到用户数据目录；失败交上层回退自研。
    let site_pkgs = sidecar_deps::ensure(app).map_err(|e| format!("转换引擎依赖未就绪: {}", e))?;
    let py = root.join(PY_REL);
    let script = root.join(SCRIPT_REL);
    let report = dst.with_file_name(format!(
        "{}.sidecar-report.json",
        dst.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default()
    ));

    let mut cmd = Command::new(py);
    cmd.current_dir(root.join("python"));
    cmd.arg(&script)
        .arg("--src")
        .arg(src)
        .arg("--dst")
        .arg(dst)
        .arg("--report")
        .arg(&report);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd.env("PYTHONIOENCODING", "utf-8");
    // 外置依赖目录通过环境变量传给脚本，由 convert.py 插入 sys.path
    cmd.env("PDFCONV_SITE_PACKAGES", &site_pkgs);
    // 不继承本机 PYTHONPATH，避免污染随包解释器
    cmd.env_remove("PYTHONPATH");

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动内置转换引擎失败: {}", e))?;

    let start = Instant::now();
    let status = loop {
        if cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("已取消".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(TIMEOUT_SECS) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("内置转换引擎超时".to_string());
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => return Err(format!("等待转换进程失败: {}", e)),
        }
    };

    let code = status.code().unwrap_or(-1);
    let rep = read_report(&report);
    let _ = std::fs::remove_file(&report);

    // 退出码 0 且 report.status=ok，再校验产物非空
    if code == 0 && rep.status == "ok" {
        let size = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
        if size == 0 {
            return Err("内置引擎产物为空".to_string());
        }
        let mut parts = Vec::new();
        if rep.pages > 0 {
            parts.push(format!("{} 页", rep.pages));
        }
        if rep.tables > 0 {
            parts.push(format!("{} 个表格", rep.tables));
        }
        let warn: Vec<String> = rep
            .warnings
            .into_iter()
            .filter(|w| !w.is_empty())
            .collect();
        let mut text = parts.join("，");
        if !warn.is_empty() {
            text = format!("{}（{}）", text, warn.join("；"));
        }
        if !text.is_empty() {
            *note = Some(text);
        }
        return Ok(());
    }

    // code 2 = 扫描件；code 1 / 其他 = 失败。统一交上层回退自研链路。
    let reason = match code {
        2 => "扫描件无可提取文字".to_string(),
        _ if !rep.message.is_empty() => rep.message,
        _ => format!("内置引擎退出码 {}", code),
    };
    Err(reason)
}
