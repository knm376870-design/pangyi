//! 候选窗 —— 视觉参数照搬青简（palette.rs / theme/mod.rs）
//!
//! 关键点：常规字重、四列对齐（序号 / 中文 / 拼音 / 俄语）、间距 8/4/8、圆角 8、跟随系统深浅色。
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct Item {
    pub word: String,
    pub pinyin: String,
    pub ru: String,
    pub fresh: bool,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());
static PREEDIT: Mutex<String> = Mutex::new(String::new());
static SEL: AtomicUsize = AtomicUsize::new(0);
static DARK: AtomicBool = AtomicBool::new(true);
static mut WND: HWND = HWND(0);
static mut DPI: i32 = 96;
static mut BG: HBRUSH = HBRUSH(0);

// ---- 青简配色（一字不差）----
struct Palette {
    text: COLORREF,
    gloss: COLORREF,
    index: COLORREF,
    fresh: COLORREF,
    background: COLORREF,
    highlight: COLORREF,
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            text: rgb(0xf5, 0xf5, 0xf7),
            gloss: rgb(0xae, 0xae, 0xb2),
            index: rgb(0x8e, 0x8e, 0x93),
            fresh: rgb(0xff, 0x9f, 0x0a),
            background: rgb(0x2a, 0x2a, 0x2c),
            highlight: rgb(0x2f, 0x4d, 0x72),
        }
    } else {
        Palette {
            text: rgb(0x1d, 0x1d, 0x1f),
            gloss: rgb(0x6b, 0x6b, 0x70),
            index: rgb(0xa0, 0xa0, 0xa6),
            fresh: rgb(0xff, 0x95, 0x00),
            background: rgb(0xf8, 0xf8, 0xf8),
            highlight: rgb(0xcf, 0xe4, 0xf9),
        }
    }
}

fn sc(px: i32) -> i32 {
    let d = unsafe { if DPI <= 0 { 96 } else { DPI } };
    px * d / 96
}

fn font(px: i32) -> HFONT {
    unsafe {
        CreateFontW(-sc(px), 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET.0 as u32,
            0, 0, CLEARTYPE_QUALITY.0 as u32, 0, windows::core::w!("Microsoft YaHei UI"))
    }
}

const PAD: i32 = 8;
const ROW_PAD: i32 = 4;
const RADIUS: i32 = 8;
const WIN_W: i32 = 520;
const ROW_H: i32 = 30;
const COL_IDX: i32 = 14;
const COL_WORD: i32 = 40;
const COL_PINYIN: i32 = 130;
const COL_RU: i32 = 216;

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

pub unsafe fn detect_theme() {
    use windows::Win32::System::Registry::*;
    let mut h = HKEY::default();
    let sub: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0".encode_utf16().collect();
    if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), 0, KEY_READ, &mut h).is_ok() {
        let name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
        let mut v: u32 = 0;
        let mut cb = 4u32;
        let mut ty = REG_DWORD;
        if RegQueryValueExW(h, PCWSTR(name.as_ptr()), None, Some(&mut ty),
            Some(&mut v as *mut u32 as *mut u8), Some(&mut cb)).is_ok() {
            DARK.store(v == 0, Ordering::Relaxed);
        }
        let _ = RegCloseKey(h);
    }
    let dc = GetDC(None);
    if dc.0 != 0 {
        DPI = GetDeviceCaps(dc, LOGPIXELSY);
        let _ = ReleaseDC(None, dc);
    }
    if DPI <= 0 {
        DPI = 96;
    }
}

unsafe extern "system" fn proc_cand(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let p = palette(DARK.load(Ordering::Relaxed));
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let bg = CreateSolidBrush(p.background);
            FillRect(dc, &rc, bg);
            let _ = DeleteObject(bg);
            SetBkMode(dc, TRANSPARENT);

            let f_text = font(16);
            let f_anno = font(12);
            let f_idx = font(11);

            let mut y = sc(PAD);

            let pre = PREEDIT.lock().unwrap().clone();
            if !pre.is_empty() {
                let old = SelectObject(dc, f_anno);
                SetTextColor(dc, p.gloss);
                let s: Vec<u16> = pre.encode_utf16().collect();
                TextOutW(dc, sc(COL_IDX), y + sc(3), &s);
                SelectObject(dc, old);
                y += sc(12) + sc(ROW_PAD) * 2 + sc(4);
            }

            let items = ITEMS.lock().unwrap();
            let sel = SEL.load(Ordering::Relaxed);
            let row_h = sc(ROW_H);
            let max_chars = ((WIN_W - COL_RU - PAD) / 7).max(8) as usize;

            for (i, it) in items.iter().take(9).enumerate() {
                let ry = y + (i as i32) * row_h;

                if i == sel {
                    let hb = CreateSolidBrush(p.highlight);
                    let rgn = CreateRoundRectRgn(sc(4), ry - sc(2), sc(WIN_W - 4), ry + row_h - sc(4), sc(RADIUS), sc(RADIUS));
                    FillRgn(dc, rgn, hb);
                    let _ = DeleteObject(rgn);
                    let _ = DeleteObject(hb);
                }

                let old = SelectObject(dc, f_idx);
                SetTextColor(dc, p.index);
                let n: Vec<u16> = format!("{}.", i + 1).encode_utf16().collect();
                TextOutW(dc, sc(COL_IDX), ry + sc(9), &n);
                SelectObject(dc, old);

                let old = SelectObject(dc, f_text);
                SetTextColor(dc, p.text);
                let word: Vec<u16> = it.word.encode_utf16().collect();
                TextOutW(dc, sc(COL_WORD), ry + sc(6), &word);
                SelectObject(dc, old);

                let old = SelectObject(dc, f_anno);
                SetTextColor(dc, p.index);
                let py: Vec<u16> = it.pinyin.encode_utf16().collect();
                TextOutW(dc, sc(COL_PINYIN), ry + sc(9), &py);

                SetTextColor(dc, if it.fresh { p.fresh } else { p.gloss });
                let gloss_s = truncate(&it.ru.replace('|', " · "), max_chars);
                let gloss: Vec<u16> = gloss_s.encode_utf16().collect();
                TextOutW(dc, sc(COL_RU), ry + sc(9), &gloss);
                SelectObject(dc, old);
            }

            let _ = DeleteObject(f_text);
            let _ = DeleteObject(f_anno);
            let _ = DeleteObject(f_idx);
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

pub unsafe fn init() {
    if WND.0 != 0 {
        return;
    }
    detect_theme();
    let p = palette(DARK.load(Ordering::Relaxed));
    BG = CreateSolidBrush(p.background);
    let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
    let cls: Vec<u16> = "PangyiCandModern\0".encode_utf16().collect();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(proc_cand),
        lpszClassName: PCWSTR(cls.as_ptr()),
        hInstance: hinst.into(),
        hbrBackground: BG,
        ..Default::default()
    };
    RegisterClassW(&wc);
    let title: Vec<u16> = "旁译候选\0".encode_utf16().collect();
    WND = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
        PCWSTR(cls.as_ptr()),
        PCWSTR(title.as_ptr()),
        WS_POPUP,
        0, 0, sc(WIN_W), sc(ROW_H),
        None, None, hinst, None);
}

pub unsafe fn set_items(items: Vec<Item>) {
    {
        let mut g = ITEMS.lock().unwrap();
        *g = items;
    }
    SEL.store(0, Ordering::Relaxed);
}

pub unsafe fn set_preedit(s: &str) {
    *PREEDIT.lock().unwrap() = s.to_string();
}

pub unsafe fn select(i: usize) {
    let n = ITEMS.lock().unwrap().len();
    if n > 0 {
        SEL.store(i.min(n - 1), Ordering::Relaxed);
        if WND.0 != 0 {
            let _ = InvalidateRect(WND, None, TRUE);
        }
    }
}

pub unsafe fn current() -> Option<String> {
    let g = ITEMS.lock().unwrap();
    let i = SEL.load(Ordering::Relaxed);
    if g.is_empty() || i >= g.len() {
        None
    } else {
        Some(g[i].word.clone())
    }
}

pub unsafe fn at(i: usize) -> Option<String> {
    let g = ITEMS.lock().unwrap();
    if i < g.len() {
        Some(g[i].word.clone())
    } else {
        None
    }
}

pub unsafe fn count() -> usize {
    ITEMS.lock().unwrap().len()
}

pub unsafe fn show_at(x: i32, y: i32) {
    if WND.0 == 0 {
        return;
    }
    let n = ITEMS.lock().unwrap().len().min(9);
    if n == 0 {
        hide();
        return;
    }
    let pre_h = if PREEDIT.lock().unwrap().is_empty() { 0 } else { sc(12) + sc(ROW_PAD) * 2 + sc(4) };
    let h = sc(PAD) * 2 + pre_h + (n as i32) * sc(ROW_H);
    let w = sc(WIN_W);
    let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, sc(RADIUS) * 2, sc(RADIUS) * 2);
    let _ = SetWindowRgn(WND, rgn, TRUE);
    let _ = SetWindowPos(WND, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    let _ = InvalidateRect(WND, None, TRUE);
}

pub unsafe fn hide() {
    if WND.0 != 0 {
        let _ = ShowWindow(WND, SW_HIDE);
    }
}