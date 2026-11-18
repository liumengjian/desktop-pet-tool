//! 单实例 + 轻量 IPC（Windows 命名互斥量 + 命名管道）。
//!
//! 资源管理器右键「文件粉碎」在**多选**时会对每个选中项各拉起一次进程
//! （静态动词默认按 Player 模型调用）。如果每个进程都建托盘和悬浮球，
//! 桌面上就会堆出一串悬浮球。这里保证只有一个主实例：
//!
//! 1. 启动先抢 `Local\com.desktop-pet-tool.float-instance` 命名互斥量；
//! 2. 抢不到说明已有实例在跑，把 `--shred <路径>` 通过命名管道转交给它，自己退出；
//! 3. 主实例在 setup 里起一个管道服务线程，收到消息就走与菜单相同的入队流程。
//!
//! 例外：`--fetch-pdf-deps`（安装期预下载依赖，见 NSIS 钩子）与 `--elevated`
//! （磁盘清理里「以管理员重启」拉起的副本）必须是独立进程，跳过单实例判断。
//!
//! 只用 std + windows crate，不引入新依赖（与项目其它模块一致）。

use std::time::{Duration, Instant};

/// 与 tauri.conf.json 的 identifier 对齐，避免与其它程序撞名字。
const OBJECT_PREFIX: &str = "com.desktop-pet-tool.float";

#[cfg(target_os = "windows")]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 本进程是否为主实例。抢不到互斥量时返回 false。
///
/// 互斥量句柄故意不关闭：进程退出时由系统回收，退出即释放单实例占用。
#[cfg(target_os = "windows")]
pub fn claim_primary() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    let name = wide(&format!("Local\\{}-instance", OBJECT_PREFIX));
    unsafe {
        match CreateMutexW(None, true, PCWSTR(name.as_ptr())) {
            // HANDLE 是 Copy 类型、没有 Drop，离开作用域也不会关闭：
            // 互斥量一直被本进程占着，直到进程退出由系统回收。
            Ok(_handle) => GetLastError() != ERROR_ALREADY_EXISTS,
            // 创建失败（极少数受限环境）时按主实例继续启动，不阻断用户使用
            Err(_) => true,
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn claim_primary() -> bool {
    true
}

/// 转交给已在运行的主实例。返回是否送达（送达后调用方应直接退出）。
#[cfg(target_os = "windows")]
pub fn forward_shred(paths: &[String]) -> bool {
    if paths.is_empty() {
        return false;
    }
    send(serde_json::json!({ "action": "shred", "paths": paths }))
}

/// 没有粉碎路径时（例如用户又从开始菜单点了一次图标）：让主实例把悬浮球叫到前台。
#[cfg(target_os = "windows")]
pub fn forward_activate() -> bool {
    send(serde_json::json!({ "action": "activate", "paths": [] }))
}

#[cfg(target_os = "windows")]
fn send(msg: serde_json::Value) -> bool {
    let bytes = match serde_json::to_vec(&msg) {
        Ok(b) => b,
        Err(_) => return false,
    };
    write_to_primary(&bytes)
}

#[cfg(target_os = "windows")]
fn write_to_primary(bytes: &[u8]) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE, OPEN_EXISTING,
    };
    use windows::Win32::System::Pipes::WaitNamedPipeW;

    let pipe = wide(&format!(r"\\.\pipe\{}-ipc", OBJECT_PREFIX));
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let opened = unsafe {
            CreateFileW(
                PCWSTR(pipe.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        };
        if let Ok(handle) = opened {
            let mut written = 0u32;
            let ok = unsafe { WriteFile(handle, Some(bytes), Some(&mut written), None) }.is_ok();
            unsafe {
                let _ = CloseHandle(handle);
            }
            return ok;
        }
        if Instant::now() >= deadline {
            return false;
        }
        // 主实例可能正忙在其它连接上，等一个可用实例后重试
        unsafe {
            let _ = WaitNamedPipeW(PCWSTR(pipe.as_ptr()), 500);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[cfg(not(target_os = "windows"))]
pub fn forward_shred(_paths: &[String]) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn forward_activate() -> bool {
    false
}

/// 主实例侧：常驻线程接收其它实例转交的粉碎请求。
#[cfg(target_os = "windows")]
pub fn start_server(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::Storage::FileSystem::{ReadFile, PIPE_ACCESS_INBOUND};
        use windows::Win32::System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
            PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        };

        let pipe = wide(&format!(r"\\.\pipe\{}-ipc", OBJECT_PREFIX));
        loop {
            let handle = unsafe {
                CreateNamedPipeW(
                    PCWSTR(pipe.as_ptr()),
                    PIPE_ACCESS_INBOUND,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    0,
                    64 * 1024,
                    0,
                    None,
                )
            };
            if handle.is_invalid() {
                // 极少见的创建失败（例如实例数打满）：歇一下再来，不能死循环烧 CPU
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }

            // 客户端可能先于本次调用连上（ERROR_PIPE_CONNECTED），忽略错误即可
            unsafe {
                let _ = ConnectNamedPipe(handle, None);
            }

            let mut buf: Vec<u8> = Vec::new();
            loop {
                let mut chunk = [0u8; 4096];
                let mut read = 0u32;
                match unsafe { ReadFile(handle, Some(&mut chunk), Some(&mut read), None) } {
                    Ok(()) if read > 0 => buf.extend_from_slice(&chunk[..read as usize]),
                    _ => break,
                }
                if buf.len() > 1024 * 1024 {
                    break;
                }
            }
            unsafe {
                let _ = CloseHandle(handle);
            }
            if buf.is_empty() {
                continue;
            }
            match serde_json::from_slice::<IpcMessage>(&buf) {
                Ok(msg) if msg.action == "shred" => {
                    crate::shred::stage_and_open(&app, msg.paths);
                }
                Ok(msg) if msg.action == "activate" => {
                    use tauri::Manager;
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
                _ => {}
            }
        }
    });
}

#[cfg(not(target_os = "windows"))]
pub fn start_server(_app: tauri::AppHandle) {}

#[derive(serde::Deserialize)]
struct IpcMessage {
    action: String,
    paths: Vec<String>,
}
