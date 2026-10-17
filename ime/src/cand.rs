//! 候选窗 —— v4.7.0 换成设置面板同款设计语言：圆角卡片 + 青色选中条 +
//! 悬停高亮 + 页码药丸（配色照 settings.rs 的 Palette）。
//! 排版仍是青简同款：竖排为「序号 中文 俄语」三列对齐，
//! 横排为「序号 中文」一行排开、当前项的俄语另起一行。
//! v4.8.0：去掉拼音列（拼音只在顶行输入串里显示，候选词旁不再展示）。
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicUsize, Ordering};
use windows::core::{PCWSTR, w};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP, VK_RETURN,
    TrackMouseEvent, TRACKMOUSEEVENT, TME_LEAVE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct Item {
    pub word: String,
    pub ru: String,
    pub fresh: bool,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());
static PREEDIT: Mutex<String> = Mutex::new(String::new());
static SENTENCE: Mutex<Option<String>> = Mutex::new(None);
static SEL: AtomicUsize = AtomicUsize::new(0);
/// 青条选中哪一侧：false = 中文（默认），true = 俄语侧 —— 上屏只出俄语，
/// 中文不输入。←/→ 切换，鼠标点俄语列同样进入；每次换新候选组自动回中文侧。
static RU_SEL: AtomicBool = AtomicBool::new(false);
/// 鼠标悬停哪一行（-1 = 没有）。只影响悬停高亮和手型光标，不碰选中/提交。
static HOT_ROW: AtomicI32 = AtomicI32::new(-1);
/// 悬停行落在哪一侧（和 RU_SEL 同一套 x_ru 判定，画悬停条用）
static HOT_RU: AtomicBool = AtomicBool::new(false);
/// 已经申请过 TrackMouseEvent 了；申请一次就能收到 WM_MOUSELEAVE（防重复挂）
static TRACKED: AtomicBool = AtomicBool::new(false);
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

// ---- v4.7.0 配色：设置面板同款设计语言（与 settings.rs Palette 同名字段同值）----
struct Palette {
    text: COLORREF,     // 主文字（词）
    faint: COLORREF,    // 次要文字（拼音/序号/页码/未选中俄语）
    fresh: COLORREF,    // 生词橙色（语义色，两主题保持一致）
    cloud: COLORREF,    // ☁ 整句提示（= 面板 accent）
    card: COLORREF,     // 窗口卡片底
    border: COLORREF,   // 卡片 1px 描边
    bar_fill: COLORREF, // 选中条填充（= 面板 pill_sel_fill）
    bar_text: COLORREF, // 选中条上的文字（= 面板 pill_sel_text）
    hover: COLORREF,    // 悬停行填充（= 面板 pill_hover）
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            text: rgb(0xf5, 0xf5, 0xf7),
            faint: rgb(0xae, 0xae, 0xb2),
            fresh: rgb(0xff, 0x9f, 0x0a),
            cloud: rgb(0x40, 0xc8, 0xe0),
            card: rgb(0x2a, 0x2a, 0x2d),
            border: rgb(0x3a, 0x3a, 0x3e),
            bar_fill: rgb(0x1e, 0x3a, 0x44),
            bar_text: rgb(0xb7, 0xe6, 0xf0),
            hover: rgb(0x33, 0x33, 0x37),
        }
    } else {
        Palette {
            text: rgb(0x1d, 0x1d, 0x1f),
            faint: rgb(0x6b, 0x6b, 0x70),
            fresh: rgb(0xff, 0x95, 0x00),
            cloud: rgb(0x2f, 0xb8, 0xcc),
            card: rgb(0xff, 0xff, 0xff),
            border: rgb(0xe3, 0xe6, 0xea),
            bar_fill: rgb(0xdd, 0xf2, 0xf6),
            bar_text: rgb(0x17, 0x60, 0x6c),
            hover: rgb(0xe9, 0xeb, 0xee),
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

// ---- 绘制助手（照 settings.rs 同款：区域/刷子成对创建删除，GDI 对象不漏）----

/// 圆角矩形填充
unsafe fn fill_round(dc: HDC, r: &RECT, radius: i32, color: COLORREF) {
    let rb = CreateSolidBrush(color);
    let rgn = CreateRoundRectRgn(r.left, r.top, r.right, r.bottom, radius, radius);
    FillRgn(dc, rgn, rb);
    let _ = DeleteObject(rgn);
    let _ = DeleteObject(rb);
}

/// 圆角矩形 1px 描边（画在矩形内侧）
unsafe fn frame_round(dc: HDC, r: &RECT, radius: i32, color: COLORREF) {
    let rb = CreateSolidBrush(color);
    let rgn = CreateRoundRectRgn(r.left, r.top, r.right, r.bottom, radius, radius);
    let _ = FrameRgn(dc, rgn, rb, 1, 1);
    let _ = DeleteObject(rgn);
    let _ = DeleteObject(rb);
}

/// 无边框矩形填充
unsafe fn fill_rect(dc: HDC, l: i32, t: i32, r: i32, b: i32, color: COLORREF) {
    let rb = CreateSolidBrush(color);
    FillRect(dc, &RECT { left: l, top: t, right: r, bottom: b }, rb);
    let _ = DeleteObject(rb);
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
/// 窗口圆角（卡片风）：SetWindowRgn 的角椭圆直径 = WIN_RADIUS*2
const WIN_RADIUS: i32 = 10;
/// 选中条 / 悬停条 / 页码药丸的圆角
const BAR_RADIUS: i32 = 8;
/// 页码药丸的内边距
const PAGE_PAD_X: i32 = 6;
const PAGE_PAD_Y: i32 = 2;
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
    item_w: Vec<i32>,
    /// 横排时每条目的命中 x 跨度 (l, r)，和 WM_PAINT 同一套累加；竖排空着
    hitem: Vec<(i32, i32)>,
    row_h: i32,
    line_h: i32,
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
    let mut ru_w = 0;
    let mut row_h = 0;
    let mut line_h = 0;
    let mut item_w: Vec<i32> = Vec::new();
    for (i, it) in items.iter().take(crate::settings::max_cands()).enumerate() {
        let idx = format!("{}.", i + 1);
        let a = measure(dc, f_idx, &idx).0;
        let (bw, bh) = measure(dc, f_text, &it.word);
        let d = measure(dc, f_anno, &gloss_of(it)).0;
        if a > index_w { index_w = a; }
        if bw > word_w { word_w = bw; }
        if d > ru_w { ru_w = d; }
        let rh = bh + sc(ROW_PAD) * 2;
        if rh > row_h { row_h = rh; }
        line_h = bh;
        item_w.push(a + gap / 2 + bw);
    }
    if row_h == 0 {
        row_h = sc(TEXT_PX) + sc(ROW_PAD) * 2;
    }

    // 高亮项的注释行（横排时另起一行）；无释义的条目不占这行（v4.8.0 去拼音后没内容可画）
    let mut anno_w = 0;
    let mut anno_h = 0;
    if let Some(it) = items.get(SEL.load(Ordering::Relaxed)) {
        let s = gloss_of(it);
        if !s.is_empty() {
            anno_w = measure(dc, f_anno, &s).0;
            anno_h = measure(dc, f_anno, "x").1 + sc(ROW_PAD);
        }
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
        // v4.7.0 页码装进小药丸：宽度加药丸左右内边距，高度不变
        (measure(dc, f_anno, &s).0 + sc(PAGE_PAD_X) * 2, measure(dc, f_anno, "x").1 + sc(ROW_PAD))
    } else {
        (0, 0)
    };

    let mut hitem: Vec<(i32, i32)> = Vec::new();
    let (width, height) = if crate::settings::horizontal() {
        let inset = sc(PAD) / 2;
        let mut w = pad * 2 + inset * 2;
        let mut x = pad + inset;
        for (i, iw) in item_w.iter().take(nth).enumerate() {
            if i > 0 { w += gap; }
            w += iw;
            // 条目的悬停跨度 = 选中条矩形（x-inset .. x+iw+inset）
            hitem.push((x - inset, x + iw + inset));
            x += iw + gap;
        }
        let h = pad * 2 + pre_h + row_h + if anno_h > 0 { anno_h } else { 0 } + footer_h;
        (w.max(pad * 2 + anno_w).max(pad * 2 + top_w).max(pad * 2 + footer_w).max(sc(120)), h)
    } else {
        let w = (pad * 2 + index_w + gap + word_w + gap + ru_w)
            .max(pad * 2 + top_w)
            .max(pad * 2 + footer_w);
        let h = pad * 2 + pre_h + (nth as i32) * row_h + footer_h;
        (w, h)
    };

    Layout { index_w, word_w, item_w, hitem, row_h, line_h, anno_h, width, height }
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
            // 窗口本体 = 一张圆角卡片：先铺卡片底色，最后描一圈 1px 边框
            fill_rect(dc, 0, 0, rc.right, rc.bottom, p.card);
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
                    SetTextColor(dc, p.faint);
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
                    SetTextColor(dc, p.faint);
                    let g: Vec<u16> = s.encode_utf16().collect();
                    TextOutW(dc, tx, ty, &g);
                }
                y += ph + sc(ROW_PAD) * 2;
                // 顶行和候选之间的分隔线：画在顶行自带的下留白里，不改变几何
                fill_rect(dc, pad, y - sc(ROW_PAD), rc.right - pad, y - sc(ROW_PAD) + 1, p.border);
            }

            let sel = SEL.load(Ordering::Relaxed);
            let hot = HOT_ROW.load(Ordering::Relaxed);
            let hot_ru = HOT_RU.load(Ordering::Relaxed);
            let small_idx = ((lay.line_h - measure(dc, f_idx, "x").1) / 2).max(0);
            let small_anno = ((lay.line_h - measure(dc, f_anno, "x").1) / 2).max(0);

            if crate::settings::horizontal() {
                let inset = sc(PAD) / 2;
                let mut x = pad + inset;
                let baseline = y + sc(ROW_PAD);
                for (i, it) in items.iter().take(crate::settings::max_cands()).enumerate() {
                    let iw = lay.item_w.get(i).copied().unwrap_or(0);
                    let bar = RECT { left: x - inset, top: y, right: x + iw + inset, bottom: y + lay.row_h };
                    if i == sel {
                        fill_round(dc, &bar, sc(BAR_RADIUS), p.bar_fill);
                    } else if i as i32 == hot {
                        fill_round(dc, &bar, sc(BAR_RADIUS), p.hover);
                    }
                    let idx = format!("{}.", i + 1);
                    let iwid = measure(dc, f_idx, &idx).0;
                    let on_bar = i == sel;
                    SelectObject(dc, f_idx);
                    SetTextColor(dc, if on_bar { p.bar_text } else { p.faint });
                    let n: Vec<u16> = idx.encode_utf16().collect();
                    TextOutW(dc, x, baseline + small_idx, &n);

                    SelectObject(dc, f_text);
                    SetTextColor(dc, if on_bar { p.bar_text } else { p.text });
                    let word: Vec<u16> = it.word.encode_utf16().collect();
                    TextOutW(dc, x + iwid + gap / 2, baseline, &word);
                    x += iw + gap;
                }
                if lay.anno_h > 0 {
                    if let Some(it) = items.get(sel) {
                        let s = gloss_of(it);
                        SelectObject(dc, f_anno);
                        SetTextColor(dc, if it.fresh { p.fresh } else { p.faint });
                        let g: Vec<u16> = s.encode_utf16().collect();
                        TextOutW(dc, pad, y + lay.row_h + sc(ROW_PAD) / 2, &g);
                    }
                }
            } else {
                let x_index = pad;
                let x_word = x_index + lay.index_w + gap;
                let x_ru = x_word + lay.word_w + gap;
                for (i, it) in items.iter().take(crate::settings::max_cands()).enumerate() {
                    let ry = y + (i as i32) * lay.row_h;
                    let ru = RU_SEL.load(Ordering::Relaxed);
                    // 选中俄语侧时青条只罩住俄语那一列（←→/点俄语列进入）
                    let bar_l = |ru_side: bool| if ru_side { x_ru - pad / 2 } else { pad / 2 };
                    let bar = RECT { left: 0, top: ry, right: rc.right - pad / 2, bottom: ry + lay.row_h };
                    if i == sel {
                        let mut b = bar;
                        b.left = bar_l(ru);
                        fill_round(dc, &b, sc(BAR_RADIUS), p.bar_fill);
                    } else if i as i32 == hot {
                        let mut b = bar;
                        b.left = bar_l(hot_ru);
                        fill_round(dc, &b, sc(BAR_RADIUS), p.hover);
                    }
                    // 中文侧选中：序号/词都压在青条上；俄语列两种选中侧都压着
                    let on_bar_word = i == sel && !ru;
                    let on_bar_ru = i == sel;
                    let ty = ry + sc(ROW_PAD);
                    SelectObject(dc, f_idx);
                    SetTextColor(dc, if on_bar_word { p.bar_text } else { p.faint });
                    let n: Vec<u16> = format!("{}.", i + 1).encode_utf16().collect();
                    TextOutW(dc, x_index, ty + small_idx, &n);

                    SelectObject(dc, f_text);
                    SetTextColor(dc, if on_bar_word { p.bar_text } else { p.text });
                    let word: Vec<u16> = it.word.encode_utf16().collect();
                    TextOutW(dc, x_word, ty, &word);

                    // 俄语列字体（v4.8.0 去拼音列后这行 SelectObject 仍要给下面的
                    // 俄语 TextOutW 用，不能跟拼音块一起删）
                    SelectObject(dc, f_anno);

                    // 压着青条的列一律用条上文字色（生词橙在浅色青底上对比太弱）；
                    // 非选中行的生词照旧橙色
                    SetTextColor(dc, if on_bar_ru { p.bar_text } else if it.fresh { p.fresh } else { p.faint });
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
                // 页码装进一个小药丸（灰底 + 细描边），文字位置和老版完全一样
                let pill = RECT {
                    left: rc.right - pad - wpx - sc(PAGE_PAD_X) * 2,
                    top: rc.bottom - pad - hh - sc(ROW_PAD) / 2 - sc(PAGE_PAD_Y),
                    right: rc.right - pad,
                    bottom: rc.bottom - pad - sc(ROW_PAD) / 2 + sc(PAGE_PAD_Y),
                };
                fill_round(dc, &pill, sc(BAR_RADIUS), p.hover);
                frame_round(dc, &pill, sc(BAR_RADIUS), p.border);
                SelectObject(dc, f_anno);
                SetTextColor(dc, p.faint);
                let t: Vec<u16> = s.encode_utf16().collect();
                TextOutW(dc, pill.left + sc(PAGE_PAD_X), pill.top + sc(PAGE_PAD_Y), &t);
            }

            // 卡片外描边最后画，盖在所有内容上（窗口区域是圆角的，描边跟着圆角走）
            frame_round(dc, &RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom }, sc(WIN_RADIUS) * 2, p.border);

            // 注意：f_text / f_anno / f_idx 是缓存的字体对象，这里不能删
            let _ = (f_text, f_anno, f_idx);
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        // 悬停高亮 + 手型光标（v4.7.0）：只改视觉，不碰选中/提交。
        // 悬停行用 hit_test 同一套几何，画的和点的一致（v4.5.0 教训）。
        WM_MOUSEMOVE => {
            let x = (l.0 & 0xFFFF) as i16 as i32;
            let y = ((l.0 >> 16) & 0xFFFF) as i16 as i32;
            let (nr, nru) = match hover_at(x, y) {
                Some((i, ru)) => (i as i32, ru),
                None => (-1, false),
            };
            if nr != HOT_ROW.load(Ordering::Relaxed) || nru != HOT_RU.load(Ordering::Relaxed) {
                HOT_ROW.store(nr, Ordering::Relaxed);
                HOT_RU.store(nru, Ordering::Relaxed);
                // FALSE = 不擦背景只重画，防闪
                let _ = InvalidateRect(h, None, FALSE);
            }
            // 只挂一次 MouseLeave 追踪（TRACKED 防重复挂）。而且只有光标真在
            // 客户区里才挂：光标本来在窗外时（比如台架 PostMessage 模拟的
            // WM_MOUSEMOVE），TrackMouseEvent 会立刻回发 WM_MOUSELEAVE，
            // 悬停刚设上就被清掉。光标在窗外时由本消息自己负责清悬停。
            if !TRACKED.load(Ordering::Relaxed) {
                let mut cp = POINT::default();
                if GetCursorPos(&mut cp).is_ok() && ScreenToClient(h, &mut cp).as_bool() {
                    let mut cr = RECT::default();
                    if GetClientRect(h, &mut cr).is_ok()
                        && cp.x >= 0 && cp.y >= 0 && cp.x < cr.right && cp.y < cr.bottom
                    {
                        TRACKED.store(true, Ordering::Relaxed);
                        let mut tme = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: h,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&mut tme);
                    }
                }
            }
            LRESULT(0)
        }
        // windows-rs 0.52 的 WM_MOUSELEAVE 是复合常量表达式，不能当 match 模式：
        // rustc 会把它当通配臂、把后面所有臂判成不可达直接从二进制里删掉（v4.6.0 大坑）。
        // 这里用字面量 0x02A3（= WM_MOUSELEAVE）。
        0x02A3 => {
            HOT_ROW.store(-1, Ordering::Relaxed);
            HOT_RU.store(false, Ordering::Relaxed);
            TRACKED.store(false, Ordering::Relaxed);
            let _ = InvalidateRect(h, None, FALSE);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            // 悬停在候选行上时给手型光标
            if ((l.0 & 0xFFFF) as u16 as u32) == HTCLIENT && HOT_ROW.load(Ordering::Relaxed) >= 0 {
                if let Ok(cur) = LoadCursorW(None, IDC_HAND) {
                    let _ = SetCursor(cur);
                }
                LRESULT(1)
            } else {
                DefWindowProcW(h, msg, w, l)
            }
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
    // 悬停状态跟着清掉（行数可能变了，旧行号没意义）
    HOT_ROW.store(-1, Ordering::Relaxed);
    HOT_RU.store(false, Ordering::Relaxed);
    // 数据变了立即重画：翻页键（PageDown/-/=）只走 set_items/set_page，
    // 以前不重画，窗口要等别的重绘触发才更新（v4.7.0 修）
    if alive() { let _ = InvalidateRect(hwnd(), None, TRUE); }
}

/// 记下页码（画右下角的「▲▼ n/总」）。cur 是 0 起的页号。
pub unsafe fn set_page(cur: usize, total: usize) {
    PAGE_CUR.store(cur, Ordering::Relaxed);
    PAGE_TOT.store(total.max(1), Ordering::Relaxed);
    if alive() { let _ = InvalidateRect(hwnd(), None, TRUE); }
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

/// ←/→ 切换选中侧：青条从整行挪到俄语那一列（或挪回来），触发重画。
pub unsafe fn set_ru_sel(v: bool) {
    RU_SEL.store(v, Ordering::Relaxed);
    if alive() { let _ = InvalidateRect(hwnd(), None, TRUE); }
}

/// 竖排几何（和 WM_PAINT 同一套算法）：首行 y、行高、词列 x、俄语列 x。
/// 顶行（拼音 / ☁ 提示）占多高就先加多高，这样点击和画的位置对得上。
unsafe fn v_geom(dc: HDC) -> Option<(i32, i32, i32, i32, Layout)> {
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
    let x_ru = x_word + lay.word_w + gap;
    Some((y, lay.row_h, x_word, x_ru, lay))
}

/// 鼠标点 (x, y)（客户区坐标）落在哪一行、哪一侧：(行号, 是否俄语列)。
/// 点不到候选行返回 None。索引列和词列都算中文侧。
pub unsafe fn hit_test(x: i32, y: i32) -> Option<(usize, bool)> {
    let h = hwnd();
    let dc = GetDC(h);
    if dc.0 == 0 { return None; }
    let r = v_geom(dc);
    if dc.0 != 0 { let _ = ReleaseDC(h, dc); }
    let (y0, row_h, _x_word, x_ru, _lay) = r?;
    let dy = y - y0;
    if dy < 0 { return None; }
    let i = (dy / row_h) as usize;
    if i >= ITEMS.lock().unwrap().len() { return None; }
    if x >= x_ru {
        Some((i, true))
    } else if x >= 0 {
        // 俄语列左边整块（含序号、词列）都算中文侧，点空白也能选
        Some((i, false))
    } else {
        None
    }
}

/// 鼠标悬停命中（只画高亮，不改变点击语义）：竖排直接走 hit_test；
/// 横排所有条目共一行，hit_test 的 y→行公式会把整行都判成第 0 条 ——
/// 改按条目 x 跨度找（compute_layout 里和 WM_PAINT 同一套累加）。
unsafe fn hover_at(x: i32, y: i32) -> Option<(usize, bool)> {
    if !crate::settings::horizontal() {
        return hit_test(x, y);
    }
    let h = hwnd();
    let dc = GetDC(h);
    if dc.0 == 0 { return None; }
    let items = ITEMS.lock().unwrap();
    if items.is_empty() { drop(items); let _ = ReleaseDC(h, dc); return None; }
    let lay = compute_layout(dc, &items);
    drop(items);
    // 和 v_geom 同款：顶行（拼音 / ☁ 提示）占多高就先加多高
    let mut y0 = sc(PAD);
    let pre = PREEDIT.lock().unwrap().clone();
    let sent = SENTENCE.lock().unwrap().clone();
    if !pre.is_empty() || sent.is_some() {
        let f_anno = font(ANNO_PX);
        let (_, ph) = measure(dc, f_anno, "x");
        y0 += ph + sc(ROW_PAD) * 2;
    }
    let _ = ReleaseDC(h, dc);
    if y < y0 || y >= y0 + lay.row_h { return None; }
    for (i, (l, r)) in lay.hitem.iter().enumerate() {
        if x >= *l && x < *r { return Some((i, false)); }
    }
    None
}

/// 第 row 行中文侧/俄语侧的可点击矩形（客户区坐标），测试台架算点击点用。
pub unsafe fn hit_rect(row: usize, ru: bool) -> Option<(i32, i32, i32, i32)> {
    let h = hwnd();
    let dc = GetDC(h);
    if dc.0 == 0 { return None; }
    let r = v_geom(dc);
    if dc.0 != 0 { let _ = ReleaseDC(h, dc); }
    let (y0, row_h, x_word, x_ru, _lay) = r?;
    if row >= ITEMS.lock().unwrap().len() { return None; }
    let top = y0 + row as i32 * row_h;
    // v4.8.0 去拼音列后中文侧矩形跨整个词列（x_word..x_ru），中心必落在中文侧；
    // 旧固定 60px 宽在窄词（word_w+gap<30）时中心会越过 x_ru 翻到俄语侧
    Some(if ru { (x_ru, top, x_ru + 60, top + row_h) } else { (x_word, top, x_ru, top + row_h) })
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

/// 横排第 row 条目的悬停跨度 (l, r) 写进 out[2]（竖排没有跨度，返回 0）。
/// 台架横排悬停模拟用 —— hit_rect 是竖排几何，横排条目没有对应矩形，只能走跨度。
#[no_mangle]
pub unsafe extern "system" fn cand_span_export(row: u32, out: *mut i32) -> u32 {
    if out.is_null() { return 0; }
    let h = hwnd();
    let dc = GetDC(h);
    if dc.0 == 0 { return 0; }
    let r = v_geom(dc);
    let _ = ReleaseDC(h, dc);
    let (_y0, _row_h, _xw, _xr, lay) = match r { Some(v) => v, None => return 0 };
    match lay.hitem.get(row as usize) {
        Some((l, rr)) => {
            *out.add(0) = *l;
            *out.add(1) = *rr;
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
    // 夹进屏幕：右边超出就左移，下方放不下就翻到光标上方
    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);
    let mut px = x;
    let mut py = y;
    if px + cw > sw - 4 { px = (sw - cw - 4).max(0); }
    if py + ch > sh - 4 { py = (y - ch - sc(30)).max(0); }
    if py < 0 { py = 0; }
    let rgn = CreateRoundRectRgn(0, 0, cw + 1, ch + 1, sc(WIN_RADIUS) * 2, sc(WIN_RADIUS) * 2);
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

pub unsafe fn hide() {
    // 窗口可能是在鼠标下面被藏掉的（那样发不出 WM_MOUSELEAVE）：
    // 悬停状态在这里清掉，不然下次显示会闪一下残留的悬停条
    HOT_ROW.store(-1, Ordering::Relaxed);
    HOT_RU.store(false, Ordering::Relaxed);
    TRACKED.store(false, Ordering::Relaxed);
    if alive() { let _ = ShowWindow(hwnd(), SW_HIDE); }
}