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
    pub fn popup_menu(hwnd: HWND) -> i32 {
        let mut pt = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut pt) };

        let unpin = wide("解除置顶");
        let quit = wide("退出");

        unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return 0;
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
    pub fn alive(_h: *mut c_void) -> bool {
        false
    }
    pub fn cursor() -> Option<(i32, i32)> {
        None
    }
    pub fn popup_menu(_h: *mut c_void) -> i32 {
        0
    }
    pub fn screen_size() -> (i32, i32) {
        (1920, 1080)
    }
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

#[cfg(windows)]
fn clip_circle(frame: &eframe::Frame, ppp: f32) -> bool {
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

    let rgn = unsafe { CreateEllipticRgn(0, 0, cw, ch) };
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
fn clip_circle(_frame: &eframe::Frame, _ppp: f32) -> bool {
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

/// tilt 0 = 未钉（直立灰蓝），1 = 已钉（左倾红色）
fn paint_pin(ui: &egui::Ui, tilt: f32, hover: f32) {
    let p = ui.painter();
    let center = ui.max_rect().center() + Vec2::new(0.0, -3.0 * tilt);

    let base = if tilt > 0.5 { C_PINNED } else { C_IDLE };
    let base = shade(base, 1.0 + 0.16 * hover);
    let ang = -0.58 * tilt;

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
    let from_viewport = ctx.input(|i| i.viewport().monitor_size);
    let sz = match from_viewport {
        Some(m) if m.x > 200.0 && m.y > 200.0 => m,
        _ => {
            let (w, h) = w32::screen_size();
            let ppp = ctx.pixels_per_point();
            Vec2::new(w as f32 / ppp, h as f32 / ppp)
        }
    };
    Pos2::new(sz.x - PANE - MARGIN, sz.y - PANE - MARGIN)
}

// ============================================================
// 应用
// ============================================================

struct Pin {
    home: Option<Pos2>,
    pos_done: bool,
    border_fixed: bool,
    region_done: bool,
    /// 当前被置顶的窗口句柄（存成整数方便跨平台占位）
    pinned: Option<isize>,
    /// 拖动开始时的鼠标屏幕坐标，用来区分「单击」和「拖动」
    drag_from: Option<(i32, i32)>,
    tilt: f32,
    hover: f32,
    next_check: f64,
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
            region_done: false,
            pinned: None,
            drag_from: None,
            tilt: 0.0,
            hover: 0.0,
            next_check: 0.0,
            capture_home: false,
            warmup: 0,
        }
    }
}

impl Pin {
    fn unpin(&mut self) {
        if let Some(old) = self.pinned.take() {
            w32::set_pinned(old as *mut core::ffi::c_void, false);
        }
    }

    fn pin_at_cursor(&mut self, frame: &eframe::Frame) -> bool {
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
        } else {
            return false;
        }

        // 目标窗口刚被提到最前，图钉会被它盖住（尤其是全屏或大窗口）。
        // 立刻把图钉自己重新提到最前，保证下一次还能点到。
        w32::set_pinned(me, true);
        true
    }
}

impl eframe::App for Pin {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);

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
        // 于是再也没有机会补上。改成低频巡检，发现不是椭圆就重设。
        #[cfg(windows)]
        {
            if hwnd_of(frame).is_some() {
                // 这里必须无条件重设，不能靠"读回来判断是否已经裁过"：
                // 实测 GetWindowRgn 对被 winit 重置的矩形也报 COMPLEXREGION，
                // 两种状态的返回值完全一样，判断不出来。而 SetWindowRgn 只是
                // 一次很轻的 GDI 调用，2 秒一次的开销可以忽略。
                self.region_done = clip_circle(frame, ctx.pixels_per_point());
            }
        }

        // 巡检节拍：2 秒一帧，开销可忽略（一次静态重绘），但换来区域不会丢
        ctx.request_repaint_after(Duration::from_secs(2));

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
            // 交给系统做拖动：比自己在每帧发 OuterPosition 跟手得多
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        if resp.drag_stopped() {
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
                if self.pin_at_cursor(frame) {
                    // 钉住了：图钉弹回原位，让「拖动」只表示「钉窗口」这一个意思
                    if let Some(h) = self.home {
                        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(h));
                    }
                } else {
                    // 落在空白处：没钉到任何窗口，那就把当前位置当成新的家。
                    // 这样「把图钉挪到顺手的地方」也顺带有了，不用额外做菜单项。
                    self.capture_home = true;
                }
            } else {
                self.unpin();
            }
            ctx.request_repaint();
        }

        if resp.secondary_clicked() {
            if let Some(h) = hwnd_of(frame) {
                match w32::popup_menu(h) {
                    1 => {
                        self.unpin();
                        ctx.request_repaint();
                    }
                    2 => ctx.send_viewport_cmd(ViewportCommand::Close),
                    _ => {}
                }
            }
        }

        // ---- 被钉的窗口关掉了就自动松开 ----
        if self.pinned.is_some() && now >= self.next_check {
            self.next_check = now + 1.0;
            if let Some(h) = self.pinned {
                let gone = !w32::alive(h as *mut core::ffi::c_void);
                if gone {
                    self.pinned = None;
                }
            }
        }
        if self.pinned.is_some() {
            let wait = (self.next_check - now).max(0.2) as f32;
            ctx.request_repaint_after(Duration::from_secs_f32(wait));
        }

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
