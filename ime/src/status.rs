//! 状态条 —— 照青简的 ui/status：屏幕右下角一条可拖动的小条，
//! 两格：「中/英」（点击切换）与齿轮 ⚙（点击弹菜单）。配色/字号/间距与候选窗同一套。
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;

static ENGLISH: AtomicBool = AtomicBool::new(false);
static mut WND: HWND = HWND(0);
static mut BG: HBRUSH = HBRUSH(0);
static mut CELL1_RIGHT: i32 = 0;
static mut DRAGGING: bool = false;
static mut MOVED: bool = false;
static mut DRAG_START: POINT = POINT { x: 0, y: 0 };
static mut DRAG_ORIGIN: POINT = POINT { x: 0, y: 0 };

const PAD: i32 = 8;
const ROW_PAD: i32 = 4;
const RADIUS: i32 = 8;
const EDGE_GAP: i32 = 14;

pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

fn sc(px: i32) -> i32 {
    let d = unsafe { crate::cand::dpi() };
    (px * d) / 96
}

fn colors() -> (COLORREF, COLORREF, COLORREF) {
    if crate::cand::is_dark() {
        (rgb(0xf5, 0xf5, 0xf7), rgb(0x8e, 0x8e, 0x93), rgb(0x2a, 0x2a, 0x2c))
    } else {
        (rgb(0x1d, 0x1d, 0x1f), rgb(0xa0, 0xa0, 0xa6), rgb(0xf8, 0xf8, 0xf8))
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

fn font(px: i32, face: PCWSTR) -> HFONT {
    unsafe {
        CreateFontW(-sc(px), 0, 0, 0, 400, 0, 0, 0,
            DEFAULT_CHARSET.0 as u32, OUT_TT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32, (VARIABLE_PITCH.0 | FF_DONTCARE.0) as u32, face)
    }
}

fn measure(dc: HDC, f: HFONT, s: &str) -> (i32, i32) {
    let u: Vec<u16> = s.encode_utf16().collect();
    let mut sz = SIZE::default();
    unsafe {
        SelectObject(dc, f);
        let _ = GetTextExtentPoint32W(dc, &u, &mut sz);
    }
    (sz.cx, sz.cy)
}

fn pos_file() -> String {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    format!("{base}\\Pangyi\\status.pos")
}

fn load_pos() -> Option<(i32, i32)> {
    let s = std::fs::read_to_string(pos_file()).ok()?;
    let mut it = s.trim().split(',');
    let x = it.next()?.trim().parse().ok()?;
    let y = it.next()?.trim().parse().ok()?;
    Some((x, y))
}

fn save_pos(h: HWND) {
    unsafe {
        let mut r = RECT::default();
        if GetWindowRect(h, &mut r).is_ok() {
            let _ = std::fs::create_dir_all(std::path::Path::new(&pos_file()).parent().unwrap());
            let _ = std::fs::write(pos_file(), format!("{},{}", r.left, r.top));
        }
    }
}

unsafe fn default_pos(w: i32, h: i32) -> (i32, i32) {
    let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let work = if GetMonitorInfoW(mon, &mut mi).as_bool() {
        mi.rcWork
    } else {
        RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) }
    };
    (work.right - sc(EDGE_GAP) - w, work.bottom - sc(EDGE_GAP) - h)
}

/// 两格宽度：中文/英文格 + 齿轮格。
unsafe fn cell_widths(dc: HDC) -> (i32, i32, i32) {
    let f_idx = font(11, w!("Microsoft YaHei UI"));
    let f_sym = font(15, w!("Segoe UI Symbol"));
    let mode = if english() { "英" } else { "中" };
    let a = measure(dc, f_idx, mode).0 + sc(PAD) * 2;
    let b = measure(dc, f_sym, "⚙").0 + sc(PAD) * 2;
    let line_h = measure(dc, f_idx, "x").1;
    let _ = DeleteObject(f_idx);
    let _ = DeleteObject(f_sym);
    (a.max(sc(30)), b.max(sc(30)), line_h)
}

unsafe fn layout_size(h: HWND) -> (i32, i32) {
    let dc = GetDC(h);
    let (a, b, line_h) = cell_widths(dc);
    let _ = ReleaseDC(h, dc);
    (a + b, line_h.max(sc(15)) + sc(ROW_PAD) * 2 + sc(4))
}

unsafe extern "system" fn proc_status(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let (text_color, faint, bg_color) = colors();
            let bg = CreateSolidBrush(bg_color);
            FillRect(dc, &rc, bg);
            let _ = DeleteObject(bg);
            SetBkMode(dc, TRANSPARENT);

            let f_idx = font(11, w!("Microsoft YaHei UI"));
            let f_sym = font(15, w!("Segoe UI Symbol"));
            let (w1, w2, line_h) = cell_widths(dc);
            CELL1_RIGHT = w1;
            let base_y = (rc.bottom - line_h) / 2;

            let mode = if english() { "英" } else { "中" };
            let (mw, _) = measure(dc, f_idx, mode);
            SelectObject(dc, f_idx);
            SetTextColor(dc, text_color);
            let u: Vec<u16> = mode.encode_utf16().collect();
            TextOutW(dc, (w1 - mw) / 2, base_y, &u);

            let sep = sc(4);
            let brush = CreateSolidBrush(faint);
            let bar = RECT { left: w1, top: sep, right: w1 + 1, bottom: rc.bottom - sep };
            FillRect(dc, &bar, brush);
            let _ = DeleteObject(brush);

            let (gw, _) = measure(dc, f_sym, "⚙");
            SelectObject(dc, f_sym);
            SetTextColor(dc, faint);
            let gu: Vec<u16> = "⚙".encode_utf16().collect();
            TextOutW(dc, w1 + (w2 - gw) / 2, (rc.bottom - measure(dc, f_sym, "⚙").1) / 2, &gu);

            let _ = DeleteObject(f_idx);
            let _ = DeleteObject(f_sym);
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let mut r = RECT::default();
            let _ = GetWindowRect(h, &mut r);
            DRAGGING = true;
            MOVED = false;
            DRAG_START = p;
            DRAG_ORIGIN = POINT { x: r.left, y: r.top };
            SetCapture(h);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if DRAGGING {
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                let dx = p.x - DRAG_START.x;
                let dy = p.y - DRAG_START.y;
                if dx.abs() > 3 || dy.abs() > 3 {
                    MOVED = true;
                }
                if MOVED {
                    let _ = SetWindowPos(h, HWND_TOPMOST, DRAG_ORIGIN.x + dx, DRAG_ORIGIN.y + dy, 0, 0,
                        SWP_NOSIZE | SWP_NOACTIVATE);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if DRAGGING {
                DRAGGING = false;
                let _ = ReleaseCapture();
                if MOVED {
                    save_pos(h);
                } else {
                    let mut p = POINT::default();
                    let _ = GetCursorPos(&mut p);
                    let mut r = RECT::default();
                    let _ = GetWindowRect(h, &mut r);
                    let rel = p.x - r.left;
                    if rel < CELL1_RIGHT {
                        ENGLISH.store(!english(), Ordering::Relaxed);
                        let _ = InvalidateRect(h, None, TRUE);
                    } else {
                        gear_menu(h);
                    }
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

unsafe fn gear_menu(h: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let a = wide("关于旁译");
    let b = wide("恢复默认位置");
    let _ = AppendMenuW(menu, MF_STRING, 1, PCWSTR(a.as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, 2, PCWSTR(b.as_ptr()));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(h);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, h, None);
    let _ = DestroyMenu(menu);
    match cmd.0 {
        1 => {
            let text = wide("旁译 · 中文 → 俄语输入法\n\nWin + 空格 切到「旁译」，敲拼音选字，候选窗里带俄语译词。\n空格上屏第一个，数字 1-9 选第几个，Esc 取消。\n\n点击左边的「中/英」可以在中文输入和直接输入英文之间切换。");
            let cap = wide("关于旁译");
            let _ = MessageBoxW(h, PCWSTR(text.as_ptr()), PCWSTR(cap.as_ptr()), MB_OK);
        }
        2 => {
            let (w, hh) = layout_size(h);
            let (x, y) = default_pos(w, hh);
            let _ = SetWindowPos(h, HWND_TOPMOST, x, y, w, hh, SWP_NOACTIVATE);
            save_pos(h);
        }
        _ => {}
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub unsafe fn init() {
    if WND.0 != 0 {
        return;
    }
    crate::cand::detect_theme();
    let (_, _, bg_color) = colors();
    BG = CreateSolidBrush(bg_color);
    let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
    let cls = wide("PangyiStatus");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(proc_status),
        lpszClassName: PCWSTR(cls.as_ptr()),
        hInstance: hinst.into(),
        hbrBackground: BG,
        ..Default::default()
    };
    RegisterClassW(&wc);
    let title = wide("旁译状态");
    WND = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
        PCWSTR(cls.as_ptr()), PCWSTR(title.as_ptr()), WS_POPUP,
        0, 0, sc(80), sc(28), None, None, hinst, None);
}

pub unsafe fn show() {
    init();
    if WND.0 == 0 {
        return;
    }
    let (w, h) = layout_size(WND);
    let (x, y) = match load_pos() {
        Some((x, y)) => (x, y),
        None => default_pos(w, h),
    };
    let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, sc(RADIUS), sc(RADIUS));
    let _ = SetWindowRgn(WND, rgn, TRUE);
    let _ = SetWindowPos(WND, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    let _ = InvalidateRect(WND, None, TRUE);
}

pub unsafe fn hide() {
    if WND.0 != 0 {
        let _ = ShowWindow(WND, SW_HIDE);
    }
}