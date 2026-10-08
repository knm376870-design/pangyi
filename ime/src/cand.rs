//! 候选窗（系统输入法版与独立版共用）
//!
//! 视觉参考青简：圆角、跟随系统深浅色、每行 = 序号 | 中文 | 拼音 | 俄语译词
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
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());
static SEL: AtomicUsize = AtomicUsize::new(0);
static DARK: AtomicBool = AtomicBool::new(true);
static mut WND: HWND = HWND(0);

const ROW_H: i32 = 36;
const PAD: i32 = 10;
const IDX_W: i32 = 26;
const WORD_W: i32 = 132;
const W: i32 = 560;

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().collect() }

fn theme() -> (COLORREF, COLORREF, COLORREF, COLORREF, COLORREF, COLORREF) {
    // (背景, 边框, 主文字, 次要文字, 译词, 选中行背景)
    if DARK.load(Ordering::Relaxed) {
        (COLORREF(0x00261f1b), COLORREF(0x00473a33), COLORREF(0x00f4efec),
         COLORREF(0x009a8b82), COLORREF(0x00c7a4ff), COLORREF(0x00493a2e))
    } else {
        (COLORREF(0x00fdfbf9), COLORREF(0x00e7ddd6), COLORREF(0x00201a17),
         COLORREF(0x0085786f), COLORREF(0x007736c2), COLORREF(0x00f3e8de))
    }
}

pub unsafe fn detect_theme() {
    use windows::Win32::System::Registry::*;
    let mut h = HKEY::default();
    let sub = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0");
    if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), 0, KEY_READ, &mut h).is_ok() {
        let name = wide("AppsUseLightTheme\0");
        let mut v: u32 = 0;
        let mut cb = 4u32;
        let mut ty = REG_DWORD;
        if RegQueryValueExW(h, PCWSTR(name.as_ptr()), None, Some(&mut ty),
                            Some(&mut v as *mut u32 as *mut u8), Some(&mut cb)).is_ok() {
            DARK.store(v == 0, Ordering::Relaxed);
        }
        let _ = RegCloseKey(h);
    }
}

unsafe extern "system" fn proc_cand(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let (bg, bd, fg, dim, acc, selbg) = theme();
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let b = CreateSolidBrush(bg);
            FillRect(dc, &rc, b);
            let _ = DeleteObject(b);

            SetBkMode(dc, TRANSPARENT);
            let f_cjk = CreateFontW(-19, 0, 0, 0, 600, 0, 0, 0, 134, 0, 0, 0, 0, windows::core::w!("Microsoft YaHei UI"));
            let f_sm  = CreateFontW(-12, 0, 0, 0, 400, 0, 0, 0, 134, 0, 0, 0, 0, windows::core::w!("Segoe UI"));
            let f_ru  = CreateFontW(-14, 0, 0, 0, 400, 0, 0, 0, 134, 0, 0, 0, 0, windows::core::w!("Segoe UI"));

            let items = ITEMS.lock().unwrap();
            let sel = SEL.load(Ordering::Relaxed);
            for (i, it) in items.iter().take(9).enumerate() {
                let y = PAD + (i as i32) * ROW_H;
                let row = RECT { left: 4, top: y - 3, right: rc.right - 4, bottom: y + ROW_H - 3 };
                if i == sel {
                    let sb = CreateSolidBrush(selbg);
                    let rgn = CreateRoundRectRgn(row.left, row.top, row.right, row.bottom, 8, 8);
                    FillRgn(dc, rgn, sb);
                    let _ = DeleteObject(rgn);
                    let _ = DeleteObject(sb);
                }
                // 序号
                let old = SelectObject(dc, f_sm);
                SetTextColor(dc, if i == sel { acc } else { dim });
                let n = wide(&format!("{}", i + 1));
                TextOutW(dc, PAD + 4, y + 8, &n);
                // 中文
                let _ = SelectObject(dc, f_cjk);
                SetTextColor(dc, fg);
                let word = wide(&it.word);
                TextOutW(dc, PAD + IDX_W, y + 3, &word);
                // 拼音
                let _ = SelectObject(dc, f_sm);
                SetTextColor(dc, dim);
                let py = wide(&it.pinyin);
                TextOutW(dc, PAD + IDX_W + WORD_W, y + 10, &py);
                // 俄语
                let _ = SelectObject(dc, f_ru);
                SetTextColor(dc, acc);
                let ru = wide(&it.ru.replace('|', "·"));
                let cut: String = ru.iter().take(46).map(|_| '\0').collect(); let _ = cut;
                TextOutW(dc, PAD + IDX_W + WORD_W + 116, y + 7, &ru[..ru.len().min(60)]);

                if i == sel { let _ = SelectObject(dc, old); }
            }
            let _ = SelectObject(dc, f_cjk);
            let _ = DeleteObject(f_cjk);
            let _ = DeleteObject(f_sm);
            let _ = DeleteObject(f_ru);

            // 外边框
            let frame = CreateSolidBrush(bd);
            let _ = FrameRect(dc, &rc, frame);
            let _ = DeleteObject(frame);

            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

pub unsafe fn init() {
    if WND.0 != 0 { return; }
    detect_theme();
    let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
    let cls: Vec<u16> = "PangyiCandModern\0".encode_utf16().collect();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(proc_cand),
        lpszClassName: PCWSTR(cls.as_ptr()),
        hInstance: hinst.into(),
        hbrBackground: HBRUSH(6isize),
        ..Default::default()
    };
    RegisterClassW(&wc);
    let title: Vec<u16> = "旁译候选\0".encode_utf16().collect();
    WND = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
        PCWSTR(cls.as_ptr()), PCWSTR(title.as_ptr()),
        WS_POPUP, 0, 0, W, 60, None, None, hinst, None);
}

pub unsafe fn set_items(items: Vec<Item>) {
    {
        let mut g = ITEMS.lock().unwrap();
        *g = items;
    }
    SEL.store(0, Ordering::Relaxed);
}

pub unsafe fn select(i: usize) {
    let n = ITEMS.lock().unwrap().len();
    if n > 0 { SEL.store(i.min(n - 1), Ordering::Relaxed); if WND.0 != 0 { let _ = InvalidateRect(WND, None, TRUE); } }
}

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

pub unsafe fn style_selected() { if WND.0 != 0 { let r = CreateRoundRectRgn(0, 0, W, 400, 12, 12); let _ = SetWindowRgn(WND, r, TRUE); } }

pub unsafe fn show_at(x: i32, y: i32) {
    if WND.0 == 0 { return; }
    let n = ITEMS.lock().unwrap().len().min(9);
    if n == 0 { hide(); return; }
    let h = PAD * 2 + (n as i32) * ROW_H - 6;
    style_selected();
    let _ = SetWindowPos(WND, HWND_TOPMOST, x, y, W, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    let _ = InvalidateRect(WND, None, TRUE);
}

pub unsafe fn hide() { if WND.0 != 0 { let _ = ShowWindow(WND, SW_HIDE); } }

pub unsafe fn hwnd() -> HWND { WND }
