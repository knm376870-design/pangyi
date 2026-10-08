//! 设置面板 + 配置读写。
//!
//! 齿轮点开的那个小面板：候选个数 / 字号 / 排版 / 主题。
//! 配置存在 %LOCALAPPDATA%\Pangyi\settings.txt，候选窗启动时读一次。
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static MAX_CANDS: AtomicUsize = AtomicUsize::new(9);
static FONT_SCALE: AtomicI32 = AtomicI32::new(100);
static HORIZONTAL: AtomicBool = AtomicBool::new(false);
static THEME: AtomicI32 = AtomicI32::new(0); // 0 跟随系统 / 1 浅色 / 2 深色

static STANDALONE: AtomicBool = AtomicBool::new(false);
static mut WND: HWND = HWND(0);
static mut BG: HBRUSH = HBRUSH(0);

pub fn cfg_file() -> String {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    format!("{base}\\Pangyi\\settings.txt")
}

pub fn load() {
    // 老版本把排版单独存在 layout.txt，这里兼容读一次
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    if let Ok(s) = std::fs::read_to_string(format!("{base}\\Pangyi\\layout.txt")) {
        HORIZONTAL.store(s.trim().eq_ignore_ascii_case("h"), Ordering::Relaxed);
    }
    if let Ok(s) = std::fs::read_to_string(cfg_file()) {
        for line in s.lines() {
            let mut it = line.splitn(2, '=');
            let (k, v) = match (it.next(), it.next()) {
                (Some(k), Some(v)) => (k.trim(), v.trim()),
                _ => continue,
            };
            match k {
                "max" => if let Ok(n) = v.parse::<usize>() {
                    MAX_CANDS.store(n.clamp(3, 9), Ordering::Relaxed);
                },
                "scale" => if let Ok(n) = v.parse::<i32>() {
                    FONT_SCALE.store(n.clamp(80, 140), Ordering::Relaxed);
                },
                "layout" => HORIZONTAL.store(v.eq_ignore_ascii_case("h"), Ordering::Relaxed),
                "theme" => if let Ok(n) = v.parse::<i32>() {
                    THEME.store(n.clamp(0, 2), Ordering::Relaxed);
                },
                _ => {}
            }
        }
    }
}

pub fn save() {
    let path = cfg_file();
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = format!(
        "max={}\nscale={}\nlayout={}\ntheme={}\n",
        max_cands(), font_scale(),
        if horizontal() { "h" } else { "v" },
        THEME.load(Ordering::Relaxed)
    );
    let _ = std::fs::write(path, body);
}

pub fn max_cands() -> usize {
    MAX_CANDS.load(Ordering::Relaxed)
}
pub fn font_scale() -> i32 {
    FONT_SCALE.load(Ordering::Relaxed)
}
pub fn horizontal() -> bool {
    HORIZONTAL.load(Ordering::Relaxed)
}
pub fn set_horizontal(v: bool) {
    HORIZONTAL.store(v, Ordering::Relaxed);
    save();
}
/// 系统是深色时返回 true；设置里可以强制浅色/深色。
pub fn dark(system_dark: bool) -> bool {
    match THEME.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => system_dark,
    }
}

// ---------------- 面板 ----------------

fn sc(px: i32) -> i32 {
    let d = unsafe { crate::cand::dpi() };
    (px * d) / 96
}

fn colors() -> (COLORREF, COLORREF, COLORREF, COLORREF) {
    let dark = dark(crate::cand::is_dark());
    if dark {
        (rgb(0xf5, 0xf5, 0xf7), rgb(0xae, 0xae, 0xb2), rgb(0x2a, 0x2a, 0x2c), rgb(0x2f, 0x4d, 0x72))
    } else {
        (rgb(0x1d, 0x1d, 0x1f), rgb(0x6b, 0x6b, 0x70), rgb(0xf8, 0xf8, 0xf8), rgb(0xcf, 0xe4, 0xf9))
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

fn font(px: i32) -> HFONT {
    unsafe {
        CreateFontW(-sc(px), 0, 0, 0, 400, 0, 0, 0,
            DEFAULT_CHARSET.0 as u32, OUT_TT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32, (VARIABLE_PITCH.0 | FF_DONTCARE.0) as u32,
            w!("Microsoft YaHei UI"))
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

fn draw(dc: HDC, f: HFONT, color: COLORREF, x: i32, y: i32, s: &str) -> i32 {
    unsafe {
        SelectObject(dc, f);
        SetTextColor(dc, color);
        let u: Vec<u16> = s.encode_utf16().collect();
        TextOutW(dc, x, y, &u);
    }
    measure(dc, f, s).0
}

const ROW_H: i32 = 34;
const PAD: i32 = 16;

/// 每行的选项：标签 + 选项文本 + 动作号
fn rows() -> Vec<(&'static str, Vec<(&'static str, i32)>)> {
    vec![
        ("候选个数", vec![("5", 1), ("7", 2), ("9", 3)]),
        ("字号", vec![("小", 4), ("标准", 5), ("大", 6)]),
        ("排版", vec![("竖排", 7), ("横排", 8)]),
        ("主题", vec![("跟随系统", 9), ("浅色", 10), ("深色", 11)]),
    ]
}

/// 某个选项是不是当前选中的（用来画高亮）
fn is_current(act: i32) -> bool {
    let m = max_cands();
    let s = font_scale();
    let t = THEME.load(Ordering::Relaxed);
    match act {
        1..=3 => {
            let cur = if m <= 5 { 1 } else if m <= 7 { 2 } else { 3 };
            act == cur
        }
        4..=6 => {
            let cur = if s <= 90 { 4 } else if s >= 115 { 6 } else { 5 };
            act == cur
        }
        7 | 8 => act == if horizontal() { 8 } else { 7 },
        9..=11 => act == 9 + t,
        _ => false,
    }
}

unsafe extern "system" fn proc_settings(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let (text, faint, bg, hl) = colors();
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let brush = CreateSolidBrush(bg);
            FillRect(dc, &rc, brush);
            let _ = DeleteObject(brush);
            SetBkMode(dc, TRANSPARENT);

            let f_title = font(14);
            let f_text = font(12);
            draw(dc, f_title, text, sc(PAD), sc(12), "旁译 · 设置");

            let mut y = sc(48);
            for (label, opts) in rows() {
                draw(dc, f_text, faint, sc(PAD), y + sc(6), label);
                let mut x = sc(96);
                for (name, act) in opts {
                    let (w, hgt) = measure(dc, f_text, name);
                    let bw = w + sc(20);
                    let bh = hgt + sc(8);
                    let r = RECT { left: x, top: y, right: x + bw, bottom: y + bh };
                    let on = is_current(act);
                    let color = if on { hl } else { bg };
                    let rb = CreateSolidBrush(color);
                    let rgn = CreateRoundRectRgn(r.left, r.top, r.right, r.bottom, sc(8), sc(8));
                    FillRgn(dc, rgn, rb);
                    let _ = DeleteObject(rgn);
                    let _ = DeleteObject(rb);
                    draw(dc, f_text, if on { text } else { faint }, x + sc(10), y + sc(4), name);
                    x += bw + sc(8);
                }
                y += sc(ROW_H);
            }
            // 底部「关闭」按钮`n            let label = "关闭";`n            let (lw, lh) = measure(dc, f_text, label);`n            let bx = sc(340) - sc(PAD) - lw - sc(20);`n            let by = y + sc(6);`n            let br = RECT { left: bx, top: by, right: bx + lw + sc(20), bottom: by + lh + sc(8) };`n            let rb = CreateSolidBrush(hl);`n            let rgn = CreateRoundRectRgn(br.left, br.top, br.right, br.bottom, sc(8), sc(8));`n            FillRgn(dc, rgn, rb);`n            let _ = DeleteObject(rgn);`n            let _ = DeleteObject(rb);`n            draw(dc, f_text, text, bx + sc(10), by + sc(4), label);
            let _ = DeleteObject(f_title);
            let _ = DeleteObject(f_text);
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let dc = GetDC(h);
            let f_text = font(12);
            let (_, _, _, _) = colors();
            let mut y = sc(48);
            let py2 = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut hit = 0;
            for (_label, opts) in rows() {
                let mut x = sc(96);
                for (name, act) in opts {
                    let (w, hgt) = measure(dc, f_text, name);
                    let bw = w + sc(20);
                    let bh = hgt + sc(8);
                    let px = (lp.0 & 0xFFFF) as i16 as i32;
                    let py = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
                    if px >= x && px < x + bw && py >= y && py < y + bh {
                        hit = act;
                    }
                    x += bw + sc(8);
                }
                y += sc(ROW_H);
            }
            let _ = DeleteObject(f_text);
            let _ = ReleaseDC(h, dc);
            // 点到底部的「关闭」就关窗
            let close_y = sc(48) + sc(ROW_H) * 4;
            if hit == 0 && py2 >= close_y && py2 <= close_y + sc(30) {
                let _ = SendMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0));
                let _ = DeleteObject(f_text);
                let _ = ReleaseDC(h, dc);
                return LRESULT(1);
            }
            if hit != 0 {
                apply(hit);
                let _ = InvalidateRect(h, None, TRUE);
                crate::cand::refresh_settings();
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wp.0 == 0x1B {
                let _ = SendMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(h);
            LRESULT(0)
        }
        WM_DESTROY => {
            WND = HWND(0);
            if STANDALONE.load(Ordering::Relaxed) {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

fn apply(action: i32) {
    match action {
        1 => MAX_CANDS.store(5, Ordering::Relaxed),
        2 => MAX_CANDS.store(7, Ordering::Relaxed),
        3 => MAX_CANDS.store(9, Ordering::Relaxed),
        4 => FONT_SCALE.store(90, Ordering::Relaxed),
        5 => FONT_SCALE.store(100, Ordering::Relaxed),
        6 => FONT_SCALE.store(115, Ordering::Relaxed),
        7 => HORIZONTAL.store(false, Ordering::Relaxed),
        8 => HORIZONTAL.store(true, Ordering::Relaxed),
        9 => THEME.store(0, Ordering::Relaxed),
        10 => THEME.store(1, Ordering::Relaxed),
        11 => THEME.store(2, Ordering::Relaxed),
        _ => {}
    }
    save();
}

unsafe fn init() {
    if WND.0 != 0 {
        return;
    }
    let (_, _, bg, _) = colors();
    BG = CreateSolidBrush(bg);
    let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
    let cls: Vec<u16> = "PangyiSettings\0".encode_utf16().collect();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(proc_settings),
        lpszClassName: PCWSTR(cls.as_ptr()),
        hInstance: hinst.into(),
        hbrBackground: BG,
        ..Default::default()
    };
    RegisterClassW(&wc);
    let title: Vec<u16> = "旁译 设置\0".encode_utf16().collect();
    // 普通窗口：有标题栏、有关闭按钮，能正常关掉
    WND = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(cls.as_ptr()), PCWSTR(title.as_ptr()),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
        0, 0, sc(340), sc(260), None, None, hinst, None);
}
/// 独立设置程序用：开一个带标题栏的普通窗口（可以从右上角关掉）。
pub unsafe fn open_app() {
    STANDALONE.store(true, Ordering::Relaxed);
    init();
    if WND.0 == 0 {
        return;
    }
    let (_, _, bg, _) = colors();
    let _ = DeleteObject(BG);
    BG = CreateSolidBrush(bg);
    let (w, h) = (sc(340), sc(260));
    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);
    let _ = SetWindowPos(WND, HWND_TOP, (sw - w) / 2, (sh - h) / 2, w, h, SWP_SHOWWINDOW);
    let _ = InvalidateRect(WND, None, TRUE);
}