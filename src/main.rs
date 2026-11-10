// 发布版不弹控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::time::Duration;

use eframe::egui;
use egui::viewport::{ViewportCommand, WindowLevel};
use egui::{Color32, Pos2, Sense, Shape, Stroke, Vec2};

/// 窗口边长（逻辑像素）
const PANE: f32 = 68.0;
/// 图钉离屏幕右下角的留白
const MARGIN: f32 = 26.0;

const C_IDLE: Color32 = Color32::from_rgb(148, 157, 177);
const C_PINNED: Color32 = Color32::from_rgb(255, 104, 86);

// ============================================================
// Win32：找鼠标下的窗口、置顶、原生菜单
// ============================================================

#[cfg(windows)]
mod w32 {
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn class_of(h: HWND) -> String {
        let mut buf = [0u16; 256];
        let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
        if n <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..n as usize])
    }

    /// 桌面外壳 / 托盘 / 提示框：这些不是用户想钉住的对象
    fn is_shell_class(class: &str) -> bool {
        matches!(
            class,
            "Progman"
                | "WorkerW"
                | "Shell_TrayWnd"
                | "Shell_SecondaryTrayWnd"
                | "SysListView32"
                | "tooltips_class32"
        )
    }

    fn ex_style(h: HWND) -> isize {
        unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) }
    }

    pub fn is_topmost(h: HWND) -> bool {
        ex_style(h) & (WS_EX_TOPMOST as isize) != 0
    }

    /// 从 Z 序最顶层往下扫，返回鼠标指针下第一个「可钉住」的窗口。
    ///
    /// 这里不用 WindowFromPoint：拖动时图钉就压在鼠标正下方，
    /// WindowFromPoint 只会返回图钉自己。按 Z 序逐个比对矩形，
    /// 跳过自己之后拿到的就是用户视觉上看到的那个窗口。
    pub fn under_cursor(self_hwnd: HWND) -> Option<HWND> {
        let mut pt = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut pt) } == 0 {
            return None;
        }

        let mut h = unsafe { GetTopWindow(std::ptr::null_mut()) };
        let mut guard = 0;

        while !h.is_null() && guard < 512 {
            guard += 1;

            if h != self_hwnd && unsafe { IsWindowVisible(h) } != 0 {
                // 鼠标穿透的覆盖层不参与命中，否则会钉到一层看不见的东西上
                let click_through = ex_style(h) & (WS_EX_TRANSPARENT as isize) != 0;

                let mut rc: RECT = unsafe { std::mem::zeroed() };
                let inside = unsafe { GetWindowRect(h, &mut rc) } != 0
                    && pt.x >= rc.left
                    && pt.x < rc.right
                    && pt.y >= rc.top
                    && pt.y < rc.bottom;

                if inside && !click_through && !is_shell_class(&class_of(h)) {
                    return Some(h);
                }
            }

            h = unsafe { GetWindow(h, GW_HWNDNEXT) };
        }

        None
    }

    pub fn set_pinned(h: HWND, on: bool) -> bool {
        if unsafe { IsWindow(h) } == 0 {
            return false;
        }
        let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
        unsafe {
            SetWindowPos(h, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) != 0
        }
    }

    /// 只提 Z 序，不碰窗口级别。
    ///
    /// 目标窗口被置顶之后，图钉（也是 topmost）必须排在它前面才看得见；
    /// 而两者都是 topmost 时，后一次 SetWindowPos 的会排到最前。
    pub fn raise(h: HWND) {
        unsafe {
            SetWindowPos(
                h,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// 窗口的屏幕矩形（物理像素）：(left, top, width, height)。
    pub fn window_rect(h: HWND) -> Option<(i32, i32, i32, i32)> {
        if unsafe { IsWindow(h) } == 0 {
            return None;
        }
        let mut rc: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(h, &mut rc) } == 0 {
            return None;
        }
        let w = rc.right - rc.left;
        let hh = rc.bottom - rc.top;
        if w <= 0 || hh <= 0 {
            return None;
        }
        Some((rc.left, rc.top, w, hh))
    }

    pub fn minimized(h: HWND) -> bool {
        unsafe { IsIconic(h) != 0 }
    }

    pub fn pid_of(h: HWND) -> u32 {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, &mut pid) };
        pid
    }

    /// 目标窗口属于哪个进程（只要文件名）。取不到就返回空串。
    pub fn proc_name(h: HWND) -> String {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, &mut pid) };
        if pid == 0 {
            return String::new();
        }
        let hp = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if hp.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(hp, 0, buf.as_mut_ptr(), &mut len) };
        unsafe { CloseHandle(hp) };
        if ok == 0 || len == 0 {
            return String::new();
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit('\\').next().unwrap_or("").to_string()
    }

    pub fn alive(h: HWND) -> bool {
        // 注意：写成 `unsafe { IsWindow(h) } != 0` 会编译不过 ——
        // unsafe 块出现在语句位置时会被当成语句，后面的 != 就没有左操作数了。
        unsafe { IsWindow(h) != 0 }
    }

    /// 屏幕坐标下的鼠标位置。
    ///
    /// 用它来判断「用户是拖了还是只点了一下」：系统拖动结束后，
    /// egui 的 outer_rect 还停留在拖动前的值，要等下一帧才同步，
    /// 拿它做判据会把拖动误判成单击。鼠标坐标是即时的，不会错。
    pub fn cursor() -> Option<(i32, i32)> {
        let mut pt = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut pt) } == 0 {
            return None;
        }
        Some((pt.x, pt.y))
    }

    /// 原生弹出菜单。
    ///
    /// 不用 egui 的 context_menu：egui 的菜单画在当前视口内部，
    /// 而图钉窗口只有 68px，菜单会被窗口边界整块裁掉。
    /// 返回 1 = 解除置顶，2 = 退出，0 = 取消。
    ///
    /// 第一项是只读的「已钉住 xxx.exe」：图钉贴在窗口角上时，
    /// 光看图钉分不清它到底钉的是哪个程序，这一行同时承担状态回显。
    pub fn popup_menu(hwnd: HWND, pinned_name: Option<&str>) -> i32 {
        let mut pt = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut pt) };

        let head = pinned_name.map(|n| wide(&format!("已钉住 {n}")));
        let unpin = wide("解除置顶");
        let quit = wide("退出");

        unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return 0;
            }
            if let Some(h) = &head {
                AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, h.as_ptr());
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            }
            AppendMenuW(menu, MF_STRING, 1, unpin.as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(menu, MF_STRING, 2, quit.as_ptr());

            // 不先设前景窗口，点菜单外面菜单不会消失 —— Win32 的老坑
            SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                0,
                hwnd,
                std::ptr::null(),
            );
            DestroyMenu(menu);
            // 投一个空消息，防止菜单在极端情况下残留不消失
            PostMessageW(hwnd, WM_NULL, 0, 0);
            cmd
        }
    }

    pub fn screen_size() -> (i32, i32) {
        unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
    }
}

#[cfg(not(windows))]
mod w32 {
    use core::ffi::c_void;

    pub fn under_cursor(_self_hwnd: *mut c_void) -> Option<*mut c_void> {
        None
    }
    pub fn set_pinned(_h: *mut c_void, _on: bool) -> bool {
        false
    }
    pub fn is_topmost(_h: *mut c_void) -> bool {
        false
    }
    pub fn raise(_h: *mut c_void) {}
    pub fn window_rect(_h: *mut c_void) -> Option<(i32, i32, i32, i32)> {
        None
    }
    pub fn minimized(_h: *mut c_void) -> bool {
        false
    }
    pub fn pid_of(_h: *mut c_void) -> u32 {
        0
    }
    pub fn proc_name(_h: *mut c_void) -> String {
        String::new()
    }
    pub fn alive(_h: *mut c_void) -> bool {
        false
    }
    pub fn cursor() -> Option<(i32, i32)> {
        None
    }
    pub fn popup_menu(_h: *mut c_void, _pinned: Option<&str>) -> i32 {
        0
    }
    pub fn screen_size() -> (i32, i32) {
        (1920, 1080)
    }
}

// ============================================================
// 跟随的事件源：SetWinEventHook
//
// 图钉贴在目标窗口上之后必须跟着它动。最直接的办法是定时轮询
// GetWindowRect，但实测 60ms 一次会吃掉 3% CPU —— 对一个以省电为
// 卖点的小工具来说不可接受，而 150ms 一次又会明显跟不上手。
//
// 改成事件驱动：系统在目标窗口位置变化时回调我们，静止时一次都
// 不醒。代价是要把回调上下文放进 static（C 回调没法捕获环境）。
//
// 兜底：万一某些环境下 hook 收不到事件，主循环仍有低频轮询兜着，
// 最坏退化成一秒跟上一次，不会彻底失效。
// ============================================================

#[cfg(windows)]
mod follow {
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::Mutex;

    use eframe::egui;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EVENT_OBJECT_LOCATIONCHANGE, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    };

    static TARGET: AtomicIsize = AtomicIsize::new(0);
    static HOOK: AtomicIsize = AtomicIsize::new(0);
    static CTX: Mutex<Option<egui::Context>> = Mutex::new(None);

    unsafe extern "system" fn on_event(
        _hook: HWINEVENTHOOK,
        event: u32,
        hwnd: HWND,
        id_object: i32,
        _id_child: i32,
        _thread: u32,
        _time: u32,
    ) {
        // 只要窗口本身的位置变化，不要菜单/控件内部的
        if event != EVENT_OBJECT_LOCATIONCHANGE || id_object != OBJID_WINDOW {
            return;
        }
        if hwnd as isize != TARGET.load(Ordering::Relaxed) {
            return;
        }
        // 回调属于主线程（WINEVENT_OUTOFCONTEXT），这里只是设个重绘标志
        if let Ok(guard) = CTX.lock() {
            if let Some(ctx) = guard.as_ref() {
                ctx.request_repaint();
            }
        }
    }

    /// 开始盯住某个窗口。重复调用会先卸掉旧的 hook。
    pub fn watch(ctx: &egui::Context, hwnd: *mut core::ffi::c_void, pid: u32) {
        unwatch();
        if let Ok(mut guard) = CTX.lock() {
            *guard = Some(ctx.clone());
        }
        TARGET.store(hwnd as isize, Ordering::Relaxed);

        let hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                std::ptr::null_mut(),
                Some(on_event),
                pid,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        HOOK.store(hook as isize, Ordering::Relaxed);
    }

    pub fn unwatch() {
        TARGET.store(0, Ordering::Relaxed);
        let hook = HOOK.swap(0, Ordering::Relaxed) as HWINEVENTHOOK;
        if !hook.is_null() {
            unsafe { UnhookWinEvent(hook) };
        }
    }
}

#[cfg(not(windows))]
mod follow {
    pub fn watch(_ctx: &egui::Context, _hwnd: *mut core::ffi::c_void, _pid: u32) {}
    pub fn unwatch() {}
}

// ============================================================
// 窗口外壳处理：去 DWM 描边、裁圆
//
// 这段和 sys-orb 是同一套做法。裁圆的第二个作用是让四个角的
// 透明区域不拦截鼠标 —— 否则图钉旁边一圈方形热区会挡住桌面。
// ============================================================

// ============================================================
// 启动诊断
//
// 打不开画面的问题，如果只看现象只能靠猜；把关键事实落到文件里，
// 一眼就能看出是「没拿到窗口句柄」还是「拿到了但样式没改掉」。
// ============================================================

fn diag_path() -> std::path::PathBuf {
    match std::env::var("APPDATA") {
        Ok(p) => std::path::PathBuf::from(p).join("win-pin").join("boot.txt"),
        Err(_) => std::path::PathBuf::from("win-pin-boot.txt"),
    }
}

fn diag_reset() {
    let p = diag_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&p, "");
}

fn diag(msg: &str) {
    use std::io::Write;
    let p = diag_path();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{}", msg);
    }
}

fn hwnd_of(frame: &eframe::Frame) -> Option<*mut core::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = frame.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(w) => Some(w.hwnd.get() as *mut core::ffi::c_void),
        _ => None,
    }
}

#[cfg(windows)]
fn strip_border(frame: &eframe::Frame) -> bool {
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_BORDER,
        WS_CAPTION, WS_DLGFRAME, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_EX_WINDOWEDGE,
        WS_SYSMENU, WS_THICKFRAME,
    };

    let Some(hwnd) = hwnd_of(frame) else {
        diag("strip: 拿不到窗口句柄，下一帧再试");
        return false;
    };

    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_DONOTROUND: u32 = 1;
    const DWMWA_NCRENDERING_POLICY: u32 = 2;
    const DWMNCRP_DISABLED: u32 = 1;

    unsafe {
        let no_border: u32 = DWMWA_COLOR_NONE;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as _,
            &no_border as *const u32 as *const core::ffi::c_void,
            4,
        );

        let no_round: u32 = DWMWCP_DONOTROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as _,
            &no_round as *const u32 as *const core::ffi::c_void,
            4,
        );

        let nc_off: u32 = DWMNCRP_DISABLED;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY as _,
            &nc_off as *const u32 as *const core::ffi::c_void,
            4,
        );

        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let style_mask =
            (WS_CAPTION | WS_THICKFRAME | WS_BORDER | WS_DLGFRAME | WS_SYSMENU) as isize;
        let had_style = style & style_mask != 0;
        if had_style {
            SetWindowLongPtrW(hwnd, GWL_STYLE, style & !style_mask);
        }

        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let ex_mask = (WS_EX_CLIENTEDGE | WS_EX_WINDOWEDGE | WS_EX_DLGMODALFRAME) as isize;
        let had_ex = ex & ex_mask != 0;
        if had_ex {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex & !ex_mask);
        }

        // 只有真改过样式才让 DWM 重算框架；无条件调用会每帧闪一次
        if had_style || had_ex {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            diag(&format!(
                "strip: style 0x{:08X} -> 0x{:08X}",
                style as u32,
                GetWindowLongPtrW(hwnd, GWL_STYLE) as u32
            ));
        }

        // 撤掉 winit 给透明窗口设的「玻璃框架」扩展 —— 它会让 DWM 在客户区
        // 顶端画一条 1px 线（sys-orb 上也出现过同样的一条）。
        // 关键在于：winit 在窗口真正显示之后会再打开一次，所以这行必须
        // 反复执行，只在首帧做一次是压不住的。
        extend_frame_reset(hwnd);
    }

    true
}

#[cfg(windows)]
unsafe fn extend_frame_reset(hwnd: *mut core::ffi::c_void) {
    #[repr(C)]
    struct Margins {
        l: i32,
        r: i32,
        t: i32,
        b: i32,
    }
    type PfnExtend = unsafe extern "system" fn(*mut core::ffi::c_void, *const Margins) -> i32;

    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

    let lib = GetModuleHandleA(b"dwmapi.dll\0".as_ptr());
    if lib.is_null() {
        return;
    }
    let proc = GetProcAddress(lib, b"DwmExtendFrameIntoClientArea\0".as_ptr());
    let Some(addr) = proc else {
        return;
    };
    let f: PfnExtend = unsafe { core::mem::transmute(addr) };
    let m = Margins {
        l: 0,
        r: 0,
        t: 0,
        b: 0,
    };
    unsafe { f(hwnd, &m) };
}

#[cfg(not(windows))]
fn strip_border(_frame: &eframe::Frame) -> bool {
    true
}

/// 把窗口区域裁成「刚好裹住图钉」的窄椭圆。
///
/// 这个区域有两个作用：
///
/// 1. 去掉圆外那圈方形热区。窗口是 68×68，图钉本身只有 30×42，
///    不裁的话图钉旁边全是会吃点击的隐形方块。
/// 2. 顺带把 DWM 画在客户区第 0 行的那条 1px 边框线裁掉 ——
///    那条线只在窗口区域是整矩形时才露出来。
///
/// 之所以不用整窗的圆：图钉现在是钉在目标窗口右上角的，整窗椭圆会
/// 盖住标题栏的关闭按钮，用户就点不到叉了。收窄到图钉轮廓之后，
/// 侵入窗口内部的部分只在针尖那 3px 高度上，宽度还从 15px 收到 0。
#[cfg(windows)]
fn clip_hotspot(frame: &eframe::Frame, ppp: f32) -> bool {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateEllipticRgn, CreateRectRgn, DeleteObject, GetWindowRgn, SetWindowRgn,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

    let Some(hwnd) = hwnd_of(frame) else {
        return false;
    };

    let mut rc: RECT = unsafe { std::mem::zeroed() };
    let got = unsafe { GetClientRect(hwnd, &mut rc) };
    let mut cw = if got != 0 { rc.right - rc.left } else { 0 };
    let mut ch = if got != 0 { rc.bottom - rc.top } else { 0 };

    if cw <= 8 || ch <= 8 {
        cw = (PANE * ppp).round() as i32;
        ch = cw;
    }

    // 图钉图形在局部坐标里是 x∈[-11,11]、y∈[-18,17]；钉住时会左倾 0.2rad，
    // 包围盒略微变大，所以留到 15×21 才够裹住。
    let scale = ch as f32 / PANE;
    let rx = 15.0 * scale;
    let ry = 21.0 * scale;
    let cx = cw as f32 * 0.5;
    let cy = ch as f32 * 0.5 - scale;

    let rgn = unsafe {
        CreateEllipticRgn(
            (cx - rx).round() as i32,
            (cy - ry).round() as i32,
            (cx + rx).round() as i32,
            (cy + ry).round() as i32,
        )
    };
    if rgn.is_null() {
        return false;
    }

    if unsafe { SetWindowRgn(hwnd, rgn, 1) } == 0 {
        unsafe { DeleteObject(rgn) };
        return false;
    }

    // 读回确认：0=ERROR 1=NULLREGION 2=SIMPLEREGION 3=COMPLEXREGION
    let probe = unsafe { CreateRectRgn(0, 0, 1, 1) };
    let kind = unsafe { GetWindowRgn(hwnd, probe) };
    unsafe { DeleteObject(probe) };

    if kind < 1 {
        diag(&format!("region: 设置失败 client={}x{} kind={}", cw, ch, kind));
    }

    kind >= 1
}

#[cfg(not(windows))]
fn clip_hotspot(_frame: &eframe::Frame, _ppp: f32) -> bool {
    true
}

// ============================================================
// 绘制
// ============================================================

fn shade(c: Color32, k: f32) -> Color32 {
    Color32::from_rgb(
        (c.r() as f32 * k).clamp(0.0, 255.0) as u8,
        (c.g() as f32 * k).clamp(0.0, 255.0) as u8,
        (c.b() as f32 * k).clamp(0.0, 255.0) as u8,
    )
}

fn rot(v: Vec2, a: f32) -> Vec2 {
    let (s, c) = a.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

fn ellipse_pts(c: Vec2, rx: f32, ry: f32, n: usize) -> Vec<Vec2> {
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            Vec2::new(c.x + a.cos() * rx, c.y + a.sin() * ry)
        })
        .collect()
}

/// tilt 0 = 未钉（直立灰蓝），1 = 已钉（微微左倾的红色）
///
/// 钉住时倾角刻意做得很小：图钉现在是扎在目标窗口的上沿上，
/// 倾得太厉害会显得是「歪在外面」，而不是「钉进去了」。
fn paint_pin(ui: &egui::Ui, tilt: f32, hover: f32) {
    let p = ui.painter();
    let center = ui.max_rect().center() + Vec2::new(0.0, -2.0 * tilt);

    let base = if tilt > 0.5 { C_PINNED } else { C_IDLE };
    let base = shade(base, 1.0 + 0.16 * hover);
    let ang = -0.20 * tilt;

    // 局部坐标 → 屏幕坐标
    let tf = |v: Vec2| center + rot(v, ang);

    // 针身：从帽底一路收到尖端
    let stem = vec![
        tf(Vec2::new(-4.2, -11.0)),
        tf(Vec2::new(4.2, -11.0)),
        tf(Vec2::new(2.6, 6.0)),
        tf(Vec2::new(0.0, 17.0)),
        tf(Vec2::new(-2.6, 6.0)),
    ];
    p.add(Shape::convex_polygon(stem, shade(base, 0.82), Stroke::NONE));

    // 针身左侧一条细高光，让金属感出来
    p.line_segment(
        [tf(Vec2::new(-1.5, -9.0)), tf(Vec2::new(-0.5, 11.0))],
        Stroke::new(1.1, shade(base, 1.25)),
    );

    // 帽：一个压扁的椭圆，代表俯视看到的圆盘
    let head: Vec<Pos2> = ellipse_pts(Vec2::new(0.0, -13.5), 11.0, 4.8, 28)
        .into_iter()
        .map(|v| center + rot(v, ang))
        .collect();
    p.add(Shape::convex_polygon(head, base, Stroke::NONE));

    // 帽顶高光
    let gloss: Vec<Pos2> = ellipse_pts(Vec2::new(0.0, -15.2), 7.0, 2.2, 24)
        .into_iter()
        .map(|v| center + rot(v, ang))
        .collect();
    p.add(Shape::convex_polygon(gloss, shade(base, 1.42), Stroke::NONE));

    // 钉住时，针尖处点一小圈暖光，表示「已经扎进去了」
    if tilt > 0.05 {
        let tip = tf(Vec2::new(0.0, 18.5));
        let glow = Color32::from_rgba_unmultiplied(
            C_PINNED.r(),
            C_PINNED.g(),
            C_PINNED.b(),
            (70.0 * tilt) as u8,
        );
        p.add(Shape::convex_polygon(
            ellipse_pts(Vec2::ZERO, 4.0 * tilt, 1.6 * tilt, 16)
                .into_iter()
                .map(|v| tip + v)
                .collect(),
            glow,
            Stroke::NONE,
        ));
    }
}

// ============================================================
// 位置持久化：一行文本，不引入任何序列化依赖
// ============================================================

fn pos_file() -> std::path::PathBuf {
    match std::env::var("APPDATA") {
        Ok(p) => std::path::PathBuf::from(p).join("win-pin").join("pos.txt"),
        Err(_) => std::path::PathBuf::from("win-pin-pos.txt"),
    }
}

fn load_pos() -> Option<Pos2> {
    let s = std::fs::read_to_string(pos_file()).ok()?;
    let mut it = s.split_whitespace();
    let x: f32 = it.next()?.parse().ok()?;
    let y: f32 = it.next()?.parse().ok()?;
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    Some(Pos2::new(x, y))
}

fn save_pos(p: Pos2) {
    let f = pos_file();
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(f, format!("{:.1} {:.1}", p.x, p.y));
}

// ============================================================
// 屏幕右下角的位置（单位：points，和 OuterPosition 一致）
// ============================================================

fn home_pos(ctx: &egui::Context) -> Pos2 {
    let sz = screen_size_pt(ctx);
    Pos2::new(sz.x - PANE - MARGIN, sz.y - PANE - MARGIN)
}

fn screen_size_pt(ctx: &egui::Context) -> Vec2 {
    let from_viewport = ctx.input(|i| i.viewport().monitor_size);
    match from_viewport {
        Some(m) if m.x > 200.0 && m.y > 200.0 => m,
        _ => {
            let (w, h) = w32::screen_size();
            let ppp = ctx.pixels_per_point().max(0.1);
            Vec2::new(w as f32 / ppp, h as f32 / ppp)
        }
    }
}

// ============================================================
// 应用
// ============================================================

struct Pin {
    home: Option<Pos2>,
    pos_done: bool,
    border_fixed: bool,
    /// 当前被置顶的窗口句柄（存成整数方便跨平台占位）
    pinned: Option<isize>,
    /// 拖动开始时的鼠标屏幕坐标，用来区分「单击」和「拖动」
    drag_from: Option<(i32, i32)>,
    /// 被钉窗口上一次的屏幕矩形（物理像素）。用来判断它有没有动。
    last_rect: Option<(i32, i32, i32, i32)>,
    /// 系统拖动进行中：这时候位置由鼠标决定，绝不能再发定位命令
    dragging: bool,
    tilt: f32,
    hover: f32,
    /// 被钉窗口的进程名，只在钉住时用来做菜单回显
    target_name: String,
    /// 拖动落到空白处后，等下一帧（位置已同步）再把新位置记为家
    capture_home: bool,
    /// 启动预热帧计数：见 ui() 里的说明
    warmup: u8,
}

impl Default for Pin {
    fn default() -> Self {
        Self {
            home: None,
            pos_done: false,
            border_fixed: false,
            pinned: None,
            drag_from: None,
            last_rect: None,
            dragging: false,
            tilt: 0.0,
            hover: 0.0,
            target_name: String::new(),
            capture_home: false,
            warmup: 0,
        }
    }
}

/// 图钉骑在目标窗口右上角时自己的位置（points）。
///
/// 针尖在局部坐标里是 y=+17，窗口中心在 `pos + PANE/2`。
/// 要让针尖正好落在窗口上沿，窗口中心就得在 top-17，于是
/// 窗口左上角在 top-51。水平方向让图钉中心几乎压在窗口右边缘上
/// （只往里收 2px），这样既看得出是「夹在角上」，又只压住标题栏
/// 最右端十来像素，不会盖住关闭按钮。
fn pin_pos_for_window(ctx: &egui::Context, rc: (i32, i32, i32, i32)) -> Pos2 {
    let ppp = ctx.pixels_per_point().max(0.1);
    let (left, top, w, _h) = rc;
    let right_pt = (left + w) as f32 / ppp;
    let top_pt = top as f32 / ppp;
    let sz = screen_size_pt(ctx);

    let clamp = |p: Pos2| {
        Pos2::new(
            p.x.clamp(0.0, (sz.x - PANE).max(0.0)),
            p.y.clamp(0.0, (sz.y - PANE).max(0.0)),
        )
    };

    // 首选：坐在窗口上沿右侧，针尖扎进上沿
    if top_pt >= 51.0 {
        return clamp(Pos2::new(right_pt - 32.0, top_pt - 51.0));
    }

    // 窗口贴着屏幕上沿（最大化就是这种）：上面没地方放，硬塞会被屏幕裁掉
    // 半个图钉。改挂到窗口右侧外面，至少完整可见、也不遮窗口内容。
    let side = Pos2::new(right_pt + 2.0, top_pt + 4.0);
    if side.x + PANE <= sz.x {
        return clamp(side);
    }

    // 右侧也顶到屏幕边了：只能压回窗口里，优先保住「在右上角」这个位置感
    clamp(Pos2::new(right_pt - 32.0, top_pt + 4.0))
}

impl Pin {
    fn target(&self) -> Option<*mut core::ffi::c_void> {
        self.pinned.map(|h| h as *mut core::ffi::c_void)
    }

    /// 只松开：取消目标窗口的置顶并清掉跟踪状态，图钉原地不动。
    ///
    /// 和 unpin 分开是因为「拖到空白处换个家」这个动作要求图钉留在松手的
    /// 位置，如果顺手飞回窝，紧接着记下的新家就变成旧窝了。
    fn release(&mut self) {
        if let Some(old) = self.pinned.take() {
            w32::set_pinned(old as *mut core::ffi::c_void, false);
        }
        follow::unwatch();
        self.last_rect = None;
        self.target_name.clear();
    }

    /// 松开，并把图钉送回它的窝。单击、右键菜单、目标窗口关闭都走这条。
    fn unpin(&mut self, ctx: &egui::Context) {
        self.release();
        self.fly_home(ctx);
    }

    fn fly_home(&mut self, ctx: &egui::Context) {
        if let Some(h) = self.home {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(h));
        }
    }

    /// 把图钉贴到目标窗口的右上角，并记下这一帧的位置基准。
    fn snap_to_target(&mut self, ctx: &egui::Context) {
        let Some(h) = self.target() else {
            return;
        };
        let Some(rc) = w32::window_rect(h) else {
            return;
        };
        self.last_rect = Some(rc);
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pin_pos_for_window(ctx, rc)));
    }

    fn pin_at_cursor(&mut self, frame: &eframe::Frame, ctx: &egui::Context) -> bool {
        let Some(me) = hwnd_of(frame) else {
            return false;
        };
        let Some(target) = w32::under_cursor(me) else {
            return false;
        };

        // 一次只钉一个：钉新的自动松开旧的，避免桌面上出现多个互不相让的置顶窗口
        if let Some(old) = self.pinned {
            if old != target as isize {
                w32::set_pinned(old as *mut core::ffi::c_void, false);
            }
        }

        if w32::set_pinned(target, true) {
            self.pinned = Some(target as isize);
            self.target_name = w32::proc_name(target);
            // 让系统在目标窗口移动时回调（而不是自己定时轮询）
            follow::watch(ctx, target, w32::pid_of(target));
        } else {
            return false;
        }

        // 目标窗口刚被提到最前，图钉会被它盖住（尤其是全屏或大窗口）。
        // 立刻把图钉自己重新提到最前，保证下一次还能点到。
        w32::raise(me);
        true
    }

    /// 每帧维护「图钉贴在窗口角上」这件事：目标窗口动了就跟着动。
    ///
    /// 返回 true 表示这一帧处于跟随态（还在盯着某个窗口），
    /// 调用方据此决定轮询节拍：只有跟随态才需要高频醒来。
    fn follow_target(&mut self, ctx: &egui::Context, frame: &eframe::Frame) -> bool {
        let Some(h) = self.target() else {
            return false;
        };

        if !w32::alive(h) {
            // 窗口被关掉了：松开，图钉飞回窝。这是最自然的收尾。
            self.unpin(ctx);
            return false;
        }

        // 最小化时没有「角」可钉：图钉回窝待着，但保持已钉状态，
        // 等窗口恢复再自己贴回去。
        if w32::minimized(h) {
            if self.last_rect.take().is_some() {
                self.fly_home(ctx);
            }
            return true;
        }

        // 用户正拖着图钉：位置归鼠标管，这时插一脚会让它抖
        if self.dragging {
            return true;
        }

        let Some(rc) = w32::window_rect(h) else {
            return true;
        };

        // 目标窗口会自己把置顶改回去 —— 实测 .NET / WinForms 的窗口在
        // 被激活时就会丢掉 WS_EX_TOPMOST（Z 序仍在前面，但不再钉住）。
        // 所以不能"设一次就信它"，每次兜底检查时都确认一遍。
        if !w32::is_topmost(h) {
            w32::set_pinned(h, true);
        }

        if Some(rc) != self.last_rect {
            self.last_rect = Some(rc);
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pin_pos_for_window(ctx, rc)));
            ctx.request_repaint();
        }

        // 图钉必须一直压在目标窗口上面：两者都是 topmost，目标窗口被激活时
        // 会排到前面去。这里每次兜底检查都把自己提回最前，代价是一次
        // 不带移动/缩放的 SetWindowPos，几乎为零。
        if let Some(me) = hwnd_of(frame) {
            w32::raise(me);
        }
        true
    }
}

impl eframe::App for Pin {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // ---- 启动预热：前 90 帧强制重绘 ----
        // 窗口刚创建时 GL surface 往往还没就绪，首帧画的内容会整个丢掉。
        // 本组件静止时不请求重绘，于是首帧一丢就永久停在系统白底上。
        // sys-orb 每 2 秒采样一次、顺带重绘，所以从没暴露这个问题。
        if self.warmup < 90 {
            self.warmup += 1;
            ctx.request_repaint();
            if self.warmup == 1 || self.warmup == 45 || self.warmup == 90 {
                let r = ui.max_rect();
                diag(&format!(
                    "frame {}: max_rect=({:.0},{:.0},{:.0},{:.0}) ppp={:.2}",
                    self.warmup,
                    r.min.x,
                    r.min.y,
                    r.max.x,
                    r.max.y,
                    ctx.pixels_per_point()
                ));
            }
        }

        // ---- 去描边：只做一次 ----
        // 千万不要每帧重做：清完样式必须 SetWindowPos(SWP_FRAMECHANGED) 让
        // DWM 重算框架，而这个调用每帧来一次会把 GL 画面反复刷掉 —— 实测
        // 表现就是整个窗口退回系统白底。
        if self.warmup == 1 {
            self.border_fixed = strip_border(frame);
        }

        // ---- 窗口真正显示后，再撤一次玻璃框架 ----
        // winit 在窗口可见之后会把透明窗口的框架扩展重新打开，DWM 于是又在
        // 客户区顶端画一条 1px 中性灰的线（和 sys-orb 当初那条一模一样）。
        // 这里只在几个时间点补刀，且只动 DWM 属性、不动窗口样式，不触发重排。
        if matches!(self.warmup, 3 | 8 | 20 | 45 | 90) {
            #[cfg(windows)]
            {
                if let Some(h) = hwnd_of(frame) {
                    unsafe { extend_frame_reset(h) };
                }
            }
        }

        // ---- 首次定位：上次的位置，没有就用右下角 ----
        if !self.pos_done {
            self.pos_done = true;
            let p = load_pos().unwrap_or_else(|| home_pos(&ctx));
            self.home = Some(p);
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(p));
        }

        // 首帧发的定位命令可能因为窗口还没就绪而被丢掉，预热到第 5 帧补一次
        if self.warmup == 5 {
            if let Some(p) = self.home {
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(p));
            }
        }

        // ---- 圆形裁剪：必须放在定位之后，而且要持续巡检 ----
        // 移动窗口会让 winit 把窗口区域重置回矩形，DWM 随即在客户区第 0 行
        // 画出一条 1px 深灰边框线 —— 就是那条"怎么都去不掉"的线。
        // 实测把一个只有左上 20x20 的矩形设成窗口区域，那条线立刻被裁掉，
        // 说明裁剪本身有效，问题在于它会被重置、而本组件静止时不重绘，
        // 于是再也没有机会补上。
        //
        // 现在图钉会跟着目标窗口一起移动，每次移动都可能把区域冲掉，
        // 所以这里必须无条件、每帧重设：不能靠"读回来判断是否已经裁过"——
        // 实测 GetWindowRgn 对被重置的矩形也报 COMPLEXREGION，判不出来。
        #[cfg(windows)]
        {
            if hwnd_of(frame).is_some() {
                clip_hotspot(frame, ctx.pixels_per_point());
            }
        }

        // ---- 上一帧把图钉拖到了空白处，这一帧位置已同步，记成新家 ----
        // 预热期间不接受：避免启动瞬间的误判把默认位置写坏。
        if self.capture_home && self.warmup >= 90 {
            self.capture_home = false;
            if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
                self.home = Some(r.min);
                save_pos(r.min);
            }
        }

        // ---- 交互 ----
        let rect = ui.max_rect();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());

        let want_hover = if resp.hovered() { 1.0f32 } else { 0.0 };
        if (self.hover - want_hover).abs() > 0.004 {
            self.hover += (want_hover - self.hover) * 0.28;
            ctx.request_repaint_after(Duration::from_millis(16));
        } else {
            self.hover = want_hover;
        }

        if resp.drag_started() {
            self.drag_from = w32::cursor();
            self.dragging = true;
            // 交给系统做拖动：比自己在每帧发 OuterPosition 跟手得多
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        if resp.drag_stopped() {
            self.dragging = false;

            // 用鼠标位移判断，而不是窗口位置：系统拖动结束后 egui 的
            // outer_rect 还停在旧值上，拿它判会把拖动误看成单击。
            let moved = match (self.drag_from.take(), w32::cursor()) {
                (Some(a), Some(b)) => {
                    let dx = (a.0 - b.0) as f32;
                    let dy = (a.1 - b.1) as f32;
                    (dx * dx + dy * dy).sqrt() > 6.0
                }
                _ => false,
            };

            if moved {
                if self.pin_at_cursor(frame, &ctx) {
                    // 钉住了：图钉立刻贴到那个窗口的右上角，之后跟着窗口走。
                    // 这是整套交互的核心 —— 钉在哪里，就留在哪里。
                    self.snap_to_target(&ctx);
                } else if self.pinned.is_some() {
                    // 本来钉着某个窗口，这次拖到了空白处 → 解钉 + 换家
                    self.release();
                    self.capture_home = true;
                } else {
                    // 落在空白处：把当前位置当成新的家。
                    // 「把图钉挪到顺手的地方」也顺带有了，不用额外做菜单项。
                    self.capture_home = true;
                }
            } else {
                // 单击：解钉，图钉飞回窝
                self.unpin(&ctx);
            }
            ctx.request_repaint();
        }

        // 纯单击（没有拖动）：解钉，图钉飞回窝。
        // 不能只靠 drag_started/drag_stopped —— egui 的 drag 要位移超过阈值
        // 才成立，手不抖的一下点击根本不会产生 drag，只会产生 clicked。
        // 之前只写在 drag_stopped 里，结果单击完全没反应。
        if resp.clicked() {
            self.unpin(&ctx);
            ctx.request_repaint();
        }

        if resp.secondary_clicked() {
            if let Some(h) = hwnd_of(frame) {
                let name = if self.pinned.is_some() {
                    Some(self.target_name.as_str())
                } else {
                    None
                };
                match w32::popup_menu(h, name) {
                    1 => {
                        self.unpin(&ctx);
                        ctx.request_repaint();
                    }
                    2 => ctx.send_viewport_cmd(ViewportCommand::Close),
                    _ => {}
                }
            }
        }

        // ---- 跟随：目标窗口移动 / 缩放 / 最小化 / 关闭 ----
        let following = self.follow_target(&ctx, frame);

        // 目标窗口一动，系统会通过 WinEventHook 把我们叫醒，所以这里的
        // 定时器只是兜底（防某些环境下收不到事件），一秒一次足够。
        // 不跟随时回到 2 秒，常驻开销归零。
        let wait = if following {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(2)
        };
        ctx.request_repaint_after(wait);

        // ---- 倾斜动画 ----
        let target_tilt = if self.pinned.is_some() { 1.0 } else { 0.0 };
        if (self.tilt - target_tilt).abs() > 0.004 {
            self.tilt += (target_tilt - self.tilt) * 0.22;
            ctx.request_repaint_after(Duration::from_millis(16));
        } else {
            self.tilt = target_tilt;
        }

        paint_pin(ui, self.tilt, self.hover);
    }
}

fn main() -> eframe::Result<()> {
    diag_reset();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([PANE, PANE])
            .with_min_inner_size([PANE, PANE])
            .with_max_inner_size([PANE, PANE])
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_title("置顶图钉"),
        ..Default::default()
    };

    eframe::run_native(
        "置顶图钉",
        options,
        Box::new(|_cc| Ok(Box::new(Pin::default()))),
    )
}
