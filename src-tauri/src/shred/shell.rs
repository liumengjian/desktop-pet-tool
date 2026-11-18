//! 资源管理器右键菜单「文件粉碎」的注册。
//!
//! 键位写在 `HKCU\Software\Classes` 下：只对当前用户生效、不需要管理员权限，
//! 与安装模式（currentUser）一致。文件与文件夹各注册一个静态动词：
//!
//! ```text
//! HKCU\Software\Classes\*\shell\dptFileShred             ← 任意文件
//! HKCU\Software\Classes\Directory\shell\dptFileShred     ← 任意文件夹
//!   (默认)      = 菜单文字（跟随界面语言）
//!   Icon        = "<exe>,0"
//!   command(默认) = "<exe>" --shred "%1"
//! ```
//!
//! 应用每次启动都会调用 [`ensure_registered`]，安装目录变了（升级 / 换盘）
//! 也能自动把 exe 路径改过来；卸载时由 NSIS 钩子删除同名键。

#[cfg(target_os = "windows")]
use windows::core::PCWSTR;

/// 文件动词键（`*` 代表所有文件类型）
#[cfg(target_os = "windows")]
const FILE_VERB: &str = r"Software\Classes\*\shell\dptFileShred";
/// 文件夹动词键
#[cfg(target_os = "windows")]
const DIR_VERB: &str = r"Software\Classes\Directory\shell\dptFileShred";

#[cfg(target_os = "windows")]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// REG_SZ 的字节形式：UTF-16LE + 结尾 0
#[cfg(target_os = "windows")]
fn reg_bytes(s: &str) -> Vec<u8> {
    let mut out: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    out.extend_from_slice(&[0, 0]);
    out
}

/// 把右键动词写进注册表。失败只记录，不阻断启动。
#[cfg(target_os = "windows")]
pub fn ensure_registered() -> Result<(), String> {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let exe = std::env::current_exe()
        .map_err(|e| format!("取程序路径失败: {e}"))?
        .to_string_lossy()
        .into_owned();
    // 命令行里的 exe 路径统一加引号，防止安装目录带空格
    let command = format!("\"{exe}\" --shred \"%1\"");
    let icon = format!("{exe},0");

    unsafe fn write_value(
        key: &windows::Win32::System::Registry::HKEY,
        name: Option<&str>,
        text: &str,
    ) -> u32 {
        let name_w = name.map(wide);
        let name_ptr = match &name_w {
            Some(v) => PCWSTR(v.as_ptr()),
            None => PCWSTR::null(),
        };
        let bytes = reg_bytes(text);
        unsafe {
            RegSetValueExW(*key, name_ptr, None, REG_SZ, Some(bytes.as_slice())).0
        }
    }

    unsafe {
        let mut root = HKEY::default();
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wide(FILE_VERB).as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut root,
            None,
        );
        if rc.0 != 0 {
            return Err(format!("创建文件动词键失败（错误码 {}）", rc.0));
        }
        let mut cmd = HKEY::default();
        let sub = wide(&format!("{FILE_VERB}\\command"));
        let rc2 = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut cmd,
            None,
        );
        if rc2.0 != 0 {
            let _ = RegCloseKey(root);
            return Err(format!("创建文件命令子键失败（错误码 {}）", rc2.0));
        }
        let label = crate::i18n::t("文件粉碎", "Shred file");
        let code = write_value(&root, None, &label)
            | write_value(&root, Some("Icon"), &icon)
            // 多选时按项各拉起一次进程，由单实例 IPC 合并（见 single.rs）
            | write_value(&root, Some("MultiSelectModel"), "Player")
            | write_value(&cmd, None, &command);
        let _ = RegCloseKey(cmd);
        let _ = RegCloseKey(root);
        if code != 0 {
            return Err(format!("写入文件动词失败（错误码 {code}）"));
        }

        let mut d_root = HKEY::default();
        let rc3 = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wide(DIR_VERB).as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut d_root,
            None,
        );
        if rc3.0 != 0 {
            return Err(format!("创建文件夹动词键失败（错误码 {}）", rc3.0));
        }
        let mut d_cmd = HKEY::default();
        let d_sub = wide(&format!("{DIR_VERB}\\command"));
        let rc4 = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(d_sub.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut d_cmd,
            None,
        );
        if rc4.0 != 0 {
            let _ = RegCloseKey(d_root);
            return Err(format!("创建文件夹命令子键失败（错误码 {}）", rc4.0));
        }
        let d_label = crate::i18n::t("文件粉碎", "Shred folder");
        let d_code = write_value(&d_root, None, &d_label)
            | write_value(&d_root, Some("Icon"), &icon)
            | write_value(&d_root, Some("MultiSelectModel"), "Player")
            | write_value(&d_cmd, None, &command);
        let _ = RegCloseKey(d_cmd);
        let _ = RegCloseKey(d_root);
        if d_code != 0 {
            return Err(format!("写入文件夹动词失败（错误码 {d_code}）"));
        }
    }

    notify_shell();
    Ok(())
}

/// 通知资源管理器刷新关联 / 右键菜单，不用重启 explorer 就能看到新项。
#[cfg(target_os = "windows")]
fn notify_shell() {
    use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

#[cfg(not(target_os = "windows"))]
pub fn ensure_registered() -> Result<(), String> {
    Err("仅支持 Windows".to_string())
}
