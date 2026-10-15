//! 候选窗 —— 视觉与排版照青简（apps/windows/server/src/ui/candidates）：
//! 同一套配色/字体/间距/圆角；竖排为「序号 中文 拼音 俄语」四列对齐，
//! 横排为「序号 中文」一行排开、当前项的拼音与俄语另起一行。
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicUsize, Ordering};
use windows::core::{PCWSTR, w};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP, VK_RETURN,
};
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
/// 蓝条选中哪一侧：false = 中文（默认），true = 俄语侧 —— 上屏只出俄语，
/// 中文不输入。←/→ 切换，鼠标点俄语列同样进入；每次换新候选组自动回中文侧。
static RU_SEL: AtomicBool = AtomicBool::new(false);
/// 当前页号（0 起）和总页数，画右下角那个「▲▼ 2/59」用。总页数 1 就不画。
static PAGE_CUR: AtomicUsize = AtomicUsize::new(0);
static PAGE_TOT: AtomicUsize = AtomicUsize::new(1);
static DARK: AtomicBool = AtomicBool::new(true);
// 句柄和 DPI 是全进程共享的（同一进程里可能有好几个线程在用输入法），
// 原来用 static mut 属于数据竞争 —— 换成原子量。
static WND: AtomicIsize = AtomicIsize::new(0);
static DPI: AtomicI32 = AtomicI32::new(96);

fn hwnd() -> HWND {
    HWND(WND.load(Ordering::Relaxed))
}

fn set_hwnd(h: HWND) {
    WND.store(h.0, Ordering::Relaxed);
}

/// 窗口还活着吗？
/// 关键：候选窗是在「第一次激活输入法」的那个线程上创建的，
/// 那个线程一退出，窗口就被销毁，但句柄还留在全局变量里 ——
/// 这就是"切过去候选窗再也不出现"的原因。所以每次用之前都要验一次。
pub unsafe fn alive() -> bool {
    let h = hwnd();
    h.0 != 0 && IsWindow(h).as_bool()
}

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

pub fn dpi() -> i32 {
    let d = DPI.load(Ordering::Relaxed);
    if d <= 0 { 96 } else { d }
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
    clear_font_cache();
    if alive() {
        let _ = InvalidateRect(hwnd(), None, TRUE);
    }
}

fn sc(px: i32) -> i32 {
    (px * dpi()) / 96
}

/// 字体缓存。
/// 原来每次重绘 / 每次测宽都要 CreateFontW 三个字体再删掉，
/// 敲一个字母最多要建 9 次字体。字体对象按 (字号, 缩放, DPI) 复用。
static FONTS: Mutex<Vec<(i32, i32, i32, HFONT)>> = Mutex::new(Vec::new());

fn font(px: i32) -> HFONT {
    let scale = crate::settings::font_scale();
    let d = dpi();
    {
        let g = FONTS.lock().unwrap();
        if let Some((_, _, _, h)) = g.iter().find(|(p, s, dd, _)| *p == px && *s == scale && *dd == d) {
            return *h;
        }
    }
    let h = unsafe { make_font(px) };
    FONTS.lock().unwrap().push((px, scale, d, h));
    h
}

unsafe fn make_font(px: i32) -> HFONT {
    {
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

/// 设置/DPI 变了就把缓存清掉重建
pub fn clear_font_cache() {
    let mut g = FONTS.lock().unwrap();
    for (_, _, _, h) in g.drain(..) {
        unsafe { let _ = DeleteObject(h); }
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

    // 右下角页码「▲▼ 2/59」——只有一页时不出，老尺寸完全不变
    let pages = PAGE_TOT.load(Ordering::Relaxed);
    let (footer_w, footer_h) = if pages > 1 {
        let s = format!("▲▼ {}/{}", PAGE_CUR.load(Ordering::Relaxed) + 1, pages);
        (measure(dc, f_anno, &s).0, measure(dc, f_anno, "x").1 + sc(ROW_PAD))
    } else {
        (0, 0)
    };

    let (width, height) = if crate::settings::horizontal() {
        let inset = sc(PAD) / 2;
        let mut w = pad * 2 + inset * 2;
        for (i, iw) in item_w.iter().take(nth).enumerate() {
            if i > 0 { w += gap; }
            w += iw;
        }
        let h = pad * 2 + pre_h + row_h + if anno_h > 0 { anno_h } else { 0 } + footer_h;
        (w.max(pad * 2 + anno_w).max(pad * 2 + top_w).max(pad * 2 + footer_w).max(sc(120)), h)
    } else {
        let w = (pad * 2 + index_w + gap + word_w + gap + pinyin_w + gap + ru_w)
            .max(pad * 2 + top_w)
            .max(pad * 2 + footer_w);
        let h = pad * 2 + pre_h + (nth as i32) * row_h + footer_h;
        (w, h)
    };

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
        DPI.store(GetDeviceCaps(dc, LOGPIXELSY), Ordering::Relaxed);
        let _ = ReleaseDC(None, dc);
    }
    if DPI.load(Ordering::Relaxed) <= 0 { DPI.store(96, Ordering::Relaxed); }
    crate::settings::load();
}

unsafe extern "system" fn proc_cand(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        // 我们自己把整块客户区画满，不让系统擦背景，避免闪一下白
        WM_ERASEBKGND => LRESULT(1),
        // 窗口被销毁（宿主线程退出等）：把句柄清掉，下次要显示时会重建
        WM_DESTROY | WM_NCDESTROY => {
            if hwnd().0 == h.0 { set_hwnd(HWND(0)); }
            LRESULT(0)
        }
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
                        // 选中俄语侧时蓝条只罩住俄语那一列（←→/点俄语列进入）
                        let l = if RU_SEL.load(Ordering::Relaxed) { x_ru - pad / 2 } else { pad / 2 };
                        let rgn = CreateRoundRectRgn(l, ry, rc.right - pad / 2, ry + lay.row_h, sc(RADIUS), sc(RADIUS));
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

            // 右下角页码：▲▼ 2/59（只有一页时不画）。它就是"还能往下翻"的提示，
            // 没有它用户根本不知道候选不止窗口里这几个
            let pages = PAGE_TOT.load(Ordering::Relaxed);
            if pages > 1 {
                let s = format!("▲▼ {}/{}", PAGE_CUR.load(Ordering::Relaxed) + 1, pages);
                let (_, hh) = measure(dc, f_anno, "x");
                let wpx = measure(dc, f_anno, &s).0;
                let ty = rc.bottom - pad - hh - sc(ROW_PAD) / 2;
                SelectObject(dc, f_anno);
                SetTextColor(dc, p.index);
                let t: Vec<u16> = s.encode_utf16().collect();
                TextOutW(dc, rc.right - pad - wpx, ty, &t);
            }

            // 注意：f_text / f_anno / f_idx 是缓存的字体对象，这里不能删
            let _ = (f_text, f_anno, f_idx);
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        // 鼠标点候选直接上屏：点哪条选哪条；竖排点俄语列＝选中俄语侧。
        // 然后合成一次回车，走和真键盘完全一样的 TSF 提交路径 —— 自学习、
        // 逐字/整句翻译、句子缓冲一个不落，不另开一条提交逻辑。
        WM_LBUTTONDOWN => {
            // 客户区坐标：l 低 16 位是 x、高 16 位是 y（有符号）
            let x = (l.0 as i16) as i32;
            let y = ((l.0 >> 16) as i16) as i32;
            if let Some((i, ru)) = hit_test(x, y) {
                select(i);
                set_ru_sel(ru && !crate::settings::horizontal());
                click_commit();
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

/// 合成一次回车（按下+抬起）：候选窗点选后触发提交。
/// 候选窗是 WS_EX_NOACTIVATE，点它不抢焦点，回车照旧落回宿主应用，
/// 由 TSF 正常接管 —— 和用户手按回车一模一样。
unsafe fn click_commit() {
    let mut down = INPUT::default();
    down.r#type = INPUT_KEYBOARD;
    down.Anonymous.ki.wVk = VK_RETURN;
    let mut up = INPUT::default();
    up.r#type = INPUT_KEYBOARD;
    up.Anonymous.ki.wVk = VK_RETURN;
    up.Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
    let ins = [down, up];
    let _ = SendInput(&ins, std::mem::size_of::<INPUT>() as i32);
}

pub unsafe fn init() {
    if alive() { return; }
    // 句柄已经失效（建窗线程没了）：清掉重新建
    if WND.load(Ordering::Relaxed) != 0 { set_hwnd(HWND(0)); }
    detect_theme();
    let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
    let cls: Vec<u16> = "PangyiCandModern\0".encode_utf16().collect();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(proc_cand),
        lpszClassName: PCWSTR(cls.as_ptr()),
        hInstance: hinst.into(),
        // 背景全部由 WM_PAINT 自己画；类背景给空刷子，再吃掉 WM_ERASEBKGND，避免闪白
        hbrBackground: HBRUSH(0),
        ..Default::default()
    };
    // 同一个进程里重复注册会失败，无所谓，类已经在了
    let _ = RegisterClassW(&wc);
    let title: Vec<u16> = "旁译候选\0".encode_utf16().collect();
    let h = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
        PCWSTR(cls.as_ptr()), PCWSTR(title.as_ptr()), WS_POPUP,
        0, 0, sc(400), sc(40), None, None, hinst, None);
    set_hwnd(h);
}

pub unsafe fn set_items(items: Vec<Item>) {
    { let mut g = ITEMS.lock().unwrap(); *g = items; }
    SEL.store(0, Ordering::Relaxed);
    // 新一组候选重新从中文侧开始（上次 ←→ 选中的俄语侧不再沿用）
    RU_SEL.store(false, Ordering::Relaxed);
}

/// 记下页码（画右下角的「▲▼ n/总」）。cur 是 0 起的页号。
pub unsafe fn set_page(cur: usize, total: usize) {
    PAGE_CUR.store(cur, Ordering::Relaxed);
    PAGE_TOT.store(total.max(1), Ordering::Relaxed);
}

/// 给测试台架用：当前页号和总页数
pub unsafe fn page_info() -> (usize, usize) {
    (PAGE_CUR.load(Ordering::Relaxed), PAGE_TOT.load(Ordering::Relaxed))
}

pub unsafe fn set_preedit(s: &str) { *PREEDIT.lock().unwrap() = s.to_string(); }

pub unsafe fn set_sentence(s: Option<String>) { *SENTENCE.lock().unwrap() = s; }

/// 蓝条现在选中哪一侧（true = 俄语侧）。提交路径据此决定上屏中文还是俄语。
pub fn ru_sel() -> bool {
    RU_SEL.load(Ordering::Relaxed)
}

/// ←/→ 切换选中侧：蓝条从整行挪到俄语那一列（或挪回来），触发重画。
pub unsafe fn set_ru_sel(v: bool) {
    RU_SEL.store(v, Ordering::Relaxed);
    if alive() { let _ = InvalidateRect(hwnd(), None, TRUE); }
}

fn horizontal_item_rect(index: usize, widths: &[i32], origin: (i32, i32), row_h: i32, spacing: (i32, i32)) -> Option<RECT> {
    let width = *widths.get(index)?;
    if width <= 0 || row_h <= 0 { return None; }
    let (gap, inset) = spacing;
    let x = widths[..index].iter().fold(origin.0, |x, width| x.saturating_add(*width).saturating_add(gap));
    Some(RECT {
        left: x.saturating_sub(inset), top: origin.1,
        right: x.saturating_add(width).saturating_add(inset), bottom: origin.1.saturating_add(row_h),
    })
}

fn horizontal_hit(point: (i32, i32), widths: &[i32], origin: (i32, i32), row_h: i32, spacing: (i32, i32)) -> Option<usize> {
    (0..widths.len()).find(|&i| {
        horizontal_item_rect(i, widths, origin, row_h, spacing).map_or(false, |r|
            point.0 >= r.left && point.0 < r.right && point.1 >= r.top && point.1 < r.bottom)
    })
}

/// 竖排几何（和 WM_PAINT 同一套算法）：首行 y、行高、词列 x、俄语列 x。
/// 顶行（拼音 / ☁ 提示）占多高就先加多高，这样点击和画的位置对得上。
unsafe fn v_geom(dc: HDC) -> Option<(i32, i32, i32, i32)> {
    let items = ITEMS.lock().unwrap();
    if items.is_empty() { return None; }
    let lay = compute_layout(dc, &items);
    drop(items);
    let pad = sc(PAD);
    let gap = sc(GAP);
    let mut y = pad;
    let pre = PREEDIT.lock().unwrap().clone();
    let sent = SENTENCE.lock().unwrap().clone();
    if !pre.is_empty() || sent.is_some() {
        let f_anno = font(ANNO_PX);
        let (_, ph) = measure(dc, f_anno, "x");
        y += ph + sc(ROW_PAD) * 2;
    }
    let x_word = pad + lay.index_w + gap;
    let x_ru = x_word + lay.word_w + gap + lay.pinyin_w + gap;
    Some((y, lay.row_h, x_word, x_ru))
}

/// 鼠标点 (x, y)（客户区坐标）落在哪一行、哪一侧：(行号, 是否俄语列)。
/// 点不到候选行返回 None。索引列和词列都算中文侧。
pub unsafe fn hit_test(x: i32, y: i32) -> Option<(usize, bool)> {
    let h = hwnd();
    let dc = GetDC(h);
    if dc.0 == 0 { return None; }
    let result = (|| {
        let (y0, row_h, _x_word, x_ru) = v_geom(dc)?;
        if crate::settings::horizontal() {
            let items = ITEMS.lock().unwrap();
            let lay = compute_layout(dc, &items);
            let pad = sc(PAD);
            return horizontal_hit((x, y), &lay.item_w, (pad + pad / 2, y0), row_h, (sc(GAP), pad / 2))
                .map(|i| (i, false));
        }
        let dy = y - y0;
        if dy < 0 || row_h <= 0 { return None; }
        let i = (dy / row_h) as usize;
        if i >= ITEMS.lock().unwrap().len() { return None; }
        if x >= x_ru {
            Some((i, true))
        } else if x >= 0 {
            Some((i, false))
        } else {
            None
        }
    })();
    let _ = ReleaseDC(h, dc);
    result
}

/// 第 row 行中文侧/俄语侧的可点击矩形（客户区坐标），测试台架算点击点用。
pub unsafe fn hit_rect(row: usize, ru: bool) -> Option<(i32, i32, i32, i32)> {
    let h = hwnd();
    let dc = GetDC(h);
    if dc.0 == 0 { return None; }
    let result = (|| {
        let (y0, row_h, x_word, x_ru) = v_geom(dc)?;
        if crate::settings::horizontal() {
            if ru { return None; } // 横排只显示一排中文候选，没有可选的俄语侧。
            let items = ITEMS.lock().unwrap();
            let lay = compute_layout(dc, &items);
            let pad = sc(PAD);
            let r = horizontal_item_rect(row, &lay.item_w, (pad + pad / 2, y0), row_h, (sc(GAP), pad / 2))?;
            return Some((r.left, r.top, r.right, r.bottom));
        }
        if row >= ITEMS.lock().unwrap().len() { return None; }
        let top = y0 + row as i32 * row_h;
        let l = if ru { x_ru } else { x_word };
        Some((l, top, l + 60, top + row_h))
    })();
    let _ = ReleaseDC(h, dc);
    result
}

// ---- 测试台架专用导出（side_e2e 用 GetProcAddress 拿，不挂在正常 API 上）----

/// 候选窗句柄：台架 PostMessage 鼠标点击用
#[no_mangle]
pub unsafe extern "system" fn cand_hwnd_export() -> isize {
    hwnd().0
}

/// 第 row 行中文侧(ru=0)/俄语侧(ru=1)的可点击矩形写进 out[4] = l,t,r,b
#[no_mangle]
pub unsafe extern "system" fn cand_hit_rect_export(row: u32, ru: u32, out: *mut i32) -> u32 {
    if out.is_null() { return 0; }
    match hit_rect(row as usize, ru != 0) {
        Some((l, t, r, b)) => {
            *out.add(0) = l;
            *out.add(1) = t;
            *out.add(2) = r;
            *out.add(3) = b;
            1
        }
        None => 0,
    }
}

pub unsafe fn select(i: usize) {
    let n = ITEMS.lock().unwrap().len();
    if n > 0 {
        SEL.store(i.min(n - 1), Ordering::Relaxed);
        if alive() { let _ = InvalidateRect(hwnd(), None, TRUE); }
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

/// 把候选窗放入光标所在显示器的工作区，保留副屏的负坐标，避开任务栏。
fn fit_popup(anchor: (i32, i32), size: (i32, i32), work: RECT, margin: i32, above_gap: i32) -> RECT {
    let span_w = work.right.saturating_sub(work.left).max(1);
    let span_h = work.bottom.saturating_sub(work.top).max(1);
    let mx = margin.max(0).min((span_w - 1) / 2);
    let my = margin.max(0).min((span_h - 1) / 2);
    let left = work.left.saturating_add(mx);
    let top = work.top.saturating_add(my);
    let right = work.right.saturating_sub(mx).max(left.saturating_add(1));
    let bottom = work.bottom.saturating_sub(my).max(top.saturating_add(1));
    let width = size.0.max(1).min(right.saturating_sub(left).max(1));
    let height = size.1.max(1).min(bottom.saturating_sub(top).max(1));
    let x = anchor.0.clamp(left, right.saturating_sub(width).max(left));
    let y = if anchor.1.saturating_add(height) > bottom {
        anchor.1.saturating_sub(height).saturating_sub(above_gap.max(0))
    } else {
        anchor.1
    }.clamp(top, bottom.saturating_sub(height).max(top));
    RECT { left: x, top: y, right: x.saturating_add(width), bottom: y.saturating_add(height) }
}

pub unsafe fn show_at(x: i32, y: i32) {
    // 每次显示前都确认窗口还在：宿主进程换了线程、原来的窗口没了就重建一个，
    // 而不是直接 return（那会让候选窗永久消失）
    if !alive() { init(); }
    if !alive() { return; }
    let h = hwnd();
    let n = ITEMS.lock().unwrap().len().min(crate::settings::max_cands());
    if n == 0 { hide(); return; }
    let dc = GetDC(h);
    let (mut cw, mut ch) = (sc(400), sc(40));
    if dc.0 != 0 {
        let items = ITEMS.lock().unwrap();
        let lay = compute_layout(dc, &items);
        cw = lay.width;
        ch = lay.height;
        let _ = ReleaseDC(h, dc);
    }
    // SM_CXSCREEN/SM_CYSCREEN 只有主屏尺寸，不能用来夹副屏坐标。
    let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let work = if GetMonitorInfoW(monitor, &mut info).as_bool() {
        info.rcWork
    } else {
        RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN).max(1), bottom: GetSystemMetrics(SM_CYSCREEN).max(1) }
    };
    let bounds = fit_popup((x, y), (cw, ch), work, sc(4), sc(30));
    let (px, py) = (bounds.left, bounds.top);
    cw = bounds.right - bounds.left;
    ch = bounds.bottom - bounds.top;
    let rgn = CreateRoundRectRgn(0, 0, cw + 1, ch + 1, sc(RADIUS) * 2, sc(RADIUS) * 2);
    let _ = SetWindowRgn(h, rgn, TRUE);
    // 只在“从隐藏变显示”的那一次抬高到置顶带的最前面（保证能盖住置顶窗口），
    // 后续按键只挪位置、不动 z 序——每次按键都抢顶层会让某些宿主丢焦点、上屏失败。
    let was_visible = IsWindowVisible(h).as_bool();
    if was_visible {
        let _ = SetWindowPos(h, HWND(0), px, py, cw, ch, SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    } else {
        let _ = SetWindowPos(h, HWND_TOPMOST, px, py, cw, ch, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
    let _ = InvalidateRect(h, None, TRUE);
}

pub unsafe fn hide() { if alive() { let _ = ShowWindow(hwnd(), SW_HIDE); } }

#[cfg(test)]
mod placement_tests {
    use super::*;

    fn bounds(anchor: (i32, i32), size: (i32, i32), work: RECT) -> (i32, i32, i32, i32) {
        let r = fit_popup(anchor, size, work, 4, 30);
        (r.left, r.top, r.right, r.bottom)
    }

    #[test]
    fn right_monitor_keeps_its_screen_coordinates() {
        let work = RECT { left: 1920, top: 0, right: 3840, bottom: 1040 };
        assert_eq!(bounds((2500, 100), (400, 200), work), (2500, 100, 2900, 300));
        assert_eq!(bounds((3800, 100), (400, 200), work), (3436, 100, 3836, 300));
    }

    #[test]
    fn left_monitor_preserves_negative_coordinates() {
        let work = RECT { left: -1920, top: 0, right: 0, bottom: 1040 };
        assert_eq!(bounds((-1700, 100), (400, 200), work), (-1700, 100, -1300, 300));
        assert_eq!(bounds((-100, 100), (400, 200), work), (-404, 100, -4, 300));
    }

    #[test]
    fn monitor_above_primary_preserves_negative_y() {
        let work = RECT { left: 0, top: -1200, right: 1920, bottom: -40 };
        assert_eq!(bounds((200, -1100), (400, 200), work), (200, -1100, 600, -900));
    }

    #[test]
    fn bottom_taskbar_moves_popup_above_caret() {
        let work = RECT { left: 0, top: 0, right: 1920, bottom: 1040 };
        assert_eq!(bounds((200, 1000), (400, 250), work), (200, 720, 600, 970));
    }

    #[test]
    fn top_and_left_taskbars_define_the_usable_origin() {
        let work = RECT { left: 80, top: 40, right: 1920, bottom: 1080 };
        assert_eq!(bounds((5, 5), (400, 200), work), (84, 44, 484, 244));
    }

    #[test]
    fn oversized_popup_is_bounded_by_small_work_area() {
        let work = RECT { left: -800, top: -600, right: 0, bottom: 0 };
        assert_eq!(bounds((-300, -100), (2000, 1500), work), (-796, -596, -4, -4));
    }

    #[test]
    fn tiny_work_area_and_empty_size_do_not_panic() {
        let work = RECT { left: -10, top: -10, right: -9, bottom: -9 };
        assert_eq!(bounds((0, 0), (0, 0), work), (-10, -10, -9, -9));
    }

    #[test]
    fn horizontal_clicks_select_the_candidate_under_the_pointer() {
        let widths = [80, 120, 60];
        for (point, expected) in [((12, 35), 0), ((150, 35), 1), ((260, 35), 2)] {
            assert_eq!(horizontal_hit(point, &widths, (12, 30), 24, (8, 4)), Some(expected));
        }
    }

    #[test]
    fn horizontal_header_annotation_and_outer_padding_are_not_candidates() {
        for point in [(12, 29), (12, 54), (7, 35), (292, 35), (-1, 35)] {
            assert_eq!(horizontal_hit(point, &[80, 120, 60], (12, 30), 24, (8, 4)), None);
        }
    }

    #[test]
    fn horizontal_shared_edge_belongs_to_the_next_candidate() {
        assert_eq!(horizontal_hit((95, 35), &[80, 120], (12, 30), 24, (8, 4)), Some(0));
        assert_eq!(horizontal_hit((96, 35), &[80, 120], (12, 30), 24, (8, 4)), Some(1));
    }

    #[test]
    fn horizontal_hit_rect_matches_the_painted_candidate_extent() {
        let r = horizontal_item_rect(1, &[80, 120, 60], (12, 30), 24, (8, 4)).unwrap();
        assert_eq!((r.left, r.top, r.right, r.bottom), (96, 30, 224, 54));
        assert!(horizontal_item_rect(3, &[80, 120, 60], (12, 30), 24, (8, 4)).is_none());
    }
}
