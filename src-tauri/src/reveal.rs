// 资源管理器定位：打开目录并选中指定文件。
// 统一走 SHOpenFolderAndSelectItems，不拼 explorer /select 命令行：
// Rust 的 Command::arg 会按 MSVC 规则重新转义参数，Explorer 解析失败会退回默认目录。

use std::path::Path;

fn tr(zh: &str, en: &str) -> String {
    crate::i18n::t(zh, en)
}

/// 打开 `folder`，并选中 `items` 中位于该目录下的文件。
/// 所有文件都不存在或不在同一目录时，退化为只打开目录。
pub fn open_folder_selecting(folder: &Path, items: &[String]) -> Result<(), String> {
    let mut folder = folder.to_path_buf();
    let mut picked: Vec<String> = Vec::new();
    for item in items {
        let path = Path::new(item);
        if !path.is_file() {
            continue;
        }
        let Some(parent) = path.parent() else {
            continue;
        };
        if picked.is_empty() {
            folder = parent.to_path_buf();
        }
        // 多选要求同一个目录；跨目录的忽略，至少不破坏定位
        if parent == folder {
            picked.push(item.clone());
        }
    }
    if picked.is_empty() {
        return open_folder(&folder);
    }
    #[cfg(target_os = "windows")]
    {
        if shell_open_folder_and_select(&folder, &picked).is_ok() {
            return Ok(());
        }
    }
    // 兜底：选中第一个文件
    explorer_select(&picked[0])
}

#[cfg(target_os = "windows")]
fn shell_open_folder_and_select(folder: &Path, items: &[String]) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        ILFindLastID, ILFree, SHOpenFolderAndSelectItems, SHParseDisplayName,
    };

    unsafe fn parse_pidl(path: &str) -> Option<*mut ITEMIDLIST> {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).is_ok()
            && !pidl.is_null()
        {
            Some(pidl)
        } else {
            None
        }
    }

    unsafe {
        // SHOpenFolderAndSelectItems 需要已初始化 COM
        let com_ready = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();

        let mut ok = false;
        if items.len() == 1 {
            // 单个文件：直接把完整 PIDL 交给 API（cidl=0 时它打开父目录并选中该项）
            if let Some(pidl) = parse_pidl(&items[0]) {
                ok = SHOpenFolderAndSelectItems(pidl, None, 0).is_ok();
                let _ = ILFree(Some(pidl as *const _));
            }
        } else {
            // 多个文件：folder PIDL + 每个文件的子 PIDL（apidl 要求是相对 folder 的项）
            let folder_pidl = parse_pidl(&folder.to_string_lossy());
            let mut full: Vec<*mut ITEMIDLIST> = Vec::new();
            let mut kids: Vec<*const ITEMIDLIST> = Vec::new();
            for item in items {
                if let Some(pidl) = parse_pidl(item) {
                    kids.push(ILFindLastID(pidl) as *const _);
                    full.push(pidl);
                }
            }
            if let Some(folder_pidl) = folder_pidl {
                if !kids.is_empty() {
                    ok = SHOpenFolderAndSelectItems(folder_pidl, Some(kids.as_slice()), 0).is_ok();
                }
                let _ = ILFree(Some(folder_pidl as *const _));
            }
            for pidl in full {
                let _ = ILFree(Some(pidl as *const _));
            }
        }

        if com_ready {
            CoUninitialize();
        }

        if ok {
            Ok(())
        } else {
            Err(tr("定位失败", "Failed to reveal"))
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn shell_open_folder_and_select(_folder: &Path, _items: &[String]) -> Result<(), String> {
    Err(tr("仅支持 Windows", "Windows only"))
}

fn open_folder(folder: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .raw_arg(format!("\"{}\"", folder.to_string_lossy()))
            .spawn()
            .map_err(|e| {
                format!(
                    "{}{}",
                    tr("打开目录失败: ", "Failed to open the folder: "),
                    e
                )
            })?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = folder;
        Err(tr("仅支持 Windows", "Windows only"))
    }
}

#[cfg(target_os = "windows")]
fn explorer_select(path: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("explorer")
        .raw_arg(format!("/select,\"{}\"", path))
        .spawn()
        .map_err(|e| {
            format!(
                "{}{}",
                tr("打开目录失败: ", "Failed to open the folder: "),
                e
            )
        })?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn explorer_select(_path: &str) -> Result<(), String> {
    Err(tr("仅支持 Windows", "Windows only"))
}
