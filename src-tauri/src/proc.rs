//! Windows 子进程窗口控制。
//!
//! 从 GUI 程序启动 `python.exe` 这类控制台程序时，Windows 默认会新建一个黑色
//! 控制台窗口。这里统一使用 `CREATE_NO_WINDOW`，保持 stdout/stderr 管道可用，
//! 但不再闪出控制台窗口。

use std::process::Command;

#[cfg(windows)]
pub fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn hide_console(_cmd: &mut Command) {}
