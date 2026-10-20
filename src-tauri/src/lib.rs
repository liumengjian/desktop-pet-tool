#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clipboard_rs::Clipboard;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;
use percent_encoding::percent_decode_str;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SYSTEM_METRICS_INDEX,
};

const DRAG_DEAD_ZONE: i32 = 6;
const POLL_MS: u64 = 33;
const SETTINGS_W: f64 = 540.0;
const SETTINGS_H: f64 = 380.0;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Config {
    temp_dir: String,
    always_on_top: bool,
    auto_start: bool,
    close_to_tray: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            temp_dir: std::env::temp_dir()
                .join("file-paste-float")
                .to_string_lossy()
                .into_owned(),
            always_on_top: true,
            auto_start: false,
            close_to_tray: false,
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
    drag_origin: Mutex<Option<(i32, i32, i32, i32)>>, // (start_cursor_x, start_cursor_y, win_x, win_y)
    menu: Mutex<Option<Menu<tauri::Wry>>>,
    tray: Mutex<Option<TrayIcon<tauri::Wry>>>,
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
    std::thread::spawn(move || loop {
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
    let mut cfg = state.config.lock().unwrap();
    let mut merged = serde_json::to_value(&*cfg).map_err(|e| e.to_string())?;
    if let (Some(m), Some(n)) = (merged.as_object_mut(), new_cfg.as_object()) {
        for (k, v) in n {
            m.insert(k.clone(), v.clone());
        }
    }
    *cfg = serde_json::from_value(merged).map_err(|e| e.to_string())?;
    save_config(&app, &cfg);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_always_on_top(cfg.always_on_top);
    }
    if let Some(tray) = state.tray.lock().unwrap().as_ref() {
        let _ = tray.set_visible(cfg.close_to_tray);
    }
    Ok(cfg.clone())
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
        .set_title("选择临时文件存放目录")
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
    let pw = (w * scale) as i32;
    let ph = (h * scale) as i32;
    if let (Ok(size), Ok(pos)) = (window.outer_size(), window.outer_position()) {
        let right = pos.x + size.width as i32;
        let bottom = pos.y + size.height as i32;
        let _ = window.set_position(tauri::PhysicalPosition::new(right - pw, bottom - ph));
        let _ = window.set_size(tauri::PhysicalSize::new(pw as u32, ph as u32));
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
    if let Ok(menu) = build_menu(app) {
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

// ============== 菜单 ==============
fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let cfg = app.state::<AppState>().config.lock().unwrap().clone();
    let paste = MenuItemBuilder::with_id("paste", "粘贴文件链接…").build(app)?;
    let settings = MenuItemBuilder::with_id("settings", "设置").build(app)?;
    let open = MenuItemBuilder::with_id("open", "打开临时文件夹").build(app)?;
    let clean = MenuItemBuilder::with_id("clean", "清理临时文件夹").build(app)?;
    let autostart = CheckMenuItemBuilder::with_id("autostart", "开机自启动")
        .checked(cfg.auto_start)
        .build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "退出").build(app)?;
    MenuBuilder::new(app)
        .items(&[&paste, &settings, &open, &clean, &autostart, &quit])
        .build()
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
                    let _ = app.emit_to("main", "float-toast", "临时文件夹已清理");
                }
                Err(e) => {
                    let _ = app.emit_to("main", "float-toast", format!("清理失败: {}", e));
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
        .user_agent("Mozilla/5.0 FilePaste/1.0")
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
            drag_origin: Mutex::new(None),
            menu: Mutex::new(None),
            tray: Mutex::new(None),
        })
        .setup(|app| {
            let cfg = load_config(app.handle());
            {
                let state = app.state::<AppState>();
                *state.config.lock().unwrap() = cfg.clone();
            }
            let _ = ensure_temp_dir(&cfg);
            if cfg.auto_start {
                let _ = app.autolaunch().enable();
            }

            let menu = build_menu(app.handle())?;
            {
                let state = app.state::<AppState>();
                *state.menu.lock().unwrap() = Some(menu.clone());
            }

            // 托盘
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png")).ok();
            let tray = TrayIconBuilder::new()
                .icon(tray_icon.unwrap())
                .tooltip("FilePaste 悬浮文件粘贴")
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
            let _ = main.set_position(tauri::PhysicalPosition::new(sw - 160, sh - 220));
            let _ = main.show();
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
            close_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
