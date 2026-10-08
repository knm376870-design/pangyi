//! 候选窗 —— 视觉与排版照青简（apps/windows/server/src/ui/candidates）：
//! 同一套配色/字体/间距/圆角；竖排为「序号 中文 拼音 俄语」四列对齐，
//! 横排为「序号 中文」一行排开、当前项的拼音与俄语另起一行。
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use windows::core::{PCWSTR, w};
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
static SENTENCE: Mutex<Option<String>> = Mutex::new(None);
static SEL: AtomicUsize = AtomicUsize::new(0);
static DARK: AtomicBool = AtomicBool::new(true);
static HORIZONTAL: AtomicBool = AtomicBool::new(false);
static mut WND: HWND = HWND(0);
static mut DPI: i32 = 96;
static mut BG: HBRUSH = HBRUSH(0);

// ---- 青简配色（palette.rs 一字不差）----
struct Palette {
    text: COLORREF,
    gloss: COLORREF,
    index: COLORREF,
    fresh: COLORREF,
    cloud: COLORREF,
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
            cloud: rgb(0x40, 0xc8, 0xe0),
            background: rgb(0x2a, 0x2a, 0x2c),
            highlight: rgb(0x2f, 0x4d, 0x72),
        }
    } else {
        Palette {
            text: rgb(0x1d, 0x1d, 0x1f),
            gloss: rgb(0x6b, 0x6b, 0x70),
            index: rgb(0xa0, 0xa0, 0xa6),
            fresh: rgb(0xff, 0x95, 0x00),
            cloud: rgb(0x30, 0xb0, 0xc7),
            background: rgb(0xf8, 0xf8, 0xf8),
            highlight: rgb(0xcf, 0xe4, 0xf9),
        }
    }
}

pub fn is_dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

pub unsafe fn dpi() -> i32 {
    if DPI <= 0 { 96 } else { DPI }
}

pub fn horizontal() -> bool {
    crate::settings::horizontal()
}

pub unsafe fn set_horizontal(v: bool) {
    crate::settings::set_horizontal(v);
    refresh_settings();
}

/// 设置改了以后让候选窗按新配置重画/重新量尺寸。
pub unsafe fn refresh_settings() {
    if WND.0 != 0 {
        let _ = InvalidateRect(WND, None, TRUE);
    }
}

fn sc(px: i32) -> i32 {
    let d = unsafe { if DPI <= 0 { 96 } else { DPI } };
    (px * d) / 96
}

fn font(px: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -sc(px * crate::settings::font_scale() / 100), 0, 0, 0, 400, 0, 0, 0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            (VARIABLE_PITCH.0 | FF_DONTCARE.0) as u32,
            w!("Microsoft YaHei UI"),
        )
    }
}

const PAD: i32 = 8;
const ROW_PAD: i32 = 4;
const GAP: i32 = 8;
const RADIUS: i32 = 8;
const TEXT_PX: i32 = 16;
const ANNO_PX: i32 = 12;
const INDEX_PX: i32 = 11;
const MAX_RU_CHARS: usize = 44;

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

fn gloss_of(it: &Item) -> String {
    truncate(&it.ru.replace('|', " · "), MAX_RU_CHARS)
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

struct Layout {
    index_w: i32,
    word_w: i32,
    pinyin_w: i32,
    ru_w: i32,
    item_w: Vec<i32>,
    row_h: i32,
    line_h: i32,
    anno_w: i32,
    anno_h: i32,
    width: i32,
    height: i32,
}

unsafe fn compute_layout(dc: HDC, items: &[Item]) -> Layout {
    let f_text = font(TEXT_PX);
    let f_anno = font(ANNO_PX);
    let f_idx = font(INDEX_PX);
    let pad = sc(PAD);
    let gap = sc(GAP);
    let nth = items.len().min(9);

    let mut index_w = 0;
    let mut word_w = 0;
    let mut pinyin_w = 0;
    let mut ru_w = 0;
    let mut row_h = 0;
    let mut line_h = 0;
    let mut item_w: Vec<i32> = Vec::new();
    for (i, it) in items.iter().take(crate::settings::max_cands()).enumerate() {
        let idx = format!("{}.", i + 1);
        let a = measure(dc, f_idx, &idx).0;
        let (bw, bh) = measure(dc, f_text, &it.word);
        let c = measure(dc, f_anno, &it.pinyin).0;
        let d = measure(dc, f_anno, &gloss_of(it)).0;
        if a > index_w { index_w = a; }
        if bw > word_w { word_w = bw; }
        if c > pinyin_w { pinyin_w = c; }
        if d > ru_w { ru_w = d; }
        let rh = bh + sc(ROW_PAD) * 2;
        if rh > row_h { row_h = rh; }
        line_h = bh;
        item_w.push(a + gap / 2 + bw);
    }
    if row_h == 0 {
        row_h = sc(TEXT_PX) + sc(ROW_PAD) * 2;
    }

    // 高亮项的注释行（横排时另起一行）
    let mut anno_w = 0;
    let mut anno_h = 0;
    if let Some(it) = items.get(SEL.load(Ordering::Relaxed)) {
        let s = format!("{}   {}", it.pinyin, gloss_of(it));
        anno_w = measure(dc, f_anno, &s).0;
        anno_h = measure(dc, f_anno, "x").1 + sc(ROW_PAD);
    }

    let pre = PREEDIT.lock().unwrap().clone();
    let sent = SENTENCE.lock().unwrap().clone();
    let has_top = !pre.is_empty() || sent.is_some();
    let top_w = if !has_top { 0 } else {
        let mut w = if pre.is_empty() { 0 } else { measure(dc, f_anno, &pre).0 };
        if let Some(s) = sent.as_ref() {
            w += gap + measure(dc, f_anno, "☁").0 + gap / 2 + measure(dc, f_anno, s).0;
        }
        w
    };
    let pre_h = if !has_top { 0 } else { measure(dc, f_anno, "x").1 + sc(ROW_PAD) * 2 };

    let (width, height) = if crate::settings::horizontal() {
        let inset = sc(PAD) / 2;
        let mut w = pad * 2 + inset * 2;
        for (i, iw) in item_w.iter().take(nth).enumerate() {
            if i > 0 { w += gap; }
            w += iw;
        }
        let h = pad * 2 + pre_h + row_h + if anno_h > 0 { anno_h } else { 0 };
        (w.max(pad * 2 + anno_w).max(pad * 2 + top_w).max(sc(120)), h)
    } else {
        let w = (pad * 2 + index_w + gap + word_w + gap + pinyin_w + gap + ru_w).max(pad * 2 + top_w);
        let h = pad * 2 + pre_h + (nth as i32) * row_h;
        (w, h)
    };

    let _ = DeleteObject(f_text);
    let _ = DeleteObject(f_anno);
    let _ = DeleteObject(f_idx);
    Layout { index_w, word_w, pinyin_w, ru_w, item_w, row_h, line_h, anno_w, anno_h, width, height }
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
    if DPI <= 0 { DPI = 96; }
    crate::settings::load();
}

unsafe extern "system" fn proc_cand(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let p = palette(crate::settings::dark(DARK.load(Ordering::Relaxed)));
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let bg = CreateSolidBrush(p.background);
            FillRect(dc, &rc, bg);
            let _ = DeleteObject(bg);
            SetBkMode(dc, TRANSPARENT);

            let items = ITEMS.lock().unwrap();
            let lay = compute_layout(dc, &items);
            let f_text = font(TEXT_PX);
            let f_anno = font(ANNO_PX);
            let f_idx = font(INDEX_PX);
            let pad = sc(PAD);
            let gap = sc(GAP);
            let mut y = pad;

            // 顶行：左边是正在输入的拼音，右边是「☁ 补全」（照青简的整句补全位）
            let pre = PREEDIT.lock().unwrap().clone();
            let sent = SENTENCE.lock().unwrap().clone();
            if !pre.is_empty() || sent.is_some() {
                let (_, ph) = measure(dc, f_anno, "x");
                let ty = y + sc(ROW_PAD);
                SelectObject(dc, f_anno);
                let mut tx = pad;
                if !pre.is_empty() {
                    SetTextColor(dc, p.gloss);
                    let s: Vec<u16> = pre.encode_utf16().collect();
                    TextOutW(dc, tx, ty, &s);
                    tx += measure(dc, f_anno, &pre).0;
                }
                if let Some(s) = sent.as_ref() {
                    tx += gap;
                    SetTextColor(dc, p.cloud);
                    let c: Vec<u16> = "☁".encode_utf16().collect();
                    TextOutW(dc, tx, ty, &c);
                    tx += measure(dc, f_anno, "☁").0 + gap / 2;
                    SetTextColor(dc, p.gloss);
                    let g: Vec<u16> = s.encode_utf16().collect();
                    TextOutW(dc, tx, ty, &g);
                }
                y += ph + sc(ROW_PAD) * 2;
            }

            let sel = SEL.load(Ordering::Relaxed);
            let small_idx = ((lay.line_h - measure(dc, f_idx, "x").1) / 2).max(0);
            let small_anno = ((lay.line_h - measure(dc, f_anno, "x").1) / 2).max(0);

            if crate::settings::horizontal() {
                let inset = sc(PAD) / 2;
                let mut x = pad + inset;
                let baseline = y + sc(ROW_PAD);
                for (i, it) in items.iter().take(crate::settings::max_cands()).enumerate() {
                    let iw = lay.item_w.get(i).copied().unwrap_or(0);
                    if i == sel {
                        let hb = CreateSolidBrush(p.highlight);
                        let rgn = CreateRoundRectRgn(x - inset, y, x + iw + inset, y + lay.row_h, sc(RADIUS), sc(RADIUS));
                        FillRgn(dc, rgn, hb);
                        let _ = DeleteObject(rgn);
                        let _ = DeleteObject(hb);
                    }
                    let idx = format!("{}.", i + 1);
                    let iwid = measure(dc, f_idx, &idx).0;
                    SelectObject(dc, f_idx);
                    SetTextColor(dc, p.index);
                    let n: Vec<u16> = idx.encode_utf16().collect();
                    TextOutW(dc, x, baseline + small_idx, &n);

                    SelectObject(dc, f_text);
                    SetTextColor(dc, p.text);
                    let word: Vec<u16> = it.word.encode_utf16().collect();
                    TextOutW(dc, x + iwid + gap / 2, baseline, &word);
                    x += iw + gap;
                }
                if lay.anno_h > 0 {
                    if let Some(it) = items.get(sel) {
                        let s = format!("{}   {}", it.pinyin, gloss_of(it));
                        SelectObject(dc, f_anno);
                        SetTextColor(dc, if it.fresh { p.fresh } else { p.gloss });
                        let g: Vec<u16> = s.encode_utf16().collect();
                        TextOutW(dc, pad, y + lay.row_h + sc(ROW_PAD) / 2, &g);
                    }
                }
            } else {
                let x_index = pad;
                let x_word = x_index + lay.index_w + gap;
                let x_pinyin = x_word + lay.word_w + gap;
                let x_ru = x_pinyin + lay.pinyin_w + gap;
                for (i, it) in items.iter().take(crate::settings::max_cands()).enumerate() {
                    let ry = y + (i as i32) * lay.row_h;
                    if i == sel {
                        let hb = CreateSolidBrush(p.highlight);
                        let rgn = CreateRoundRectRgn(pad / 2, ry, rc.right - pad / 2, ry + lay.row_h, sc(RADIUS), sc(RADIUS));
                        FillRgn(dc, rgn, hb);
                        let _ = DeleteObject(rgn);
                        let _ = DeleteObject(hb);
                    }
                    let ty = ry + sc(ROW_PAD);
                    SelectObject(dc, f_idx);
                    SetTextColor(dc, p.index);
                    let n: Vec<u16> = format!("{}.", i + 1).encode_utf16().collect();
                    TextOutW(dc, x_index, ty + small_idx, &n);

                    SelectObject(dc, f_text);
                    SetTextColor(dc, p.text);
                    let word: Vec<u16> = it.word.encode_utf16().collect();
                    TextOutW(dc, x_word, ty, &word);

                    SelectObject(dc, f_anno);
                    SetTextColor(dc, p.index);
                    let py: Vec<u16> = it.pinyin.encode_utf16().collect();
                    TextOutW(dc, x_pinyin, ty + small_anno, &py);

                    SetTextColor(dc, if it.fresh { p.fresh } else { p.gloss });
                    let gloss: Vec<u16> = gloss_of(it).encode_utf16().collect();
                    TextOutW(dc, x_ru, ty + small_anno, &gloss);
                }
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
    if WND.0 != 0 { return; }
    detect_theme();
    let p = palette(crate::settings::dark(DARK.load(Ordering::Relaxed)));
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
    let hr = RegisterClassW(&wc);    let title: Vec<u16> = "旁译候选\0".encode_utf16().collect();
    WND = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
        PCWSTR(cls.as_ptr()), PCWSTR(title.as_ptr()), WS_POPUP,
        0, 0, sc(400), sc(40), None, None, hinst, None);}

pub unsafe fn set_items(items: Vec<Item>) {
    { let mut g = ITEMS.lock().unwrap(); *g = items; }
    SEL.store(0, Ordering::Relaxed);
}

pub unsafe fn set_preedit(s: &str) { *PREEDIT.lock().unwrap() = s.to_string(); }

pub unsafe fn set_sentence(s: Option<String>) { *SENTENCE.lock().unwrap() = s; }

pub unsafe fn select(i: usize) {
    let n = ITEMS.lock().unwrap().len();
    if n > 0 {
        SEL.store(i.min(n - 1), Ordering::Relaxed);
        if WND.0 != 0 { let _ = InvalidateRect(WND, None, TRUE); }
    }
}

pub unsafe fn current_index() -> usize { SEL.load(Ordering::Relaxed) }

pub unsafe fn current() -> Option<String> {
    let g = ITEMS.lock().unwrap();
    let i = SEL.load(Ordering::Relaxed);
    if g.is_empty() || i >= g.len() { None } else { Some(g[i].word.clone()) }
}

pub unsafe fn at(i: usize) -> Option<String> {
    let g = ITEMS.lock().unwrap();
    if i < g.len() { Some(g[i].word.clone()) } else { None }
}

pub unsafe fn count() -> usize { ITEMS.lock().unwrap().len() }

pub unsafe fn show_at(x: i32, y: i32) {
    if WND.0 == 0 { return; }
    let n = ITEMS.lock().unwrap().len().min(crate::settings::max_cands());    if n == 0 { hide(); return; }
    let dc = GetDC(WND);
    let (mut cw, mut ch) = (sc(400), sc(40));
    if dc.0 != 0 {
        let items = ITEMS.lock().unwrap();
        let lay = compute_layout(dc, &items);
        cw = lay.width;
        ch = lay.height;
        let _ = ReleaseDC(WND, dc);
    }
    // 夹进屏幕：右边超出就左移，下方放不下就翻到光标上方
    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);
    let mut px = x;
    let mut py = y;
    if px + cw > sw - 4 { px = (sw - cw - 4).max(0); }
    if py + ch > sh - 4 { py = (y - ch - sc(30)).max(0); }
    if py < 0 { py = 0; }
    let rgn = CreateRoundRectRgn(0, 0, cw + 1, ch + 1, sc(RADIUS) * 2, sc(RADIUS) * 2);
    let _ = SetWindowRgn(WND, rgn, TRUE);
    // 只在“从隐藏变显示”的那一次抬高到置顶带的最前面（保证能盖住置顶窗口），
    // 后续按键只挪位置、不动 z 序——每次按键都抢顶层会让某些宿主丢焦点、上屏失败。
    let was_visible = IsWindowVisible(WND).as_bool();
    if was_visible {
        let _ = SetWindowPos(WND, HWND(0), px, py, cw, ch, SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    } else {
        let _ = SetWindowPos(WND, HWND_TOPMOST, px, py, cw, ch, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
    let _ = InvalidateRect(WND, None, TRUE);
}

pub unsafe fn hide() { if WND.0 != 0 { let _ = ShowWindow(WND, SW_HIDE); } }