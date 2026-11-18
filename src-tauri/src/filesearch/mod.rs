// 文件查找：按文件名模糊匹配，范围可选「本机磁盘」或「网络共享盘」。
//
// 约束（与项目既有工具一致）：
//   - 只用 std 遍历，不引入新依赖；
//   - 扫描在后台线程里跑，随时可取消，结果按批推给前端，避免一次塞进上千条；
//   - 跳过系统目录与重解析点（junction / 符号链接），既提速也避免目录环；
//   - 网络共享盘支持 UNC 路径，也可以直接搜 Windows 里已映射的网络盘；
//   - 命中数量到上限就停，不让一次查找把 CPU / 内存占满。

mod engine;

use engine::{is_server_root, normalize_network_root, SearchQuery, WalkEvent};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

fn tr(zh: &str, en: &str) -> String {
    crate::i18n::t(zh, en)
}

/// 查找任务状态：同一时刻只跑一个查找，避免重复扫描把磁盘打满。
#[derive(Default)]
pub struct SearchState {
    pub running: AtomicBool,
    /// 当前任务的取消标记。任务开始时装进去，`fs_cancel` 把它置位。
    pub cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// 已扫描条目数（文件 + 目录），供进度事件读取。
    pub scanned: Arc<AtomicU64>,
    /// 命中数（可能大于返回上限），用于「已达上限」提示。
    pub hits: Arc<AtomicU64>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    /// 搜索来源：留空 = 本机所有磁盘；填了 = 只查这个文件夹（本机目录或网络共享）
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub path: String,
    pub name: String,
    pub dir: String,
    pub size: u64,
    pub size_label: String,
    /// 修改时间（Unix 秒），取不到为 0
    pub modified: u64,
    /// 匹配得分：100 前缀命中 / 92 包含 / 78 多关键词全命中 / 60 顺序模糊命中
    pub score: u8,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BatchPayload {
    items: Vec<Hit>,
    total: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    scanned: u64,
    hits: u64,
    elapsed_ms: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DonePayload {
    canceled: bool,
    truncated: bool,
    total: usize,
    scanned: u64,
    elapsed_ms: u64,
    roots: Vec<String>,
    errors: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TargetsPayload {
    local: Vec<String>,
    network: Vec<String>,
}

enum Msg {
    Hit(Hit),
    Err(String),
}

// ---------------- 扫描范围 ----------------

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;

fn local_roots() -> Vec<String> {
    drive_roots(&[DRIVE_FIXED, DRIVE_REMOVABLE])
}

fn network_roots() -> Vec<String> {
    drive_roots(&[DRIVE_REMOTE])
}

fn drive_roots(kinds: &[u32]) -> Vec<String> {
    let mut out = Vec::new();
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

        let mask = GetLogicalDrives();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let root = format!("{}:\\", char::from(b'A' + i as u8));
            let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
            let kind = GetDriveTypeW(PCWSTR(wide.as_ptr()));
            if kinds.contains(&kind) {
                out.push(root);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = kinds;
        out.push("/".to_string());
    }
    out
}

/// 枚举服务器上的磁盘共享（`\\DS423` → `\\DS423\共享1`、`\\DS423\共享2`…）。
/// 用户只填服务器名时用得上：这种 UNC 根不能直接 read_dir，必须先展开成共享。
#[cfg(target_os = "windows")]
fn server_shares(server: &str) -> Result<Vec<String>, String> {
    use windows::core::PCWSTR;
    use windows::Win32::NetworkManagement::NetManagement::NetApiBufferFree;
    use windows::Win32::Storage::FileSystem::{NetShareEnum, SHARE_INFO_1};

    const ERROR_MORE_DATA: u32 = 234;

    fn pwstr_text(p: windows::core::PWSTR) -> String {
        if p.0.is_null() {
            return String::new();
        }
        unsafe {
            let mut len = 0usize;
            while *p.0.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p.0 as *const u16, len))
        }
    }

    let wide: Vec<u16> = server.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf: *mut u8 = std::ptr::null_mut();
    let mut read: u32 = 0;
    let mut total: u32 = 0;
    // level 1 只要共享名与类型；MAX_PREFERRED_LENGTH 表示一次取全
    let status = unsafe {
        NetShareEnum(
            PCWSTR(wide.as_ptr()),
            1,
            &mut buf,
            u32::MAX,
            &mut read,
            &mut total,
            None,
        )
    };

    let mut names = Vec::new();
    if !buf.is_null() && read > 0 {
        let list = unsafe { std::slice::from_raw_parts(buf as *const SHARE_INFO_1, read as usize) };
        for info in list {
            let name = pwstr_text(info.shi1_netname);
            // 跳过管理共享（C$、ADMIN$ 等）
            if name.is_empty() || name.ends_with('$') {
                continue;
            }
            // STYPE_DISKTREE = 0：只要磁盘共享，不要打印 / 设备 / IPC
            if info.shi1_type.0 & 0xFFFF != 0 {
                continue;
            }
            names.push(format!("{}\\{}", server, name));
        }
    }
    if !buf.is_null() {
        unsafe {
            NetApiBufferFree(Some(buf as *const _));
        }
    }

    if status != 0 && status != ERROR_MORE_DATA {
        return Err(format!(
            "{}{} ({})\n{}",
            tr("无法读取服务器上的共享列表：", "Cannot list shares on "),
            server,
            status,
            tr(
                "请先在资源管理器里访问一次该服务器或保存登录凭据，也可以直接填写完整的共享路径。",
                "Open the server once in File Explorer or save credentials, or enter the full share path instead."
            )
        ));
    }
    if names.is_empty() {
        return Err(format!(
            "{}{}{}",
            tr(
                "该服务器上没有找到可搜索的共享文件夹，请填写完整的共享路径，例如 ",
                "No searchable share found on this server — enter the full share path, e.g. "
            ),
            server,
            tr("\\共享", "\\share")
        ));
    }
    Ok(names)
}

#[cfg(not(target_os = "windows"))]
fn server_shares(server: &str) -> Result<Vec<String>, String> {
    Err(format!(
        "{}{}",
        tr(
            "暂不支持按服务器名枚举共享：",
            "Server share enumeration is not supported: "
        ),
        server
    ))
}

fn resolve_roots(req: &SearchRequest) -> Result<Vec<String>, String> {
    let raw = req.root.as_deref().unwrap_or("").trim();

    // 来源留空：只搜本机磁盘（不含已映射的网络盘）
    if raw.is_empty() {
        let roots = local_roots();
        if roots.is_empty() {
            return Err(tr("没有找到可扫描的磁盘", "No scannable drive found"));
        }
        return Ok(roots);
    }

    let mut root = normalize_network_root(raw);
    // 只填了服务器名（含 IP / 主机名）：补成 UNC 根，再展开共享
    if !root.contains('\\') && !root.contains(':') && root.len() > 1 {
        root = format!("\\\\{}", root);
    }
    if is_server_root(&root) {
        return server_shares(&root);
    }
    if !Path::new(&root).is_dir() {
        return Err(format!(
            "{}{}",
            tr("无法访问这个路径：", "Cannot access this path: "),
            root
        ));
    }
    Ok(vec![root])
}

// ---------------- 扫描线程 ----------------

fn walk_roots(
    tx: Sender<Msg>,
    roots: Vec<String>,
    query: SearchQuery,
    cancel: &AtomicBool,
    scanned: &AtomicU64,
    hits: &AtomicU64,
    enough: &AtomicBool,
) {
    engine::walk_roots_into(roots, query, cancel, scanned, enough, |event| match event {
        WalkEvent::Hit(raw) => {
            hits.fetch_add(1, Ordering::Relaxed);
            let hit = Hit {
                path: raw.path,
                name: raw.name,
                dir: raw.dir,
                size: raw.size,
                size_label: raw.size_label,
                modified: raw.modified,
                score: raw.score,
            };
            tx.send(Msg::Hit(hit)).is_ok()
        }
        WalkEvent::Error(message) => tx.send(Msg::Err(message)).is_ok(),
    });
}

fn emit_batch(app: &AppHandle, items: Vec<Hit>, total: usize) {
    let _ = app.emit_to("main", "fs-batch", BatchPayload { items, total });
}

fn emit_progress(app: &AppHandle, scanned: &AtomicU64, hits: &AtomicU64, started: Instant) {
    let _ = app.emit_to(
        "main",
        "fs-progress",
        ProgressPayload {
            scanned: scanned.load(Ordering::Relaxed),
            hits: hits.load(Ordering::Relaxed),
            elapsed_ms: started.elapsed().as_millis() as u64,
        },
    );
}

fn run_search(
    app: AppHandle,
    query: SearchQuery,
    roots: Vec<String>,
    limit: usize,
    cancel: Arc<AtomicBool>,
    scanned: Arc<AtomicU64>,
    hits: Arc<AtomicU64>,
) {
    let started = Instant::now();
    scanned.store(0, Ordering::Relaxed);
    hits.store(0, Ordering::Relaxed);

    let (tx, rx) = mpsc::channel::<Msg>();
    let enough = Arc::new(AtomicBool::new(false));

    // 双核办公机上最多两条扫描线程，磁盘为主，CPU 占用很低
    let worker_count = roots.len().min(2).max(1);
    let mut buckets: Vec<Vec<String>> = (0..worker_count).map(|_| Vec::new()).collect();
    for (i, root) in roots.iter().enumerate() {
        buckets[i % worker_count].push(root.clone());
    }

    let mut workers = Vec::new();
    for bucket in buckets {
        let tx = tx.clone();
        let cancel = cancel.clone();
        let scanned = scanned.clone();
        let hits = hits.clone();
        let enough = enough.clone();
        let query = query.clone();
        workers.push(std::thread::spawn(move || {
            walk_roots(tx, bucket, query, &cancel, &scanned, &hits, &enough);
        }));
    }
    drop(tx);

    let mut collected: Vec<Hit> = Vec::new();
    let mut batch: Vec<Hit> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut last_emit = Instant::now();
    let mut last_progress = Instant::now();

    loop {
        match rx.recv_timeout(Duration::from_millis(120)) {
            Ok(Msg::Hit(hit)) => {
                if collected.len() < limit {
                    collected.push(hit.clone());
                    batch.push(hit);
                    if collected.len() >= limit {
                        // 到上限：让所有线程尽快收手，避免白扫几分钟
                        enough.store(true, Ordering::SeqCst);
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            Ok(Msg::Err(err)) => {
                if errors.len() < 6 && !errors.contains(&err) {
                    errors.push(err);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        let now = Instant::now();
        if !batch.is_empty() && now.duration_since(last_emit).as_millis() >= 200 {
            emit_batch(&app, std::mem::take(&mut batch), collected.len());
            last_emit = now;
        }
        if now.duration_since(last_progress).as_millis() >= 300 {
            emit_progress(&app, &scanned, &hits, started);
            last_progress = now;
        }
        if cancel.load(Ordering::SeqCst) {
            // 用户点了停止：不再等线程逐个退出（每处理一个目录都会检查标记）
            break;
        }
    }

    if !batch.is_empty() {
        emit_batch(&app, std::mem::take(&mut batch), collected.len());
    }

    let canceled = cancel.load(Ordering::SeqCst);
    if !canceled {
        // 正常跑完时线程都已经结束，join 立即返回
        for worker in workers {
            let _ = worker.join();
        }
    }

    let _ = app.emit_to(
        "main",
        "fs-done",
        DonePayload {
            canceled,
            truncated,
            total: collected.len(),
            scanned: scanned.load(Ordering::Relaxed),
            elapsed_ms: started.elapsed().as_millis() as u64,
            roots,
            errors,
        },
    );
    app.state::<SearchState>()
        .running
        .store(false, Ordering::SeqCst);
}

// ---------------- 命令 ----------------

/// 可搜索的磁盘 / 已映射网络盘清单（供前端展示，不在界面里猜）
#[tauri::command]
pub async fn fs_targets() -> Result<TargetsPayload, String> {
    tauri::async_runtime::spawn_blocking(|| TargetsPayload {
        local: local_roots(),
        network: network_roots(),
    })
    .await
    .map_err(|e| e.to_string())
}

/// 选择网络共享盘（系统文件夹选择框，支持浏览网络位置）
#[tauri::command]
pub async fn fs_pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    #[cfg(target_os = "windows")]
    {
        let _ = app;
        let title = tr("选择要查找的文件夹", "Choose a folder to search");
        tauri::async_runtime::spawn_blocking(move || pick_folder_win(&title))
            .await
            .map_err(|e| e.to_string())?
    }

    #[cfg(not(target_os = "windows"))]
    {
        let picked = tauri::async_runtime::spawn_blocking(move || {
            use tauri_plugin_dialog::DialogExt;
            app.dialog()
                .file()
                .set_title(tr("选择要查找的文件夹", "Choose a folder to search").as_str())
                .blocking_pick_folder()
        })
        .await
        .map_err(|e| e.to_string())?;
        Ok(picked
            .and_then(|p| p.into_path().ok())
            .map(|p| p.to_string_lossy().to_string()))
    }
}

/// Windows 文件夹选择框。
///
/// 不能用 tauri-plugin-dialog（底层 rfd）现成实现：rfd 只用
/// `GetDisplayName(SIGDN_FILESYSPATH)` 取选中项。网络服务器根（`\\DS423`）是
/// Shell 命名空间里的文件夹、没有文件系统路径，这一步必然失败，于是选择结果
/// 变成 `None`，输入框就空着。这里改用解析名兜底：
/// 先试文件系统路径，取不到再取 `SIGDN_DESKTOPABSOLUTEPARSING`（服务器根会得到
/// `\\DS423`）。同时明确不设 `FOS_FORCEFILESYSTEM`，否则服务器根根本选不中。
#[cfg(target_os = "windows")]
fn pick_folder_win(title: &str) -> Result<Option<String>, String> {
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_DESKTOPABSOLUTEPARSING,
        SIGDN_FILESYSPATH,
    };

    // CLSID_FileOpenDialog
    const CLSID_FILE_OPEN_DIALOG: GUID = GUID::from_u128(0xdc1c5a9c_e88a_4dde_a5a1_60f82a20aef7);

    unsafe {
        // IFileOpenDialog 要求 STA；阻塞线程池里的线程通常还没初始化 COM
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // 只有本次真正初始化成功才配对 CoUninitialize，S_FALSE 表示之前已初始化
        let com_owned = hr == windows::Win32::Foundation::S_OK;
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(format!("{}: {hr:?}", tr("COM 初始化失败", "COM init failed")));
        }

        let result = (|| -> Result<Option<String>, String> {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&CLSID_FILE_OPEN_DIALOG, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| format!("{}{e}", tr("创建选择框失败: ", "Failed to create dialog: ")))?;

            let mut opts = dialog.GetOptions().map_err(|e| e.to_string())?;
            opts |= FOS_PICKFOLDERS;
            opts &= !FOS_FORCEFILESYSTEM;
            dialog.SetOptions(opts).map_err(|e| e.to_string())?;

            let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            dialog
                .SetTitle(PCWSTR(wide.as_ptr()))
                .map_err(|e| e.to_string())?;

            if let Err(err) = dialog.Show(None) {
                // 用户取消是 0x800704C7，当作没选，不报错
                if err.code().0 == 0x8007_04C7u32 as i32 {
                    return Ok(None);
                }
                return Err(format!(
                    "{}{err}",
                    tr("打开选择框失败: ", "Failed to open dialog: ")
                ));
            }

            let item = dialog.GetResult().map_err(|e| {
                format!("{}{e}", tr("读取选择结果失败: ", "Failed to read result: "))
            })?;
            // 普通文件夹直接有文件系统路径；服务器根没有，退到解析名
            let name = item
                .GetDisplayName(SIGDN_FILESYSPATH)
                .or_else(|_| item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING))
                .map_err(|e| {
                    format!("{}{e}", tr("读取选择路径失败: ", "Failed to read path: "))
                })?;
            let text = name.to_string().unwrap_or_default();
            CoTaskMemFree(Some(name.as_ptr() as *const _));
            Ok(if text.is_empty() { None } else { Some(text) })
        })();

        if com_owned {
            CoUninitialize();
        }
        result
    }
}

#[tauri::command]
pub async fn fs_search(
    app: AppHandle,
    state: State<'_, SearchState>,
    req: SearchRequest,
) -> Result<(), String> {
    let raw = req.query.trim().to_string();
    if raw.is_empty() {
        return Err(tr(
            "请输入要查找的文件名关键词",
            "Enter a file-name keyword",
        ));
    }
    // 共享盘预检可能因为主机不在线而卡上几秒，放到阻塞线程池里等，别占着界面线程
    let probe = SearchRequest {
        query: raw.clone(),
        root: req.root.clone(),
        limit: req.limit,
    };
    let roots = tauri::async_runtime::spawn_blocking(move || resolve_roots(&probe))
        .await
        .map_err(|e| e.to_string())??;

    if state.running.swap(true, Ordering::SeqCst) {
        return Err(tr(
            "已有查找正在进行，请稍候",
            "A search is already running",
        ));
    }

    let cancel = Arc::new(AtomicBool::new(false));
    *state.cancel.lock().unwrap() = Some(cancel.clone());
    let scanned = state.scanned.clone();
    let hits = state.hits.clone();
    let limit = req.limit.unwrap_or(300).clamp(20, 800);
    let query = SearchQuery::new(&raw);
    let app_handle = app.clone();

    std::thread::spawn(move || {
        run_search(app_handle, query, roots, limit, cancel, scanned, hits);
    });

    Ok(())
}

#[tauri::command]
pub fn fs_cancel(state: State<'_, SearchState>) {
    if let Some(flag) = state.cancel.lock().unwrap().as_ref() {
        flag.store(true, Ordering::SeqCst);
    }
}

/// 在资源管理器里定位（打开所在文件夹并选中该文件）
#[tauri::command]
pub async fn fs_reveal(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || reveal_blocking(path))
        .await
        .map_err(|e| e.to_string())?
}

fn reveal_blocking(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(tr(
            "文件已不存在，可能已被移动或删除",
            "The file no longer exists",
        ));
    }
    let folder = p
        .parent()
        .map(|dir| dir.to_path_buf())
        .unwrap_or_default();
    crate::reveal::open_folder_selecting(&folder, std::slice::from_ref(&path))
}

/// 用系统默认程序打开文件
#[tauri::command]
pub async fn fs_open(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open_blocking(path))
        .await
        .map_err(|e| e.to_string())?
}

fn open_blocking(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(tr(
            "文件已不存在，可能已被移动或删除",
            "The file no longer exists",
        ));
    }
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let op: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
        let file: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let result = ShellExecuteW(
            None,
            PCWSTR(op.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if result.is_invalid() {
            return Err(tr("无法打开这个文件", "Cannot open this file").to_string());
        }
    }
    Ok(())
}
