#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clipboard_rs::Clipboard;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{CheckMenuItem, CheckMenuItemBuilder, Menu, MenuBuilder, MenuItem, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;
use percent_encoding::percent_decode_str;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR};
use windows::Win32::Graphics::Gdi::{
    CreateRectRgn, DeleteObject, GetMonitorInfoW, HGDIOBJ, MonitorFromPoint, SetWindowRgn,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SYSTEM_METRICS_INDEX,
};

const DRAG_DEAD_ZONE: i32 = 6;
const POLL_MS: u64 = 33;
const SETTINGS_W: f64 = 540.0;
/// settings.html 的内容实测 409px（540 宽下 body scrollHeight，含语言行），
/// 窗口内高按内容给足余量，避免按钮下方出现空白或出现滚动条。
const SETTINGS_H: f64 = 412.0;
/// 主窗口逻辑尺寸。窗口本身透明且用 SetWindowRgn 裁出可见区域，
/// 尺寸给足是为了让文档气泡（列表 / 按钮 / 结果）有足够的可视空间。
const MAIN_W: f64 = 420.0;
const MAIN_H: f64 = 620.0;

mod docs;
mod drop_target;
mod i18n;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Config {
    temp_dir: String,
    always_on_top: bool,
    auto_start: bool,
    close_to_tray: bool,
    /// 界面语言：zh（默认）| en。旧配置文件没有这个字段时按默认值补齐。
    #[serde(default = "default_lang")]
    lang: String,
}

fn default_lang() -> String {
    "zh".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            temp_dir: std::env::temp_dir()
                .join("desktop-pet-tool")
                .to_string_lossy()
                .into_owned(),
            always_on_top: true,
            // 全新安装默认开启开机自启；setup 中会据此调用 autolaunch().enable() 注册启动项
            auto_start: true,
            // 点击悬浮球右上角关闭按钮时，默认最小化到系统托盘（而非直接退出）
            close_to_tray: true,
            lang: default_lang(),
        }
    }
}

#[derive(Serialize)]
struct DownloadResult {
    ok: bool,
    file_path: String,
    filename: String,
    size: u64,
    copied_to_clipboard: bool,
}

struct AppState {
    config: Mutex<Config>,
    dragging: AtomicBool,
    visible_width: AtomicI32,
    visible_height: AtomicI32,
    drag_origin: Mutex<Option<(i32, i32, i32, i32)>>, // (start_cursor_x, start_cursor_y, win_x, win_y)
    menu: Mutex<Option<Menu<tauri::Wry>>>,
    /// 托盘菜单里各项的句柄。切换语言时只改文字（set_text），不重建菜单：
    /// Windows 上运行时 `tray.set_menu()` 会让主线程等消息泵，实测会把界面卡死。
    menu_items: Mutex<Option<MenuHandles>>,
    tray: Mutex<Option<TrayIcon<tauri::Wry>>>,
}

/// 托盘菜单各项的句柄，用于就地改文案
struct MenuHandles {
    paste: MenuItem<tauri::Wry>,
    docs: MenuItem<tauri::Wry>,
    open: MenuItem<tauri::Wry>,
    clean: MenuItem<tauri::Wry>,
    settings: MenuItem<tauri::Wry>,
    autostart: CheckMenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

impl MenuHandles {
    /// 按当前语言刷新七项文案。只调用 Win32 的菜单项文本更新，不触碰托盘宿主。
    fn apply_lang(&self) {
        let _ = self.paste.set_text(i18n::t("文档复制", "Copy documents"));
        let _ = self.docs.set_text(i18n::t("文档工具", "Document tools"));
        let _ = self.open.set_text(i18n::t("打开临时文件夹", "Open temp folder"));
        let _ = self.clean.set_text(i18n::t("清理临时文件夹", "Clean temp folder"));
        let _ = self.settings.set_text(i18n::t("设置", "Settings"));
        let _ = self.autostart.set_text(i18n::t("开机自启动", "Launch at login"));
        let _ = self.quit.set_text(i18n::t("退出", "Quit"));
    }
}

fn config_path(app: &AppHandle) -> std::path::PathBuf {
    let dir = app.path().app_config_dir().unwrap_or_default();
    dir.join("config.json")
}

fn load_config(app: &AppHandle) -> Config {
    if let Ok(s) = std::fs::read_to_string(config_path(app)) {
        if let Ok(c) = serde_json::from_str::<Config>(&s) {
            return c;
        }
    }
    Config::default()
}

fn save_config(app: &AppHandle, cfg: &Config) {
    if let Some(dir) = config_path(app).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(config_path(app), serde_json::to_string_pretty(cfg).unwrap());
}

fn ensure_temp_dir(cfg: &Config) -> std::io::Result<()> {
    std::fs::create_dir_all(&cfg.temp_dir)
}

// ============== 全局鼠标轮询 + 手动拖动（16ms 拖动 / 33ms 事件） ==============
fn start_global_tracking(app: AppHandle) {
    std::thread::spawn(move || {
        // 左键按下状态（上一轮），用于检测按下沿
        let mut was_down = false;
        loop {
            let state = app.state::<AppState>();
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
            }
            let (mx, my) = (pt.x, pt.y);
            let win = app.get_webview_window("main");
            let (wx, wy) = match &win {
                Some(w) => match w.outer_position() {
                    Ok(p) => (p.x, p.y),
                    Err(_) => (0, 0),
                },
                None => (0, 0),
            };
            let (ww, wh) = match &win {
                Some(w) => match w.outer_size() {
                    Ok(s) => (s.width as i32, s.height as i32),
                    Err(_) => (0, 0),
                },
                None => (0, 0),
            };

            // 左键按下沿检测（VK_LBUTTON = 0x01，返回值 MSB 置位表示按下）。
            // 按下点位于主窗口矩形之外时，通知前端收起输入气泡（点击空白处消失）。
            let down = unsafe { GetAsyncKeyState(0x01) } < 0;
            let visible_w = state.visible_width.load(Ordering::SeqCst);
            let visible_h = state.visible_height.load(Ordering::SeqCst);
            if down && !was_down && !(mx >= wx + ww - visible_w && mx < wx + ww
                && my >= wy + wh - visible_h && my < wy + wh) {
                let _ = app.emit_to("main", "mouse-outside-click", ());
            }
            was_down = down;

            if state.dragging.load(Ordering::SeqCst) {
                if let Some((sx, sy, w0x, w0y)) = *state.drag_origin.lock().unwrap() {
                    let dist = ((mx - sx).pow(2) + (my - sy).pow(2)) as f64;
                    if dist.sqrt() >= DRAG_DEAD_ZONE as f64 {
                        if let Some(w) = &win {
                            let nx = mx - (sx - w0x);
                            let ny = my - (sy - w0y);
                            let _ = w.set_position(tauri::PhysicalPosition::new(nx, ny));
                        }
                    }
                }
            }

            let _ = app.emit_to(
                "main",
                "mouse-global",
                serde_json::json!({ "mx": mx, "my": my, "wx": wx, "wy": wy }),
            );
            drop(state);
            std::thread::sleep(Duration::from_millis(POLL_MS));
        }
    });
}

// ============== 屏幕工具 ==============
fn monitor_from_point() -> (i32, i32, i32, i32) {
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let h = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(h, &mut info).as_bool() {
            let r = info.rcWork;
            return (r.left, r.top, r.right - r.left, r.bottom - r.top);
        }
    }
    (0, 0, 800, 600)
}

fn primary_work_area() -> (i32, i32) {
    unsafe {
        let sw = GetSystemMetrics(SYSTEM_METRICS_INDEX(0)); // SM_CXSCREEN
        let sh = GetSystemMetrics(SYSTEM_METRICS_INDEX(1)); // SM_CYSCREEN
        if sw > 100 && sh > 100 {
            return (sw, sh);
        }
        let sw = GetSystemMetrics(SYSTEM_METRICS_INDEX(78)); // SM_CXVIRTUALSCREEN
        let sh = GetSystemMetrics(SYSTEM_METRICS_INDEX(79)); // SM_CYVIRTUALSCREEN
        (sw, sh)
    }
}

// ============== 诊断日志 ==============
// 拖放链路跑在 SysWOW/OLE 的回调里，出问题时 release 包没有控制台可看，
// 所以把关键节点追加到 %TEMP%\desktop-pet-docs.log，方便现场定位。
pub fn diag(message: &str) {
    let path = std::env::temp_dir().join("desktop-pet-docs.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(file, "{} {}", ms, message);
    }
}

// ============== 配置命令 ==============
#[tauri::command]
fn config_get(state: State<AppState>) -> Config {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
fn config_set(
    app: AppHandle,
    state: State<AppState>,
    new_cfg: serde_json::Value,
) -> Result<Config, String> {
    // 配置锁只在读写这一段持有：后面刷新菜单/托盘时如果还握着它，
    // 而那段代码又需要配置，就会在同一线程上把不可重入的 Mutex 锁死。
    let cfg = {
        let mut guard = state.config.lock().unwrap();
        let mut merged = serde_json::to_value(&*guard).map_err(|e| e.to_string())?;
        if let (Some(m), Some(n)) = (merged.as_object_mut(), new_cfg.as_object()) {
            for (k, v) in n {
                m.insert(k.clone(), v.clone());
            }
        }
        *guard = serde_json::from_value(merged).map_err(|e| e.to_string())?;
        save_config(&app, &guard);
        guard.clone()
    }; // 锁在这里释放

    // 语言：主进程侧（菜单/托盘/对话框）与前端同时切换
    let lang_changed = {
        let current = i18n::is_en();
        i18n::set_from_code(&cfg.lang);
        current != i18n::is_en()
    };
    if lang_changed {
        // 托盘菜单只就地改文案，绝不重建/替换：Windows 上运行时 tray.set_menu()
        // 会让主线程等消息泵，实测直接把应用卡死（右键菜单是点击时新建的，不受影响）。
        if let Some(items) = state.menu_items.lock().unwrap().as_ref() {
            items.apply_lang();
        }
        for label in ["main", "settings"] {
            if let Some(w) = app.get_webview_window(label) {
                let _ = w.emit("config-changed", serde_json::json!({ "lang": cfg.lang }));
            }
        }
    }

    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_always_on_top(cfg.always_on_top);
    }
    if let Some(tray) = state.tray.lock().unwrap().as_ref() {
        let _ = tray.set_visible(cfg.close_to_tray);
    }
    Ok(cfg)
}

#[tauri::command]
async fn config_pick_temp_dir(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let current = state.config.lock().unwrap().temp_dir.clone();
    let picked = app
        .dialog()
        .file()
        .set_title(i18n::t("选择临时文件存放目录", "Choose a temp folder"))
        .set_directory(&current)
        .blocking_pick_folder();
    if let Some(p) = picked {
        if let tauri_plugin_dialog::FilePath::Path(path) = p {
            let mut cfg = state.config.lock().unwrap();
            cfg.temp_dir = path.to_string_lossy().into_owned();
            save_config(&app, &cfg);
            return Ok(Some(cfg.temp_dir.clone()));
        }
    }
    Ok(None)
}

// ============== 窗口命令 ==============
#[tauri::command]
fn window_resize(window: tauri::WebviewWindow, w: f64, h: f64) {
    let scale = window.scale_factor().unwrap_or(1.0);
    if let (Ok(size), Ok(hwnd)) = (window.outer_size(), window.hwnd()) {
        let width = size.width as i32;
        let height = size.height as i32;
        let visible_w = (w * scale).round() as i32;
        let visible_h = (h * scale).round() as i32;
        // 只改变窗口可见且可点击的区域。WebView2 的实际尺寸保持不变，
        // 球体的绘制表面不会因弹出/收起气泡而重新创建或闪白。
        let region = unsafe {
            CreateRectRgn(
                (width - visible_w).max(0),
                (height - visible_h).max(0),
                width,
                height,
            )
        };
        if !region.0.is_null() {
            // Tauri 的 HWND 来自另一个 windows crate 版本，转换原始句柄即可。
            let result = unsafe { SetWindowRgn(HWND(hwnd.0 as *mut _), Some(region), false) };
            if result == 0 {
                unsafe { let _ = DeleteObject(HGDIOBJ(region.0)); }
            } else {
                let state = window.app_handle().state::<AppState>();
                state.visible_width.store(visible_w, Ordering::SeqCst);
                state.visible_height.store(visible_h, Ordering::SeqCst);
            }
        }
    }
}

#[tauri::command]
fn drag_start(state: State<AppState>, window: tauri::WebviewWindow) {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    let pos = window
        .outer_position()
        .unwrap_or(tauri::PhysicalPosition::new(0, 0));
    *state.drag_origin.lock().unwrap() = Some((pt.x, pt.y, pos.x, pos.y));
    state.dragging.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn drag_end(state: State<AppState>) {
    state.dragging.store(false, Ordering::SeqCst);
    *state.drag_origin.lock().unwrap() = None;
}

#[tauri::command]
fn request_close(app: AppHandle, state: State<AppState>) {
    let cfg = state.config.lock().unwrap().clone();
    if cfg.close_to_tray {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.hide();
        }
    } else {
        app.exit(0);
    }
}

#[tauri::command]
fn open_context_menu(window: tauri::WebviewWindow) {
    // 不要复用托盘菜单实例：Windows 上一个原生菜单只能属于一个宿主，
    // 托盘菜单与窗口弹出菜单共用同一个 Menu 会导致托盘右键菜单异常。
    let app = window.app_handle();
    let cfg = app.state::<AppState>().config.lock().unwrap().clone();
    if let Ok((menu, _)) = build_menu(app, &cfg) {
        let _ = window.popup_menu(&menu);
    }
}

#[tauri::command]
fn notify_blur() {}

// ============== 设置窗口 ==============
// 在 Windows 上，从同步命令中调用 WebviewWindowBuilder::build() 会死锁
// （WebView2 控制器创建是异步的，需要主线程消息泵，而同步命令阻塞了主线程），
// 表现为窗口空白、应用卡死或闪退。因此必须使用 async 命令 + run_on_main_thread。
//
// 标题栏颜色：默认跟随 Windows 系统强调色（本项目可能是青色系），
// 这里用 DWM 属性强制覆盖为中性米白背景 + 深灰文字，与设置页配色一致。
fn set_neutral_titlebar(hwnd: isize) {
    unsafe {
        // COLORREF 格式 0x00BBGGRR
        // 背景 #F7F6FA（页面底色）→ B=FA G=F6 R=F7
        let bg: u32 = 0x00FAF6F7;
        // 文字 #333333
        let fg: u32 = 0x00333333;
        let _ = DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_CAPTION_COLOR,
            &bg as *const u32 as *const _,
            std::mem::size_of::<u32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_TEXT_COLOR,
            &fg as *const u32 as *const _,
            std::mem::size_of::<u32>() as u32,
        );
    }
}

#[tauri::command]
async fn open_settings_command(app: AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        #[cfg(target_os = "windows")]
        if let Ok(h) = w.hwnd() {
            set_neutral_titlebar(h.0 as isize);
        }
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let (sx, sy, sw, sh) = monitor_from_point();
    let x = sx + ((sw as f64 - SETTINGS_W) / 2.0) as i32;
    let y = sy + ((sh as f64 - SETTINGS_H) / 2.0) as i32;
    let app_thread = app.clone();
    let _ = app.run_on_main_thread(move || {
        match tauri::WebviewWindowBuilder::new(
            &app_thread,
            "settings",
            tauri::WebviewUrl::App("settings.html".into()),
        )
        .title("设置")
        .inner_size(SETTINGS_W, SETTINGS_H)
        .resizable(false)
        .position(x as f64, y as f64)
        .visible(true)
        .focused(true)
        .build()
        {
            Ok(win) => {
                #[cfg(target_os = "windows")]
                if let Ok(h) = win.hwnd() {
                    set_neutral_titlebar(h.0 as isize);
                }
                let _ = win.set_focus();
            }
            Err(e) => {
                // 不要 panic（release 配置了 panic=abort，panic 会导致整个应用闪退）
                eprintln!("创建设置窗口失败: {e}");
            }
        }
    });
}

// 关闭设置窗口。WebView2 下原生 window.close() 对非脚本创建的窗口无效，
// 必须由后端关闭，否则点“保存/关闭”后窗口不会消失。
#[tauri::command]
fn close_settings(app: AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.close();
    }
}

// ============== 文档工具（悬浮球内联） ==============
// 不再单独建窗口：拖进来的文件只入队，然后让悬浮球弹出对话气泡，
// 由气泡完成「转换 / 压缩 → 开始 → 进度 → 完成」整条交互。

/// 拖放接收器专用入口：入队后通知悬浮球弹出文档气泡。
///
/// 注意：这里**不建窗口**。早先在 `IDropTarget::Drop` 回调里建 WebView2 窗口会
/// 因为拖放模态循环仍在运行而拿不到主线程消息泵，导致整个应用卡死退出。
/// 现在只做入队 + 事件通知，回调和事件循环都能正常返回。
pub fn stage_and_open_docs(app: &AppHandle, paths: Vec<String>) {
    {
        let state = app.state::<docs::DocsState>();
        let mut pending = state.pending.lock().unwrap();
        for p in paths.iter() {
            if !pending.contains(p) {
                pending.push(p.clone());
            }
        }
    }
    diag(&format!("drop staged {} 个文件，通知悬浮球", paths.len()));
    let app_handle = app.clone();
    std::thread::spawn(move || {
        // 等 Drop 回调真正返回（DoDragDrop 的模态循环随之退出）再通知前端，
        // 避免在 OLE 模态循环里同步驱动 webview。直接轮询标记，最多等 2 秒兜底。
        let start = std::time::Instant::now();
        while crate::drop_target::drop_in_flight() && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(100));
        open_docs(&app_handle, None);
    });
}

/// 入队 + 让悬浮球把文档气泡弹出来（菜单「文档工具」与拖放共用）。
fn open_docs(app: &AppHandle, paths: Option<Vec<String>>) {
    if let Some(list) = paths {
        let state = app.state::<docs::DocsState>();
        let mut pending = state.pending.lock().unwrap();
        for p in list {
            if !pending.contains(&p) {
                pending.push(p);
            }
        }
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.emit("docs-staged", ());
    }
}

// ============== 菜单 ==============
/// 构建右键菜单 / 托盘菜单，并把各项句柄一起返回（句柄用于切语言时就地改文字）。
/// 刻意把配置作为参数传入而不是在这里 `state.config.lock()`：切换语言会从 `config_set`
/// 触发菜单文案刷新，若这里再去取同一把不可重入的 `Mutex`，就会自锁。
fn build_menu(
    app: &AppHandle,
    cfg: &Config,
) -> tauri::Result<(Menu<tauri::Wry>, MenuHandles)> {
    // 分区一：文档相关
    let paste = MenuItemBuilder::with_id("paste", i18n::t("文档复制", "Copy documents")).build(app)?;
    let docs = MenuItemBuilder::with_id("docs", i18n::t("文档工具", "Document tools")).build(app)?;
    // 分区二：临时文件夹与程序设置
    let open = MenuItemBuilder::with_id("open", i18n::t("打开临时文件夹", "Open temp folder")).build(app)?;
    let clean =
        MenuItemBuilder::with_id("clean", i18n::t("清理临时文件夹", "Clean temp folder")).build(app)?;
    let settings = MenuItemBuilder::with_id("settings", i18n::t("设置", "Settings")).build(app)?;
    let autostart = CheckMenuItemBuilder::with_id("autostart", i18n::t("开机自启动", "Launch at login"))
        .checked(cfg.auto_start)
        .build(app)?;
    let quit = MenuItemBuilder::with_id("quit", i18n::t("退出", "Quit")).build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[&paste, &docs])
        .separator()
        .items(&[&open, &clean, &settings, &autostart, &quit])
        .build()?;
    let handles = MenuHandles {
        paste,
        docs,
        open,
        clean,
        settings,
        autostart,
        quit,
    };
    Ok((menu, handles))
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        "paste" => {
            let _ = app.emit_to("main", "float-activate-input", ());
        }
        "settings" => {
            let app_owned = app.to_owned();
            let _ = tauri::async_runtime::spawn(async move {
                open_settings_command(app_owned).await;
            });
        }
        "docs" => {
            open_docs(app, None);
        }
        "open" => {
            let cfg = app.state::<AppState>().config.lock().unwrap().clone();
            let _ = ensure_temp_dir(&cfg);
            let _ = std::process::Command::new("explorer").arg(&cfg.temp_dir).spawn();
        }
        "clean" => {
            let cfg = app.state::<AppState>().config.lock().unwrap().clone();
            let result = (|| -> std::io::Result<()> {
                if std::path::Path::new(&cfg.temp_dir).exists() {
                    std::fs::remove_dir_all(&cfg.temp_dir)?;
                }
                std::fs::create_dir_all(&cfg.temp_dir)
            })();
            match result {
                Ok(_) => {
                    let _ = app.emit_to("main", "float-toast", i18n::t("临时文件夹已清理", "Temp folder cleaned"));
                }
                Err(e) => {
                    let _ = app.emit_to(
                        "main",
                        "float-toast",
                        format!("{}{}", i18n::t("清理失败: ", "Clean failed: "), e),
                    );
                }
            }
        }
        "autostart" => {
            let state = app.state::<AppState>();
            let mut cfg = state.config.lock().unwrap();
            cfg.auto_start = !cfg.auto_start;
            let val = cfg.auto_start;
            save_config(app, &cfg);
            drop(cfg);
            if val {
                let _ = app.autolaunch().enable();
            } else {
                let _ = app.autolaunch().disable();
            }
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

// ============== 下载 ==============
fn filename_from_response(resp: &reqwest::Response, parsed: &url::Url) -> String {
    let mut name = String::new();
    if let Some(cd) = resp
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(part) = cd.split(';').find(|s| s.trim_start().starts_with("filename")) {
            let mut v = part
                .splitn(2, '=')
                .nth(1)
                .unwrap_or("")
                .trim()
                .trim_matches('"')
                .to_string();
            if let Some(idx) = v.find("''") {
                v = v[idx + 2..].to_string();
            }
            name = percent_decode_str(&v)
                .decode_utf8_lossy()
                .into_owned();
        }
    }
    if name.is_empty() {
        if let Some(seg) = parsed
            .path_segments()
            .and_then(|mut s| s.next_back())
            .map(|s| s.to_string())
        {
            name = percent_decode_str(&seg)
                .decode_utf8_lossy()
                .into_owned();
        }
    }
    if name.is_empty() {
        name = "download".to_string();
    }
    let cleaned: String = name
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string();
    if cleaned.is_empty() {
        "download".to_string()
    } else {
        cleaned
    }
}

fn copy_file_to_clipboard(path: &std::path::Path) -> bool {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let abs_str = abs.to_string_lossy().into_owned();
    match clipboard_rs::ClipboardContext::new() {
        Ok(ctx) => {
            if ctx.set_files(vec![abs_str.clone()]).is_ok() {
                true
            } else {
                ctx.set_text(abs_str).is_ok()
            }
        }
        Err(_) => false,
    }
}

#[tauri::command]
async fn download_start(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
) -> Result<DownloadResult, String> {
    let parsed = url::Url::parse(&url).map_err(|_| "链接格式不正确".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("仅支持 http/https 链接".to_string());
    }
    let cfg = state.config.lock().unwrap().clone();
    ensure_temp_dir(&cfg).map_err(|e| format!("无法创建临时目录: {}", e))?;

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 desktop-pet-tool/1.0")
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("初始化下载失败: {}", e))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载失败: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败，HTTP {}", resp.status().as_u16()));
    }

    let filename = filename_from_response(&resp, &parsed);
    let dest = std::path::Path::new(&cfg.temp_dir).join(&filename);
    let total = resp.content_length().unwrap_or(0);

    let mut stream = resp.bytes_stream();
    let mut out = std::fs::File::create(&dest).map_err(|e| format!("创建文件失败: {}", e))?;
    let mut received: u64 = 0;
    let mut last_report = std::time::Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("下载中断: {}", e))?;
        received += chunk.len() as u64;
        out.write_all(&chunk)
            .map_err(|e| format!("写入失败: {}", e))?;
        if last_report.elapsed().as_millis() >= 100 {
            last_report = std::time::Instant::now();
            let _ = app.emit_to(
                "main",
                "download-progress",
                serde_json::json!({
                    "received": received,
                    "total": total,
                    "filename": filename
                }),
            );
        }
    }
    let copied = copy_file_to_clipboard(&dest);
    Ok(DownloadResult {
        ok: true,
        file_path: dest.to_string_lossy().into_owned(),
        filename,
        size: received,
        copied_to_clipboard: copied,
    })
}

// ============== 入口 ==============
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            config: Mutex::new(Config::default()),
            dragging: AtomicBool::new(false),
            visible_width: AtomicI32::new(112),
            visible_height: AtomicI32::new(112),
            drag_origin: Mutex::new(None),
            menu: Mutex::new(None),
            menu_items: Mutex::new(None),
            tray: Mutex::new(None),
        })
        .manage(docs::DocsState::default())
        .setup(|app| {
            let cfg = load_config(app.handle());
            // 主进程侧文案（菜单/托盘/对话框）跟随配置里的语言
            i18n::set_from_code(&cfg.lang);
            {
                let state = app.state::<AppState>();
                *state.config.lock().unwrap() = cfg.clone();
            }
            let _ = ensure_temp_dir(&cfg);
            if cfg.auto_start {
                let _ = app.autolaunch().enable();
            }

            let (menu, menu_items) = build_menu(app.handle(), &cfg)?;
            {
                let state = app.state::<AppState>();
                *state.menu.lock().unwrap() = Some(menu.clone());
                *state.menu_items.lock().unwrap() = Some(menu_items);
            }

            // 托盘
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png")).ok();
            let tray = TrayIconBuilder::new()
                .icon(tray_icon.unwrap())
                .tooltip("desktop-pet-tool")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    // 只在左键抬起时恢复主窗口。右键由 tray-icon 默认逻辑弹出菜单，
                    // 此处若对右键也 show/set_focus 主窗口会抢走前台，导致刚弹出的
                    // 右键菜单因失焦被立即关闭。
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(w) = tray.app_handle().get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;
            {
                let state = app.state::<AppState>();
                *state.tray.lock().unwrap() = Some(tray.clone());
                let _ = tray.set_visible(cfg.close_to_tray);
            }

            // 主窗口初始位置：右下角偏上
            let main = app.get_webview_window("main").unwrap();
            let (sw, sh) = primary_work_area();
            // 保持悬浮球原来的屏幕位置：窗口右下角固定落在「屏宽-48 / 任务栏上沿-108」，
            // 窗口变大（文档气泡需要更高更宽的可视区域）时只向左上扩展。
            let scale = main.scale_factor().unwrap_or(1.0);
            let win_w = (MAIN_W * scale).round() as i32;
            let win_h = (MAIN_H * scale).round() as i32;
            let margin_r = (48.0 * scale).round() as i32;
            let margin_b = (108.0 * scale).round() as i32;
            let _ = main.set_position(tauri::PhysicalPosition::new(
                sw - margin_r - win_w,
                sh - margin_b - win_h,
            ));
            window_resize(main.clone(), 112.0, 112.0);
            let _ = main.show();
            // 窗口显示之后再接管拖放：此时 WebView2 的子窗口都已建出，
            // 能把主窗口和它们的 IDropTarget 一起换成自己的实现（原因见 drop_target.rs 顶部注释）。
            if let Ok(h) = main.hwnd() {
                drop_target::install(
                    AppHandle::clone(app.handle()),
                    HWND(h.0 as *mut _),
                );
            }
            // 注册在 App 级别：既能收到托盘菜单事件，也能收到窗口弹出菜单事件
            // （窗口级 on_menu_event 只处理该窗口自身的菜单，收不到托盘菜单的点击）。
            app.on_menu_event(|app_handle, event| handle_menu_event(app_handle, event));

            start_global_tracking(app.handle().clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            download_start,
            config_get,
            config_set,
            config_pick_temp_dir,
            window_resize,
            drag_start,
            drag_end,
            request_close,
            open_context_menu,
            notify_blur,
            open_settings_command,
            close_settings,
            docs::docs_pick_files,
            docs::docs_stage,
            docs::docs_take_pending,
            docs::docs_inspect,
            docs::docs_start,
            docs::docs_cancel,
            docs::docs_open_outdir
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
