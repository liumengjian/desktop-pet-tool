//! 文件占用检测与强制解除（Windows Restart Manager + TerminateProcess）。
//!
//! 「文件粉碎」的“解除占用”靠两层：
//! 1. `RmGetList` 找出到底是谁占着这些文件（进程名 + PID）；
//! 2. 对占用进程发 `TerminateProcess`（跳过系统关键进程与本进程自己），再重试删除。
//!
//! 系统进程（lsass / svchost / explorer 等）一律不动：杀它们会让桌面或系统直接出问题，
//! 这类文件留着走「重启后删除」的兜底更稳妥。

#[derive(Clone, Debug)]
pub struct Holder {
    pub pid: u32,
    pub name: String,
}

/// 绝对不能强制结束的进程（小写比较）。
/// 只列真杀了会出事的系统进程；普通应用（Office、浏览器、压缩软件…）不在此列。
const PROTECTED: &[&str] = &[
    "system",
    "system idle process",
    "registry",
    "memory compression",
    "secure system",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "svchost.exe",
    "dwm.exe",
    "explorer.exe",
    "fontdrvhost.exe",
    "sihost.exe",
    "ctfmon.exe",
    "taskhostw.exe",
    "audiodg.exe",
    "wudfhost.exe",
    "dpt.exe",
];

/// 谁在占用这些文件。
#[cfg(target_os = "windows")]
pub fn holders(paths: &[String]) -> Vec<Holder> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::System::RestartManager::{
        RmEndSession, RmGetList, RmRegisterResources, RmStartSession, RM_PROCESS_INFO,
        CCH_RM_SESSION_KEY,
    };

    if paths.is_empty() {
        return Vec::new();
    }

    let mut session: u32 = 0;
    // 会话键：CCH_RM_SESSION_KEY 个字符 + 结尾 0
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    let rc = unsafe { RmStartSession(&mut session, None, PWSTR(key.as_mut_ptr())) };
    if rc.0 != 0 {
        return Vec::new();
    }

    let wide_paths: Vec<Vec<u16>> = paths
        .iter()
        .map(|p| p.encode_utf16().chain(std::iter::once(0)).collect())
        .collect();
    let refs: Vec<PCWSTR> = wide_paths.iter().map(|w| PCWSTR(w.as_ptr())).collect();
    let rc = unsafe { RmRegisterResources(session, Some(&refs), None, None) };
    if rc.0 != 0 {
        unsafe { let _ = RmEndSession(session); }
        return Vec::new();
    }

    let mut needed: u32 = 0;
    let mut count: u32 = 0;
    let mut reasons: u32 = 0;
    // 第一次只问需要多大缓冲（返回 ERROR_MORE_DATA 是正常的）
    unsafe {
        let _ = RmGetList(session, &mut needed, &mut count, None, &mut reasons);
    }
    let mut out = Vec::new();
    if needed > 0 {
        let mut infos = vec![RM_PROCESS_INFO::default(); needed as usize];
        count = needed;
        let rc2 = unsafe {
            RmGetList(
                session,
                &mut needed,
                &mut count,
                Some(infos.as_mut_ptr()),
                &mut reasons,
            )
        };
        if rc2.0 == 0 {
            infos.truncate(count.min(needed) as usize);
            for info in infos {
                let raw = &info.strAppName;
                let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
                let name = String::from_utf16_lossy(&raw[..len]);
                out.push(Holder {
                    pid: info.Process.dwProcessId,
                    name,
                });
            }
        }
        unsafe { let _ = RmEndSession(session); }
        return out;
    }
    unsafe { let _ = RmEndSession(session); }
    out
}

#[cfg(not(target_os = "windows"))]
pub fn holders(_paths: &[String]) -> Vec<Holder> {
    Vec::new()
}

/// 结束占用进程，返回被结束的进程名（去重）。
#[cfg(target_os = "windows")]
pub fn release(paths: &[String]) -> Vec<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_TERMINATE, PROCESS_SYNCHRONIZE,
    };

    let self_pid = std::process::id();
    let mut killed: Vec<String> = Vec::new();
    for holder in holders(paths) {
        if holder.pid == 0 || holder.pid == self_pid {
            continue;
        }
        let lower = holder.name.to_ascii_lowercase();
        if PROTECTED.contains(&lower.as_str()) {
            continue;
        }
        // 同一进程可能被多个文件重复报上来，结束过一次就跳过
        if killed.iter().any(|n| n.eq_ignore_ascii_case(&holder.name)) {
            continue;
        }
        unsafe {
            if let Ok(handle) = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, holder.pid)
            {
                if TerminateProcess(handle, 1).is_ok() {
                    // 等一下让它真正退出、句柄释放；超时也不阻塞流程
                    let _ = WaitForSingleObject(handle, 800);
                    killed.push(holder.name);
                }
                let _ = CloseHandle(handle);
            }
        }
    }
    killed
}

#[cfg(not(target_os = "windows"))]
pub fn release(_paths: &[String]) -> Vec<String> {
    Vec::new()
}
