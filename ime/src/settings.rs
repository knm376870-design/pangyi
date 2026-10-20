//! 设置面板 + 配置读写。
//!
//! 双击桌面「旁译.exe」打开的面板：候选个数 / 字号 / 排版 / 主题 / 配色 / 中文标点 /
//! 翻译模式 / 自定义短语。v4.10.0 加「配色」行（青/粉/橙/紫/绿，只换强调色）。
//! v4.6.0 重画成浅色圆角卡片 + 头部渐变横幅，
//! 左边熊猫、右边套娃（全 GDI 圆/椭圆程序化绘制，无图片资产、无版权问题）。
//! 配置存在 %LOCALAPPDATA%\Pangyi\settings.txt，候选窗启动时读一次。
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::Mutex;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};

static MAX_CANDS: AtomicUsize = AtomicUsize::new(9);
static FONT_SCALE: AtomicI32 = AtomicI32::new(100);
static HORIZONTAL: AtomicBool = AtomicBool::new(false);
static ENGLISH: AtomicBool = AtomicBool::new(false);
/// 表情模式：开 = 拼音正好撞上 emoji/颜文字 关键词时，表情候选排最前。
/// 默认关 —— 老用户打字行为不变。
static EMOJI: AtomicBool = AtomicBool::new(false);
static THEME: AtomicI32 = AtomicI32::new(0); // 0 跟随系统 / 1 浅色 / 2 深色
/// 调试日志开关。默认关 —— 开着的话每敲一个键都要写一次文件，会明显拖慢输入。
static DEBUG: AtomicBool = AtomicBool::new(false);
/// 中文标点（智能全角）开关。默认开 —— 中文模式下标点键直接出全角（，。、）等。
static CNPUNCT: AtomicBool = AtomicBool::new(true);
/// 翻译模式：0 关 / 1 逐字（每个词上屏时俄语跟在后面）/ 2 整句（候选窗实时
/// 显示整句翻译，句末标点或回车时整句俄语上屏）。默认关 —— 老用户行为不变。
static TRMODE: AtomicI32 = AtomicI32::new(0);
/// 配色（强调色）：0 青 / 1 粉 / 2 橙 / 3 紫 / 4 绿。默认青 —— 老用户行为不变。
static ACCENT: AtomicI32 = AtomicI32::new(0);

static STANDALONE: AtomicBool = AtomicBool::new(false);
static mut WND: HWND = HWND(0);
static mut BG: HBRUSH = HBRUSH(0);
/// 外层窗口尺寸（客户区 360×564 + 标题栏/边框）。init() 里用 AdjustWindowRectEx
/// 反推 —— 直接把 360×564 传给 CreateWindowExW 的话那是「外层」尺寸，客户区会被
/// 窗框吃掉一块，底部关闭药丸（footer_y=526..556）直接裁掉一截（v4.6.0 踩坑）。
static mut OUTER_W: i32 = 0;
static mut OUTER_H: i32 = 0;
/// 悬停的药丸：0=无、-1=关闭钮、1..26=对应动作号（画悬停底色 + 手型光标用）
static HOT: AtomicI32 = AtomicI32::new(0);
/// 是否已挂过 TrackMouseEvent（挂一次就能收 WM_MOUSELEAVE）
static TRACKED: AtomicBool = AtomicBool::new(false);

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
                "debug" => DEBUG.store(v == "1" || v.eq_ignore_ascii_case("true"), Ordering::Relaxed),
                "cnpunct" => CNPUNCT.store(v == "1" || v.eq_ignore_ascii_case("true"), Ordering::Relaxed),
                "trmode" => if let Ok(n) = v.parse::<i32>() {
                    TRMODE.store(n.clamp(0, 2), Ordering::Relaxed);
                },
                "accent" => if let Ok(n) = v.parse::<i32>() {
                    ACCENT.store(n.clamp(0, 4), Ordering::Relaxed);
                },
                // v4.15.0 补上：老版本 save() 写 english 却不读 —— 面板里打开
                // 英文模式，IME 每次激活都读不回 true，英文模式形同虚设。
                "english" => ENGLISH.store(v == "1" || v.eq_ignore_ascii_case("true"), Ordering::Relaxed),
                "emoji" => EMOJI.store(v == "1" || v.eq_ignore_ascii_case("true"), Ordering::Relaxed),
                _ => {}
            }
        }
    }
}

/// 热重载两个模式开关（english/emoji，v4.16.0）。
///
/// 面板在【别的进程】里点「英文模式/表情模式」只写 settings.txt，宿主里
/// 运行中的 IME 实例激活时读的那份就旧了——用户开了开关却一个表情不出
/// （v4.15.0「表情打不出来」的根因）。status 轮询每 500ms 查一次文件
/// 变化时间，变了就调这个：只重读这两个纯开关，其余设置保持激活值
/// （打字中途改候选数/主题/窗口几何会闪，不动它们）。
pub fn reload_modes() {
    if let Ok(s) = std::fs::read_to_string(cfg_file()) {
        for line in s.lines() {
            let mut it = line.splitn(2, '=');
            let (Some(k), Some(v)) = (it.next(), it.next()) else {
                continue;
            };
            let on = v.trim() == "1" || v.trim().eq_ignore_ascii_case("true");
            match k.trim() {
                "english" => ENGLISH.store(on, Ordering::Relaxed),
                "emoji" => EMOJI.store(on, Ordering::Relaxed),
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
        "max={}\nscale={}\nlayout={}\ntheme={}\nenglish={}\ndebug={}\ncnpunct={}\ntrmode={}\naccent={}\nemoji={}\n",
        max_cands(), font_scale(),
        if horizontal() { "h" } else { "v" },
        THEME.load(Ordering::Relaxed),
        if english() { 1 } else { 0 },
        if debug() { 1 } else { 0 },
        if cnpunct() { 1 } else { 0 },
        TRMODE.load(Ordering::Relaxed),
        ACCENT.load(Ordering::Relaxed),
        if emoji() { 1 } else { 0 }
    );
    let _ = std::fs::write(path, body);
}

/// 是否写调试日志（排查问题时把 settings.txt 里 debug 改成 1）
pub fn debug() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

pub fn set_english(v: bool) {
    ENGLISH.store(v, Ordering::Relaxed);
    save();
}

pub fn emoji() -> bool {
    EMOJI.load(Ordering::Relaxed)
}

pub fn set_emoji(v: bool) {
    EMOJI.store(v, Ordering::Relaxed);
    save();
}

pub fn cnpunct() -> bool {
    CNPUNCT.load(Ordering::Relaxed)
}

pub fn set_cnpunct(v: bool) {
    CNPUNCT.store(v, Ordering::Relaxed);
    save();
}

pub fn trmode() -> i32 {
    TRMODE.load(Ordering::Relaxed)
}

pub fn set_trmode(v: i32) {
    TRMODE.store(v.clamp(0, 2), Ordering::Relaxed);
    save();
}

pub fn accent() -> i32 {
    ACCENT.load(Ordering::Relaxed)
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

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn sc(px: i32) -> i32 {
    let d = unsafe { crate::cand::dpi() };
    (px * d) / 96
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

/// 面板色板：浅色圆角卡片风；深色主题换一套卡片/描边色，卡通主体色不变。
struct Palette {
    text: COLORREF,
    faint: COLORREF,
    canvas: COLORREF,
    card: COLORREF,
    border: COLORREF,
    grad_top: COLORREF,
    pill_sel_fill: COLORREF,
    pill_sel_text: COLORREF,
    pill_hover: COLORREF,
    accent: COLORREF,
    accent_text: COLORREF,
    panda_ink: COLORREF,
    panda_white: COLORREF,
    panda_blush: COLORREF,
    mat_red: COLORREF,
    mat_red_dark: COLORREF,
    mat_skin: COLORREF,
    mat_gold: COLORREF,
}

/// 一套强调色：fill=选中底色（条/药丸）、text=底色上的字、strong=☁提示/关闭钮、
/// on_strong=strong 底上的字。cand.rs 候选窗色板也从这张表取 —— 唯一真源。
pub struct Accent {
    pub fill: COLORREF,
    pub text: COLORREF,
    pub strong: COLORREF,
    pub on_strong: COLORREF,
}

/// 5 配色 × 浅/深 两套强调色（v4.10.0）。0 青 = 老版本原色。
pub const ACCENTS: [[Accent; 2]; 5] = [
    [ // 青
        Accent { fill: rgb(0xdd, 0xf2, 0xf6), text: rgb(0x17, 0x60, 0x6c), strong: rgb(0x2f, 0xb8, 0xcc), on_strong: rgb(0xff, 0xff, 0xff) },
        Accent { fill: rgb(0x1e, 0x3a, 0x44), text: rgb(0xb7, 0xe6, 0xf0), strong: rgb(0x40, 0xc8, 0xe0), on_strong: rgb(0x06, 0x22, 0x2a) },
    ],
    [ // 粉
        Accent { fill: rgb(0xf8, 0xe4, 0xec), text: rgb(0x82, 0x2e, 0x54), strong: rgb(0xd4, 0x6a, 0x9c), on_strong: rgb(0xff, 0xff, 0xff) },
        Accent { fill: rgb(0x44, 0x22, 0x34), text: rgb(0xf4, 0xc8, 0xda), strong: rgb(0xe0, 0x7a, 0xb0), on_strong: rgb(0x2a, 0x0c, 0x18) },
    ],
    [ // 橙
        Accent { fill: rgb(0xfa, 0xee, 0xe0), text: rgb(0x8a, 0x4a, 0x1a), strong: rgb(0xe0, 0x96, 0x40), on_strong: rgb(0xff, 0xff, 0xff) },
        Accent { fill: rgb(0x44, 0x30, 0x1e), text: rgb(0xf4, 0xd6, 0xb0), strong: rgb(0xe8, 0xa0, 0x4a), on_strong: rgb(0x2a, 0x18, 0x06) },
    ],
    [ // 紫
        Accent { fill: rgb(0xee, 0xe6, 0xfa), text: rgb(0x5a, 0x3a, 0x96), strong: rgb(0x9a, 0x70, 0xd8), on_strong: rgb(0xff, 0xff, 0xff) },
        Accent { fill: rgb(0x32, 0x26, 0x4a), text: rgb(0xd6, 0xc6, 0xf4), strong: rgb(0xa8, 0x82, 0xe4), on_strong: rgb(0x1a, 0x10, 0x2e) },
    ],
    [ // 绿
        Accent { fill: rgb(0xe4, 0xf4, 0xe8), text: rgb(0x26, 0x68, 0x3c), strong: rgb(0x3e, 0xb0, 0x60), on_strong: rgb(0xff, 0xff, 0xff) },
        Accent { fill: rgb(0x1e, 0x3c, 0x2a), text: rgb(0xc8, 0xee, 0xd4), strong: rgb(0x4a, 0xcc, 0x72), on_strong: rgb(0x0a, 0x20, 0x12) },
    ],
];

fn palette(accent: i32, dark: bool) -> Palette {
    let mut p = if dark {
        Palette {
            text: rgb(0xf5, 0xf5, 0xf7),
            faint: rgb(0xae, 0xae, 0xb2),
            canvas: rgb(0x20, 0x20, 0x23),
            card: rgb(0x2a, 0x2a, 0x2d),
            border: rgb(0x3a, 0x3a, 0x3e),
            grad_top: rgb(0x16, 0x32, 0x3c),
            pill_sel_fill: rgb(0x1e, 0x3a, 0x44),
            pill_sel_text: rgb(0xb7, 0xe6, 0xf0),
            pill_hover: rgb(0x33, 0x33, 0x37),
            accent: rgb(0x40, 0xc8, 0xe0),
            accent_text: rgb(0x06, 0x22, 0x2a),
            panda_ink: rgb(0xd9, 0xd9, 0xde),
            panda_white: rgb(0xff, 0xff, 0xff),
            panda_blush: rgb(0xb5, 0x6a, 0x7c),
            mat_red: rgb(0xd9, 0x4f, 0x4f),
            mat_red_dark: rgb(0xa9, 0x3a, 0x3a),
            mat_skin: rgb(0xff, 0xe3, 0xc9),
            mat_gold: rgb(0xff, 0xd3, 0x4d),
        }
    } else {
        Palette {
            text: rgb(0x1d, 0x1d, 0x1f),
            faint: rgb(0x6b, 0x6b, 0x70),
            canvas: rgb(0xed, 0xf0, 0xf3),
            card: rgb(0xff, 0xff, 0xff),
            border: rgb(0xe3, 0xe6, 0xea),
            grad_top: rgb(0xd9, 0xee, 0xf5),
            pill_sel_fill: rgb(0xdd, 0xf2, 0xf6),
            pill_sel_text: rgb(0x17, 0x60, 0x6c),
            pill_hover: rgb(0xe9, 0xeb, 0xee),
            accent: rgb(0x2f, 0xb8, 0xcc),
            accent_text: rgb(0xff, 0xff, 0xff),
            panda_ink: rgb(0x3a, 0x3a, 0x3e),
            panda_white: rgb(0xff, 0xff, 0xff),
            panda_blush: rgb(0xf5, 0xa8, 0xb8),
            mat_red: rgb(0xd9, 0x4f, 0x4f),
            mat_red_dark: rgb(0xa9, 0x3a, 0x3a),
            mat_skin: rgb(0xff, 0xe3, 0xc9),
            mat_gold: rgb(0xff, 0xd3, 0x4d),
        }
    };
    // v4.10.0 配色：基板（卡片/描边/正文）不动，只覆写强调位
    let a = &ACCENTS[accent as usize][dark as usize];
    p.pill_sel_fill = a.fill;
    p.pill_sel_text = a.text;
    p.accent = a.strong;
    p.accent_text = a.on_strong;
    p
}

/// 字体缓存：悬停重绘会频繁建字体，按 (字号, DPI) 复用，画完不删。
static FONTS: Mutex<Vec<(i32, i32, HFONT)>> = Mutex::new(Vec::new());

fn font(px: i32) -> HFONT {
    let d = unsafe { crate::cand::dpi() };
    {
        let g = FONTS.lock().unwrap();
        if let Some((_, _, h)) = g.iter().find(|(p, dd, _)| *p == px && *dd == d) {
            return *h;
        }
    }
    let h = unsafe {
        CreateFontW(
            -sc(px), 0, 0, 0, 400, 0, 0, 0,
            DEFAULT_CHARSET.0 as u32, OUT_TT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32, (VARIABLE_PITCH.0 | FF_DONTCARE.0) as u32,
            w!("Microsoft YaHei UI"),
        )
    };
    FONTS.lock().unwrap().push((px, d, h));
    h
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

// 窗口与布局（逻辑像素，经 sc() 按 DPI 缩放）
const WIN_W: i32 = 360;
// 10 行卡底 = 108+10×44+9×8 = 620，FOOTER_Y 留 10，窗口高 = 底 + 38
// （v4.15.0 加「表情模式」行：9 行 → 10 行，WIN_H 616→658、FOOTER_Y 578→630）
const WIN_H: i32 = 658;
const HEADER_H: i32 = 96; // 头部渐变横幅（卡通 + 标题）
const MARGIN: i32 = 12; // 卡片左右留白
const CARDS_TOP: i32 = 108;
const CARD_H: i32 = 44;
const CARD_GAP: i32 = 8;
const CARD_RADIUS: i32 = 10;
const LABEL_X: i32 = 24;
const PILL_PAD_X: i32 = 10;
const PILL_PAD_Y: i32 = 4;
const PILL_GAP: i32 = 8;
const PILL_RADIUS: i32 = 8;
const FOOTER_Y: i32 = 630; // 底部统计 + 关闭按钮

/// 每行的选项：标签 + 选项文本 + 动作号
fn rows() -> Vec<(&'static str, Vec<(&'static str, i32)>)> {
    vec![
        ("候选个数", vec![("5", 1), ("7", 2), ("9", 3)]),
        ("字号", vec![("小", 4), ("标准", 5), ("大", 6)]),
        ("排版", vec![("竖排", 7), ("横排", 8)]),
        ("主题", vec![("跟随系统", 9), ("浅色", 10), ("深色", 11)]),
        ("配色", vec![("青", 18), ("粉", 19), ("橙", 20), ("紫", 21), ("绿", 22)]),
        ("中文标点", vec![("开", 13), ("关", 14)]),
        ("翻译模式", vec![("关", 15), ("逐字", 16), ("整句", 17)]),
        // v4.13.0：英文模式（微软同款中/英开关）。开 = 字母/标点全直通英文，
        // 不进拼音组合、不翻译；Shift 单击切换和这个开关写同一个设置。
        ("英文模式", vec![("开", 23), ("关", 24)]),
        // v4.15.0：表情模式。开 = 拼音正好撞上表情关键词（xiao/ku/aini…）时
        // emoji + 颜文字候选排最前，空格上屏表情。
        ("表情模式", vec![("开", 25), ("关", 26)]),
        ("自定义短语", vec![("打开短语文件", 12)]),
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
        18..=22 => act == 18 + accent(),
        13 | 14 => act == if cnpunct() { 13 } else { 14 },
        15..=17 => act == 15 + trmode(),
        23 | 24 => act == if english() { 23 } else { 24 },
        25 | 26 => act == if emoji() { 25 } else { 26 },
        _ => false,
    }
}

// ---------------- 绘制助手 ----------------

/// 圆角矩形填充：区域和刷子成对创建/删除（GDI 对象不能漏）
unsafe fn fill_round(dc: HDC, r: &RECT, radius: i32, color: COLORREF) {
    let rb = CreateSolidBrush(color);
    let rgn = CreateRoundRectRgn(r.left, r.top, r.right, r.bottom, radius, radius);
    FillRgn(dc, rgn, rb);
    let _ = DeleteObject(rgn);
    let _ = DeleteObject(rb);
}

/// 圆角矩形 1px 描边
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

/// 无边框椭圆（选 NULL_PEN 再画，画完恢复旧笔旧刷）
unsafe fn fill_ellipse(dc: HDC, l: i32, t: i32, r: i32, b: i32, color: COLORREF) {
    let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
    let rb = CreateSolidBrush(color);
    let old_brush = SelectObject(dc, rb);
    let _ = Ellipse(dc, l, t, r, b);
    SelectObject(dc, old_brush);
    let _ = DeleteObject(rb);
    SelectObject(dc, old_pen);
}

/// 圆心 + 半轴椭圆
unsafe fn ellipse_at(dc: HDC, cx: i32, cy: i32, rx: i32, ry: i32, color: COLORREF) {
    fill_ellipse(dc, cx - rx, cy - ry, cx + rx, cy + ry, color);
}

/// 无边框弦：弦的两个端点取矩形水平中线 → 填下半月牙（熊猫的嘴）
unsafe fn fill_chord(dc: HDC, l: i32, t: i32, r: i32, b: i32, color: COLORREF) {
    let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
    let rb = CreateSolidBrush(color);
    let old_brush = SelectObject(dc, rb);
    let my = (t + b) / 2;
    let _ = Chord(dc, l, t, r, b, l, my, r, my);
    SelectObject(dc, old_brush);
    let _ = DeleteObject(rb);
    SelectObject(dc, old_pen);
}

/// 垂直渐变（GradientFill / msimg32）。万一失败退路逐行 FillRect。
unsafe fn gradient_v(dc: HDC, r: &RECT, top: COLORREF, bottom: COLORREF) {
    let ch = |c: COLORREF, shift: u32| (((c.0 >> shift) & 0xFF) << 8) as u16;
    let verts = [
        TRIVERTEX { x: r.left, y: r.top, Red: ch(top, 0), Green: ch(top, 8), Blue: ch(top, 16), Alpha: 0 },
        TRIVERTEX { x: r.right, y: r.bottom, Red: ch(bottom, 0), Green: ch(bottom, 8), Blue: ch(bottom, 16), Alpha: 0 },
    ];
    let mesh = GRADIENT_RECT { UpperLeft: 0, LowerRight: 1 };
    if GradientFill(dc, &verts, &mesh as *const GRADIENT_RECT as *const c_void, 1, GRADIENT_FILL_RECT_V)
        .as_bool()
    {
        return;
    }
    let h = r.bottom - r.top;
    for i in 0..h.max(1) {
        let t = i as f32 / h.max(1) as f32;
        let l = |s: u32| {
            let a = ((top.0 >> s) & 0xFF) as f32;
            let b = ((bottom.0 >> s) & 0xFF) as f32;
            ((a + (b - a) * t).round() as u32) << s
        };
        let rb = CreateSolidBrush(COLORREF(l(0) | l(8) | l(16)));
        FillRect(dc, &RECT { left: r.left, top: r.top + i, right: r.right, bottom: r.top + i + 1 }, rb);
        let _ = DeleteObject(rb);
    }
}

// ---- 卡通：熊猫 + 套娃（全 GDI 圆/椭圆，坐标以 s 为单位，改这些数就能调萌度）----
const OUTLINE: i32 = 2; // 描边层多出的像素（先画大一圈的深色当描边）

const PANDA_EAR_R: f64 = 0.16;
const PANDA_EAR_X: f64 = 0.33;
const PANDA_EAR_Y: f64 = -0.28;
const PANDA_HEAD_RX: f64 = 0.40;
const PANDA_HEAD_RY: f64 = 0.36;
const PANDA_HEAD_CY: f64 = 0.02;
const PANDA_EYE_RX: f64 = 0.085;
const PANDA_EYE_RY: f64 = 0.12;
const PANDA_EYE_X: f64 = 0.15;
const PANDA_EYE_Y: f64 = 0.00;
const PANDA_PUPIL_R: f64 = 0.033;
const PANDA_PUPIL_Y: f64 = -0.045;
const PANDA_NOSE_RX: f64 = 0.042;
const PANDA_NOSE_RY: f64 = 0.028;
const PANDA_NOSE_Y: f64 = 0.12;
const PANDA_BLUSH_R: f64 = 0.065;
const PANDA_BLUSH_X: f64 = 0.27;
const PANDA_BLUSH_Y: f64 = 0.15;

/// 熊猫头：黑耳朵 + 白脸（贴纸描边）+ 黑眼圈 + 白瞳孔 + 鼻 + 笑 + 腮红
unsafe fn draw_panda(dc: HDC, cx: i32, cy: i32, s: i32, p: &Palette) {
    let f = |u: f64| (u * s as f64).round() as i32;
    // 耳朵
    ellipse_at(dc, cx - f(PANDA_EAR_X), cy + f(PANDA_EAR_Y), f(PANDA_EAR_R), f(PANDA_EAR_R), p.panda_ink);
    ellipse_at(dc, cx + f(PANDA_EAR_X), cy + f(PANDA_EAR_Y), f(PANDA_EAR_R), f(PANDA_EAR_R), p.panda_ink);
    // 头：先画大一圈的 ink 椭圆当描边，再盖白椭圆
    ellipse_at(dc, cx, cy + f(PANDA_HEAD_CY), f(PANDA_HEAD_RX) + OUTLINE, f(PANDA_HEAD_RY) + OUTLINE, p.panda_ink);
    ellipse_at(dc, cx, cy + f(PANDA_HEAD_CY), f(PANDA_HEAD_RX), f(PANDA_HEAD_RY), p.panda_white);
    // 黑眼圈 + 白瞳孔
    ellipse_at(dc, cx - f(PANDA_EYE_X), cy + f(PANDA_EYE_Y), f(PANDA_EYE_RX), f(PANDA_EYE_RY), p.panda_ink);
    ellipse_at(dc, cx + f(PANDA_EYE_X), cy + f(PANDA_EYE_Y), f(PANDA_EYE_RX), f(PANDA_EYE_RY), p.panda_ink);
    ellipse_at(dc, cx - f(PANDA_EYE_X), cy + f(PANDA_PUPIL_Y), f(PANDA_PUPIL_R), f(PANDA_PUPIL_R), p.panda_white);
    ellipse_at(dc, cx + f(PANDA_EYE_X), cy + f(PANDA_PUPIL_Y), f(PANDA_PUPIL_R), f(PANDA_PUPIL_R), p.panda_white);
    // 鼻子 + 嘴（下半月牙）+ 腮红
    ellipse_at(dc, cx, cy + f(PANDA_NOSE_Y), f(PANDA_NOSE_RX), f(PANDA_NOSE_RY), p.panda_ink);
    fill_chord(dc, cx - f(0.055), cy + f(0.15), cx + f(0.055), cy + f(0.21), p.panda_ink);
    ellipse_at(dc, cx - f(PANDA_BLUSH_X), cy + f(PANDA_BLUSH_Y), f(PANDA_BLUSH_R), f(PANDA_BLUSH_R), p.panda_blush);
    ellipse_at(dc, cx + f(PANDA_BLUSH_X), cy + f(PANDA_BLUSH_Y), f(PANDA_BLUSH_R), f(PANDA_BLUSH_R), p.panda_blush);
}

const MAT_BODY_X: f64 = 0.34;
const MAT_BODY_TOP: f64 = -0.06;
const MAT_BODY_BOT: f64 = 0.44;
const MAT_HEAD_R: f64 = 0.22;
const MAT_HEAD_CY: f64 = -0.28;
const MAT_FACE_R: f64 = 0.15;
const MAT_FACE_CY: f64 = -0.29;
const MAT_SCARF_W: f64 = 0.20;
const MAT_SCARF_TOP: f64 = -0.215;
const MAT_SCARF_BOT: f64 = -0.135;
const MAT_EYE_X: f64 = 0.055;
const MAT_EYE_Y: f64 = -0.33;
const MAT_EYE_R: f64 = 0.022;
const MAT_SMILE_RX: f64 = 0.038;
const MAT_SMILE_RY: f64 = 0.02;
const MAT_SMILE_Y: f64 = -0.24;
const MAT_BLUSH_X: f64 = 0.105;
const MAT_BLUSH_Y: f64 = -0.245;
const MAT_BLUSH_R: f64 = 0.038;
const MAT_APRON_X: f64 = 0.17;
const MAT_APRON_TOP: f64 = 0.06;
const MAT_APRON_BOT: f64 = 0.42;
const MAT_FLOWER_Y1: f64 = 0.17;
const MAT_FLOWER_Y2: f64 = 0.30;
const MAT_FLOWER_C: f64 = 0.04;
const MAT_PETAL: f64 = 0.018;
const MAT_PETAL_OFF: f64 = 0.06;

/// 套娃：红身体 + 红头（贴纸描边）+ 肤色脸 + 金围巾 + 眼笑腮红 + 白围裙 + 两朵花
unsafe fn draw_matryoshka(dc: HDC, cx: i32, cy: i32, s: i32, p: &Palette) {
    let f = |u: f64| (u * s as f64).round() as i32;
    // 身体：深红描边层 + 红胶囊
    let body = RECT {
        left: cx - f(MAT_BODY_X),
        top: cy + f(MAT_BODY_TOP),
        right: cx + f(MAT_BODY_X),
        bottom: cy + f(MAT_BODY_BOT),
    };
    let body_out = RECT {
        left: body.left - OUTLINE,
        top: body.top - OUTLINE,
        right: body.right + OUTLINE,
        bottom: body.bottom + OUTLINE,
    };
    fill_round(dc, &body_out, f(MAT_BODY_X) + OUTLINE, p.mat_red_dark);
    fill_round(dc, &body, f(MAT_BODY_X), p.mat_red);
    // 头 + 脸
    ellipse_at(dc, cx, cy + f(MAT_HEAD_CY), f(MAT_HEAD_R) + OUTLINE, f(MAT_HEAD_R) + OUTLINE, p.mat_red_dark);
    ellipse_at(dc, cx, cy + f(MAT_HEAD_CY), f(MAT_HEAD_R), f(MAT_HEAD_R), p.mat_red);
    ellipse_at(dc, cx, cy + f(MAT_FACE_CY), f(MAT_FACE_R), f(MAT_FACE_R), p.mat_skin);
    // 金围巾横条（盖住脸的下巴边）
    fill_rect(dc, cx - f(MAT_SCARF_W), cy + f(MAT_SCARF_TOP), cx + f(MAT_SCARF_W), cy + f(MAT_SCARF_BOT), p.mat_gold);
    // 五官 + 腮红
    ellipse_at(dc, cx - f(MAT_EYE_X), cy + f(MAT_EYE_Y), f(MAT_EYE_R), f(MAT_EYE_R), p.panda_ink);
    ellipse_at(dc, cx + f(MAT_EYE_X), cy + f(MAT_EYE_Y), f(MAT_EYE_R), f(MAT_EYE_R), p.panda_ink);
    ellipse_at(dc, cx, cy + f(MAT_SMILE_Y), f(MAT_SMILE_RX), f(MAT_SMILE_RY), p.panda_ink);
    ellipse_at(dc, cx - f(MAT_BLUSH_X), cy + f(MAT_BLUSH_Y), f(MAT_BLUSH_R), f(MAT_BLUSH_R), p.panda_blush);
    ellipse_at(dc, cx + f(MAT_BLUSH_X), cy + f(MAT_BLUSH_Y), f(MAT_BLUSH_R), f(MAT_BLUSH_R), p.panda_blush);
    // 白围裙 + 两朵小花（金蕊 + 4 白瓣）
    let apron = RECT {
        left: cx - f(MAT_APRON_X),
        top: cy + f(MAT_APRON_TOP),
        right: cx + f(MAT_APRON_X),
        bottom: cy + f(MAT_APRON_BOT),
    };
    fill_round(dc, &apron, f(MAT_APRON_X), p.panda_white);
    for fy in [MAT_FLOWER_Y1, MAT_FLOWER_Y2] {
        let yy = cy + f(fy);
        let off = f(MAT_PETAL_OFF);
        let pr = f(MAT_PETAL);
        ellipse_at(dc, cx, yy - off, pr, pr, p.panda_white);
        ellipse_at(dc, cx, yy + off, pr, pr, p.panda_white);
        ellipse_at(dc, cx - off, yy, pr, pr, p.panda_white);
        ellipse_at(dc, cx + off, yy, pr, pr, p.panda_white);
        ellipse_at(dc, cx, yy, f(MAT_FLOWER_C), f(MAT_FLOWER_C), p.mat_gold);
    }
}

/// 头部横幅：渐变底 + 左熊猫右套娃 + 居中标题
unsafe fn draw_header(dc: HDC, w: i32, p: &Palette) {
    gradient_v(
        dc,
        &RECT { left: 0, top: 0, right: w, bottom: sc(HEADER_H) },
        p.grad_top,
        p.canvas,
    );
    draw_panda(dc, sc(92), sc(62), sc(48), p);
    draw_matryoshka(dc, sc(268), sc(62), sc(48), p);
    let f_title = font(14);
    let t = "旁译 · 设置";
    let (tw, _) = measure(dc, f_title, t);
    draw(dc, f_title, p.text, (w - tw) / 2, sc(8), t);
}

// ---------------- 共享几何 ----------------

struct Pill {
    act: i32,
    name: &'static str,
    rect: RECT,
}

struct Layout {
    cards: Vec<RECT>,
    pills: Vec<Pill>,
    close: RECT,
    footer_y: i32,
}

/// 唯一的几何真源：绘制、点击命中、悬停、台架导出全走这里，
/// 不会出现「画的和点的不是一个地方」（v4.5.0 教训）。
unsafe fn layout(dc: HDC) -> Layout {
    let f = font(12);
    let (_, texth) = measure(dc, f, "x");
    let mut cards = Vec::new();
    let mut pills = Vec::new();
    let mut y = sc(CARDS_TOP);
    for (_, opts) in rows() {
        let card = RECT {
            left: sc(MARGIN),
            top: y,
            right: sc(WIN_W - MARGIN),
            bottom: y + sc(CARD_H),
        };
        cards.push(card);
        let mut x = sc(96);
        for (name, act) in opts {
            let (w, _) = measure(dc, f, name);
            let bw = w + sc(PILL_PAD_X) * 2;
            let bh = texth + sc(PILL_PAD_Y) * 2;
            let top = card.top + (sc(CARD_H) - bh) / 2;
            pills.push(Pill {
                act,
                name,
                rect: RECT { left: x, top, right: x + bw, bottom: top + bh },
            });
            x += bw + sc(PILL_GAP);
        }
        y += sc(CARD_H) + sc(CARD_GAP);
    }
    let footer_y = sc(FOOTER_Y);
    let (lw, lh) = measure(dc, f, "关闭");
    let bw = lw + sc(PILL_PAD_X) * 2;
    let bh = lh + sc(PILL_PAD_Y) * 2;
    let bx = sc(WIN_W - MARGIN) - bw;
    Layout {
        cards,
        pills,
        close: RECT { left: bx, top: footer_y, right: bx + bw, bottom: footer_y + bh },
        footer_y,
    }
}

/// 命中测试：返回动作号（1..24）、-1=关闭钮、0=没点中
fn pill_at(l: &Layout, x: i32, y: i32) -> i32 {
    for pill in &l.pills {
        let r = pill.rect;
        if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
            return pill.act;
        }
    }
    let c = l.close;
    if x >= c.left && x < c.right && y >= c.top && y < c.bottom {
        return -1;
    }
    0
}

/// 药丸的命中矩形（悬停局部重画用；v4.16.0）。0（没点中）和 -1（关闭钮）
/// 没有悬停高亮态，返回 None。
fn pill_rect(l: &Layout, act: i32) -> Option<RECT> {
    l.pills.iter().find(|p| p.act == act).map(|p| p.rect)
}

// ---------------- 窗口过程 ----------------

unsafe extern "system" fn proc_settings(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => LRESULT(1), // 返回 TRUE 建窗才继续（文档要求；防兜底分支差异）
        WM_ERASEBKGND => LRESULT(1), // 全部自己画，防闪（cand.rs 同款）
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let wdc = BeginPaint(h, &mut ps);
            let p = palette(accent(), dark(crate::cand::is_dark()));
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            // 双缓冲（v4.16.0）：整页先画到内存 DC，再一次 BitBlt 上屏 ——
            // 鼠标滑过换悬停高亮时屏幕上永远只有完整的帧，不闪（「面板
            // 悬停闪屏」修复；cand.rs 同款）。
            let mem = CreateCompatibleDC(wdc);
            let bmp = CreateCompatibleBitmap(wdc, rc.right, rc.bottom);
            let old_bmp = SelectObject(mem, bmp);
            let dc = mem;
            let brush = CreateSolidBrush(p.canvas);
            FillRect(dc, &rc, brush);
            let _ = DeleteObject(brush);
            SetBkMode(dc, TRANSPARENT);

            let l = layout(dc);
            let f_text = font(12);
            draw_header(dc, rc.right, &p);

            // 每行一张圆角卡片：白底 + 1px 描边 + 左侧标签
            let mut row = 0usize;
            for (label, _) in rows() {
                let card = l.cards[row];
                fill_round(dc, &card, sc(CARD_RADIUS), p.card);
                frame_round(dc, &card, sc(CARD_RADIUS), p.border);
                let (_, th) = measure(dc, f_text, label);
                draw(dc, f_text, p.faint, sc(LABEL_X), card.top + (card.bottom - card.top - th) / 2, label);
                row += 1;
            }
            // 药丸：选中=青底深字，悬停=灰底，未选中只画字
            let hot = HOT.load(Ordering::Relaxed);
            for pill in &l.pills {
                let on = is_current(pill.act);
                if on {
                    fill_round(dc, &pill.rect, sc(PILL_RADIUS), p.pill_sel_fill);
                } else if hot == pill.act {
                    fill_round(dc, &pill.rect, sc(PILL_RADIUS), p.pill_hover);
                }
                draw(
                    dc, f_text,
                    if on { p.pill_sel_text } else { p.faint },
                    pill.rect.left + sc(PILL_PAD_X), pill.rect.top + sc(PILL_PAD_Y),
                    pill.name,
                );
            }
            // 底部：词库统计 + 关闭按钮
            let (dict_n, lm_n) = crate::dict::stats();
            let stat = format!("词库 {dict_n} 条 · 语言模型 {lm_n} 条搭配");
            draw(dc, f_text, p.faint, sc(LABEL_X), l.footer_y + sc(6), &stat);
            fill_round(dc, &l.close, sc(PILL_RADIUS), p.accent);
            draw(
                dc, f_text, p.accent_text,
                l.close.left + sc(PILL_PAD_X), l.close.top + sc(PILL_PAD_Y),
                "关闭",
            );
            // 内存帧一次上屏，然后释放临时 GDI 对象（成对创建/删除）
            let _ = BitBlt(wdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old_bmp);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let dc = GetDC(h);
            let l = layout(dc);
            let _ = ReleaseDC(h, dc);
            let x = (lp.0 & 0xFFFF) as i16 as i32;
            let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
            let hov = pill_at(&l, x, y);
            let old = HOT.swap(hov, Ordering::Relaxed);
            if hov != old {
                // v4.16.0 只重画「失去悬停的旧药丸 + 新悬停的药丸」两个
                // 小矩形，不整窗重画 —— 鼠标滑过时页面不再整页刷新
                // （和 WM_PAINT 双缓冲一起，悬停彻底不闪）
                for act in [old, hov] {
                    if let Some(r) = pill_rect(&l, act) {
                        let _ = InvalidateRect(h, Some(&r), FALSE);
                    }
                }
            }
            // 只挂一次 MouseLeave 追踪（TRACKED 防重复挂）
            if !TRACKED.swap(true, Ordering::Relaxed) {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: h,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
            }
            LRESULT(0)
        }
        // windows-rs 0.52 的 WM_MOUSELEAVE 是复合常量表达式，不能当 match 模式：
        // rustc 会把它当通配臂、把后面所有臂判成不可达直接从二进制里删掉（v4.6.0 大坑）。
        // 这里用字面量 0x02A3（= WM_MOUSELEAVE）。
        0x02A3 => {
            let old = HOT.swap(0, Ordering::Relaxed);
            TRACKED.store(false, Ordering::Relaxed);
            // 同样只重画刚失去悬停的那个药丸
            if old != 0 {
                let dc = GetDC(h);
                let l = layout(dc);
                let _ = ReleaseDC(h, dc);
                if let Some(r) = pill_rect(&l, old) {
                    let _ = InvalidateRect(h, Some(&r), FALSE);
                }
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            // 悬停在可点药丸上时给手型光标
            if ((lp.0 & 0xFFFF) as u16 as u32) == HTCLIENT && HOT.load(Ordering::Relaxed) != 0 {
                if let Ok(cur) = LoadCursorW(None, IDC_HAND) {
                    let _ = SetCursor(cur);
                }
                LRESULT(1)
            } else {
                DefWindowProcW(h, msg, wp, lp)
            }
        }
        WM_LBUTTONDOWN => {
            let dc = GetDC(h);
            let l = layout(dc);
            let _ = ReleaseDC(h, dc);
            let x = (lp.0 & 0xFFFF) as i16 as i32;
            let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
            let hit = pill_at(&l, x, y);
            // 点「关闭」药丸（pill_at 返回 -1）或点到底部整条空白（老行为）都关窗
            if hit == -1 || (hit == 0 && y >= l.footer_y && y <= l.footer_y + sc(30)) {
                let _ = SendMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0));
                return LRESULT(1);
            }
            if hit > 0 {
                apply(hit);
                let _ = InvalidateRect(h, None, TRUE);
                crate::cand::refresh_settings();
                sync_dwm(h); // 主题可能切了，标题栏深浅色跟着切
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
        13 => CNPUNCT.store(true, Ordering::Relaxed),
        14 => CNPUNCT.store(false, Ordering::Relaxed),
        15..=17 => TRMODE.store(action - 15, Ordering::Relaxed),
        18..=22 => ACCENT.store(action - 18, Ordering::Relaxed),
        23 => ENGLISH.store(true, Ordering::Relaxed),
        24 => ENGLISH.store(false, Ordering::Relaxed),
        25 => EMOJI.store(true, Ordering::Relaxed),
        26 => EMOJI.store(false, Ordering::Relaxed),
        12 => {
            // 打开自定义短语文件（phrases.txt）
            let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
            let file = format!("{base}\\Pangyi\\phrases.txt");
            let _ = crate::dict::phrase("dz"); // 顺便保证文件已生成
            let f = wide(&file);
            let v = wide("open");
            let n = wide("notepad.exe");
            unsafe {
                windows::Win32::UI::Shell::ShellExecuteW(
                    HWND(0), PCWSTR(n.as_ptr()), PCWSTR(f.as_ptr()),
                    PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL,
                );
                let _ = v;
            }
        }
        _ => {}
    }
    save();
}

unsafe fn init() {
    if WND.0 != 0 {
        return;
    }
    BG = CreateSolidBrush(palette(accent(), dark(crate::cand::is_dark())).canvas);
    let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
    let cls: Vec<u16> = "PangyiSettings\0".encode_utf16().collect();
    // 标题栏 / 任务栏 / Alt-Tab 用 exe 里编号 1 的图标资源（就是桌面上那个「中Я」）。
    // 独立设置程序 settings_app.exe 里没有这个资源，LoadImageW 会失败 → hIcon=0，退回系统默认图标。
    let big = LoadImageW(hinst, PCWSTR(1 as *const u16), IMAGE_ICON, 0, 0,
        LR_DEFAULTSIZE | LR_SHARED).map(|h| HICON(h.0)).unwrap_or_default();
    let (smx, smy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
    let small = LoadImageW(hinst, PCWSTR(1 as *const u16), IMAGE_ICON, smx, smy,
        LR_SHARED).map(|h| HICON(h.0)).unwrap_or_default();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(proc_settings),
        lpszClassName: PCWSTR(cls.as_ptr()),
        hInstance: hinst.into(),
        hbrBackground: BG,
        hIcon: big,
        ..Default::default()
    };
    RegisterClassW(&wc);
    let title: Vec<u16> = format!("旁译 设置  v{}\0", env!("CARGO_PKG_VERSION")).encode_utf16().collect();
    // 普通窗口：有标题栏、有关闭按钮，能正常关掉。
    // 尺寸给的是「客户区 360×616 对应的外层尺寸」——见 OUTER_W 注释。
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    let mut outer = RECT { left: 0, top: 0, right: sc(WIN_W), bottom: sc(WIN_H) };
    let _ = AdjustWindowRectEx(&mut outer, style, FALSE, WINDOW_EX_STYLE(0));
    OUTER_W = outer.right - outer.left;
    OUTER_H = outer.bottom - outer.top;
    WND = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(cls.as_ptr()), PCWSTR(title.as_ptr()),
        style,
        0, 0, OUTER_W, OUTER_H, None, None, hinst, None);
    // WNDCLASSW 没有 hIconSm 字段，标题栏那个 16x16 小图标用 WM_SETICON 单独设
    if WND.0 != 0 && small.0 != 0 {
        SendMessageW(WND, WM_SETICON, WPARAM(0), LPARAM(small.0 as isize));
    }
    sync_dwm(WND);
}

/// 深色主题时把系统标题栏也切成深色（DWM 属性 20），不然深色面板顶个白标题栏
unsafe fn sync_dwm(h: HWND) {
    if h.0 == 0 {
        return;
    }
    let v: BOOL = dark(crate::cand::is_dark()).into();
    let _ = DwmSetWindowAttribute(
        h,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        &v as *const BOOL as *const c_void,
        std::mem::size_of::<BOOL>() as u32,
    );
}
/// 独立设置程序用：开一个带标题栏的普通窗口（可以从右上角关掉）。
pub unsafe fn open_app() {
    STANDALONE.store(true, Ordering::Relaxed);
    init();
    if WND.0 == 0 {
        return;
    }
    let _ = DeleteObject(BG);
    BG = CreateSolidBrush(palette(accent(), dark(crate::cand::is_dark())).canvas);
    let (w, h) = (OUTER_W, OUTER_H);
    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);
    let _ = SetWindowPos(WND, HWND_TOP, (sw - w) / 2, (sh - h) / 2, w, h, SWP_SHOWWINDOW);
    let _ = InvalidateRect(WND, None, TRUE);
}

// ---- 测试台架专用导出（settings_e2e 用 GetProcAddress 拿，不挂在正常 API 上）----

/// 面板句柄
#[no_mangle]
pub unsafe extern "system" fn settings_hwnd_export() -> isize {
    WND.0
}

/// 开面板（先 load 配置），返回句柄；已开就返回现有句柄
#[no_mangle]
pub unsafe extern "system" fn settings_open_export() -> isize {
    load();
    open_app();
    WND.0
}

/// act=0 关闭钮 / 1..24 药丸 的可点击矩形写进 out[4] = l,t,r,b；找不到返回 0。
/// 矩形和绘制/命中走同一个 layout()，保证台架点的就是画的那块。
#[no_mangle]
pub unsafe extern "system" fn settings_action_rect_export(act: u32, out: *mut i32) -> u32 {
    if out.is_null() || WND.0 == 0 {
        return 0;
    }
    let dc = GetDC(WND);
    let l = layout(dc);
    let _ = ReleaseDC(WND, dc);
    let rect = if act == 0 {
        Some(l.close)
    } else {
        l.pills.iter().find(|p| p.act == act as i32).map(|p| p.rect)
    };
    match rect {
        Some(r) => {
            *out.add(0) = r.left;
            *out.add(1) = r.top;
            *out.add(2) = r.right;
            *out.add(3) = r.bottom;
            1
        }
        None => 0,
    }
}
