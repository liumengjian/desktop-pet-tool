// 拖放接收器：自己接管窗口的 OLE IDropTarget，不再依赖 tauri://drag-* 的默认通道。
//
// 为什么需要这一层：
// wry 只在 WebView2 建窗那一刻用 EnumChildWindows 一次性把 IDropTarget 注入到「当时已存在」的子窗口
// （wry/src/webview2/mod.rs:156-164 与 drag_drop.rs:45-64），此后新出现的子窗口不会再注册。
// 悬浮球窗口把可见/可点击区用 SetWindowRgn 裁成右下 112×112，拖拽落点很容易命中
// Chrome_RenderWidgetHostHWND 这类没有 drop target 的子窗口；一旦命中层没有 target，
// 系统沿祖先链找不到接收者，enter / over / drop 三个阶段全部沉默 ——
// 用户看到的就是「把文件拖到球上毫无反应，也没有进入待处理状态」。
// 这里在窗口显示之后把主窗口和全部子窗口的 IDropTarget 换成我们自己的实现，
// 由我们自己转发 enter/over/leave/drop 给前端，并在 drop 时直接把文件推进待处理队列。

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Manager};use windows::core::{implement, Ref};
use windows::Win32::Foundation::{HWND, POINTL};
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, RegisterDragDrop, RevokeDragDrop, CF_HDROP, DROPEFFECT,
    DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{GetWindow, GW_CHILD, GW_HWNDNEXT};

#[implement(IDropTarget)]
struct BallDropTarget {
    app: AppHandle,
    // DragEnter 判定为「可接收文件」后才继续发 over/leave/drop，避免拖文本时乱闪表情
    entered: AtomicBool,
}

// OLE 拖放回调进行中标记：Drop 期间先入队，等这个标记落下来再通知前端，
// 避免在 OLE 模态循环里同步驱动 webview。
static DROP_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 当前是否还处在 OLE 拖放回调里（DragEnter/DragOver/DragLeave/Drop 期间）。
pub fn drop_in_flight() -> bool {
    DROP_IN_FLIGHT.load(Ordering::SeqCst)
}

impl BallDropTarget {
    /// 从 IDataObject 取出 CF_HDROP 里的文件路径；不是文件拖放时返回 None。
    fn take_paths(data: Ref<'_, IDataObject>) -> Option<(Vec<String>, HDROP)> {
        let obj = data.as_ref()?;
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let medium = unsafe { obj.GetData(&format) }.ok()?;
        let hdrop = unsafe { HDROP(medium.u.hGlobal.0 as _) };
        // 0xFFFFFFFF 表示「只返回条目数」
        let count = unsafe { DragQueryFileW(hdrop, 0xFFFFFFFF, None) };
        let mut paths = Vec::new();
        for i in 0..count {
            let len = unsafe { DragQueryFileW(hdrop, i, None) } as usize;
            let mut buf = vec![0u16; len + 1];
            unsafe { DragQueryFileW(hdrop, i, Some(&mut buf)) };
            paths.push(String::from_utf16_lossy(&buf[..len]));
        }
        Some((paths, hdrop))
    }
}

impl IDropTarget_Impl for BallDropTarget_Impl {
    fn DragEnter(
        &self,
        pdataobj: Ref<'_, IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        // 只在确实是文件拖放时接管，其它类型（文本、网页片段）交还系统
        let is_files = BallDropTarget::take_paths(pdataobj).is_some();
        self.entered.store(is_files, Ordering::SeqCst);
        unsafe { *pdweffect = if is_files { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        if is_files {
            // 复用前端已有的「接收中」视觉：开心表情 + 松手提示气泡
            let _ = self.app.emit_to("main", "tauri://drag-enter", ());
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let active = self.entered.load(Ordering::SeqCst);
        unsafe { *pdweffect = if active { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        if active {
            let _ = self.app.emit_to("main", "tauri://drag-over", ());
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if self.entered.swap(false, Ordering::SeqCst) {
            let _ = self.app.emit_to("main", "tauri://drag-leave", ());
        }
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: Ref<'_, IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        DROP_IN_FLIGHT.store(true, Ordering::SeqCst);
        let result = self.drop_inner(pdataobj, pdweffect);
        // 无论走哪条分支，都要在返回前把标记落下来：建窗线程在等它
        DROP_IN_FLIGHT.store(false, Ordering::SeqCst);
        result
    }
}

impl BallDropTarget {
    fn drop_inner(
        &self,
        pdataobj: Ref<'_, IDataObject>,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let was_active = self.entered.swap(false, Ordering::SeqCst);
        unsafe { *pdweffect = DROPEFFECT_COPY };
        if let Some((paths, hdrop)) = BallDropTarget::take_paths(pdataobj) {
            if was_active {
                // 前端据此收起「接收中」状态；payload 结构与 tauri 原生一致，前端逻辑零改动
                let _ = self
                    .app
                    .emit_to("main", "tauri://drag-drop", serde_json::json!({ "paths": paths }));
            }
            unsafe { DragFinish(hdrop) };
            if !paths.is_empty() {
                // 关键：不在 Drop 回调里做重活。
                // Drop 跑在 OLE 拖放模态循环中，此时 run_on_main_thread 会同步内联执行
                // （当前线程就是主线程），建 WebView2 窗口/等待事件循环都会把应用卡死。
                // 现在文档交互改到悬浮球气泡里，只需入队 + 发事件，但仍交给线程等 Drop 返回后再发，
                // 避免在模态循环里同步驱动 webview。
                crate::stage_and_open_docs(&self.app, paths);
            }
        }
        Ok(())
    }
}

/// 把主窗口及其全部子孙窗口的拖放接收换成自己的实现。
/// WebView2 的 Chrome_WidgetWin_1 / Chrome_RenderWidgetHostHWND 挂在 Chrome_WidgetWin_0 之下，
/// 只遍历直接子窗口会漏掉它们，而拖拽落点恰恰常命中最内层，所以这里递归整棵树。
pub fn install(app: AppHandle, hwnd: HWND) {
    let target: IDropTarget = BallDropTarget {
        app: app.clone(),
        entered: AtomicBool::new(false),
    }
    .into();
    // 不在这里持有强引用：RegisterDragDrop 会对每个窗口 AddRef，
    // 窗口销毁时 RevokeDragDrop 释放，COM 对象随之回收。
    let mut registered = 0usize;
    unsafe { hook_tree(hwnd, &target, &mut registered) };
    if registered == 0 {
        eprintln!("拖放接收器未能注册到任何窗口");
    }

    // WebView2 在首帧前后还会补建子窗口，稍后再挂一次，确保最内层也有接收器
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let app_outer = app.clone();
        let app_inner = app.clone();
        let _ = app_outer.run_on_main_thread(move || {
            if let Some(w) = app_inner.get_webview_window("main") {
                if let Ok(h) = w.hwnd() {
                    let target: IDropTarget = BallDropTarget {
                        app: AppHandle::clone(&app_inner),
                        entered: AtomicBool::new(false),
                    }
                    .into();
                    let mut registered = 0usize;
                    unsafe { hook_tree(HWND(h.0 as *mut _), &target, &mut registered) };
                    let _ = registered;
                }
            }
        });
    });
}

unsafe fn hook_tree(hwnd: HWND, target: &IDropTarget, registered: &mut usize) {
    let _ = RevokeDragDrop(hwnd);
    match RegisterDragDrop(hwnd, target) {
        Ok(()) => *registered += 1,
        Err(err) => eprintln!("拖放注册失败: {err}"),
    }

    let mut child = GetWindow(hwnd, GW_CHILD).unwrap_or_default();
    while !child.is_invalid() {
        hook_tree(child, target, registered);
        child = GetWindow(child, GW_HWNDNEXT).unwrap_or_default();
    }
}
