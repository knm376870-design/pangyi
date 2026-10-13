//! v4.5.0「候选窗两侧选择（←→ / 鼠标点选）」端到端验证。
//!
//! 骨架和 trans_e2e.rs 一样：进程内预载新 DLL（CoRegisterClassObject 抢注），
//! 真实 SendInput 按键走 系统队列 -> TSF -> 旁译 -> RichEdit 完整链路。
//! settings.txt 由本台架自己写入重定向后的 LOCALAPPDATA（trmode=0 关翻译，
//! 只测两侧选择本身）。
//!
//! 用例：
//!   T1  ni+空格          -> 你（基线：默认中文侧）
//!   T2  ni+→+空格        -> ты, вы（右箭头选中俄语侧，中文不输入）
//!   T3  ni+→+←+空格      -> 你（左箭头选回中文侧）
//!   T4  nihao+→+1        -> привет（数字键也按选中的一侧上屏）
//!   T5  nihao+→+。       -> привет。（标点跟俄语走）
//!   T6  wo+空格          -> 我（新一组候选自动回到中文侧）
//!   T7  ni+[鼠标点0行中文] -> 你（点中文词上屏）
//!   T8  ni+[鼠标点0行俄语] -> ты, вы（点俄语上屏）
//!   T9  ni+[鼠标点1行中文] -> 泥（点别的行也准）
//!   T10 ←→(无拼音)+ni+空格 -> 你（无拼音时方向键直通给应用）
//!
//! 鼠标点击用 PostMessage 直接投 WM_LBUTTONDOWN 到候选窗（同进程，走和
//! 真鼠标完全一样的窗口过程），点击坐标用 DLL 的 cand_hit_rect_export 算。
//!
//! 用法：side_e2e.exe [新DLL路径]
#![allow(non_snake_case)]
use windows::core::{s, GUID, HRESULT, Interface, IUnknown, PCWSTR, Result};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::ProcessStatus::{EnumProcessModules, GetModuleFileNameExW};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const OUR_CLSID: GUID = GUID::from_u128(0xB4E8F2A1_7C3D_4E5F_9A21_8D6C4B3E1F07);
const OUR_PROFILE: GUID = GUID::from_u128(0x5A7C9E12_3D4B_4F8A_B6E1_2C9D7A5E8F34);
const LANG: u16 = 0x0804;
const OUT: &str =
    r"C:\Users\33354\Documents\Codex\2026-10-08\github-yenharvey-qingjian\work\side-e2e.txt";

static mut MAIN: HWND = HWND(0);
static mut EDIT: HWND = HWND(0);
static mut PHASE: u32 = 0;
static mut QI: usize = 0;
static mut QUEUE: Vec<u16> = Vec::new();
static mut LOG: String = String::new();
static mut PROFILES: Option<ITfInputProcessorProfiles> = None;
static mut KEEP_DLL: Option<HMODULE> = None;
static mut KEEP_UNK: Option<IUnknown> = None;
static mut COOKIE: u32 = 0;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
fn logline(s: &str) {
    unsafe { LOG.push_str(s); LOG.push('\n'); }
}
unsafe fn flush() {
    std::fs::write(OUT, LOG.as_bytes()).ok();
}

unsafe fn text_of() -> String {
    let mut buf = [0u16; 1024];
    let n = SendMessageW(EDIT, WM_GETTEXT, WPARAM(buf.len()), LPARAM(buf.as_mut_ptr() as isize)).0
        as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}
unsafe fn clear_edit() {
    let e = wide("");
    SendMessageW(EDIT, WM_SETTEXT, WPARAM(0), LPARAM(e.as_ptr() as isize));
}

/// 抢前台（复制自 ime_e2e：先抬到顶层再设焦点，多试几次）
unsafe fn focus_ours() {
    for _ in 0..8 {
        if GetForegroundWindow() == MAIN {
            break;
        }
        let _ = SetWindowPos(
            MAIN,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        let _ = SetForegroundWindow(MAIN);
        let _ = BringWindowToTop(MAIN);
        SetFocus(EDIT);
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    let _ = SetWindowPos(
        MAIN,
        HWND_NOTOPMOST,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE,
    );
    SetFocus(EDIT);
}

/// 队列编码：0x8000|vk = 只按下；0x4000|vk = 只抬起；否则一按一松。
unsafe fn send_code(code: u16) {
    let down_only = code & 0x8000 != 0;
    let up_only = code & 0x4000 != 0;
    let vk = if down_only { code & 0x3FFF } else { code & 0x0FFF };
    let mut ins: Vec<INPUT> = Vec::new();
    if !up_only {
        let mut i = INPUT::default();
        i.r#type = INPUT_KEYBOARD;
        i.Anonymous.ki.wVk = VIRTUAL_KEY(vk);
        ins.push(i);
    }
    if !down_only {
        let mut i = INPUT::default();
        i.r#type = INPUT_KEYBOARD;
        i.Anonymous.ki.wVk = VIRTUAL_KEY(vk);
        i.Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
        ins.push(i);
    }
    let _ = SendInput(&ins, std::mem::size_of::<INPUT>() as i32);
}

/// 预载新 DLL 并抢注类工厂（见 fix_e2e.rs 的同名函数注释）
unsafe fn preload(path: &str) -> bool {
    let w = wide(path);
    let h = match LoadLibraryW(PCWSTR(w.as_ptr())) {
        Ok(h) => h,
        Err(e) => {
            logline(&format!("预载新 DLL 失败: {e:?}"));
            return false;
        }
    };
    let Some(f) = GetProcAddress(h, s!("DllGetClassObject")) else {
        logline("新 DLL 没有导出 DllGetClassObject");
        return false;
    };
    type GetFn =
        unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut core::ffi::c_void) -> HRESULT;
    let get: GetFn = std::mem::transmute(f);
    let iid: GUID = GUID::from_u128(0x00000001_0000_0000_C000_000000000046); // IClassFactory
    let mut pv: *mut core::ffi::c_void = std::ptr::null_mut();
    let hr = get(&OUR_CLSID, &iid, &mut pv);
    if hr.is_err() || pv.is_null() {
        logline(&format!("DllGetClassObject 失败: {hr:?}"));
        return false;
    }
    let unk = IUnknown::from_raw(pv);
    match CoRegisterClassObject(&OUR_CLSID, &unk, CLSCTX_INPROC_SERVER, REGCLS_MULTIPLEUSE) {
        Ok(c) => {
            COOKIE = c;
            KEEP_DLL = Some(h);
            KEEP_UNK = Some(unk);
            logline("新 DLL 已注册进本进程（CoRegisterClassObject）");
            true
        }
        Err(e) => {
            logline(&format!("CoRegisterClassObject 失败: {e:?}"));
            false
        }
    }
}

/// 列出进程内所有 pangyi 相关模块 —— 确认 TSF 用的确实是预载的新 DLL
unsafe fn log_modules() {
    let me = GetCurrentProcess();
    let mut mods = [HMODULE(0); 512];
    let mut need = 0u32;
    if EnumProcessModules(
        me,
        mods.as_mut_ptr(),
        (mods.len() * std::mem::size_of::<HMODULE>()) as u32,
        &mut need,
    )
    .is_ok()
    {
        let n = (need as usize / std::mem::size_of::<HMODULE>()).min(mods.len());
        for m in mods.iter().take(n) {
            let mut buf = [0u16; 512];
            let len = GetModuleFileNameExW(me, *m, &mut buf);
            if len > 0 {
                let p = String::from_utf16_lossy(&buf[..len as usize]);
                if p.to_lowercase().contains("pangyi") {
                    logline(&format!("  模块: {p}"));
                }
            }
        }
    }
}

unsafe fn activate() {
    match PROFILES.as_ref() {
        Some(p) => match p.ActivateLanguageProfile(&OUR_CLSID, LANG, &OUR_PROFILE) {
            Ok(()) => logline("激活旁译: ok"),
            Err(e) => logline(&format!("激活旁译: 失败 {e:?}")),
        },
        None => logline("没有 ITfInputProcessorProfiles"),
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
    focus_ours();
}

unsafe fn result(tag: &str, expect: &str, ok: bool) -> String {
    let t = text_of();
    logline(&format!(
        "{tag}: 文档=[{t}]  期望{expect}  {} ",
        if ok { "✓" } else { "✗ 不符合!" }
    ));
    flush();
    t
}

/// 往候选窗投一个真鼠标点击：第 row 行、中文侧(ru=false)/俄语侧(ru=true)。
/// 坐标由 DLL 导出的 cand_hit_rect_export 按窗口自己的排版算，不会点歪。
unsafe fn click_cand(row: u32, ru: bool) -> bool {
    let Some(dll) = KEEP_DLL else {
        logline("没有 DLL 句柄，无法点击");
        return false;
    };
    type HwndFn = unsafe extern "system" fn() -> isize;
    type RectFn = unsafe extern "system" fn(u32, u32, *mut i32) -> u32;
    let Some(fh) = GetProcAddress(dll, s!("cand_hwnd_export")) else {
        logline("DLL 没有导出 cand_hwnd_export");
        return false;
    };
    let Some(fr) = GetProcAddress(dll, s!("cand_hit_rect_export")) else {
        logline("DLL 没有导出 cand_hit_rect_export");
        return false;
    };
    let get_hwnd: HwndFn = std::mem::transmute(fh);
    let get_rect: RectFn = std::mem::transmute(fr);
    let h = HWND(get_hwnd());
    if h.0 == 0 {
        logline("候选窗句柄是 0（还没建窗？）");
        return false;
    }
    let mut rect = [0i32; 4];
    if get_rect(row, if ru { 1 } else { 0 }, rect.as_mut_ptr()) == 0 {
        logline(&format!("cand_hit_rect_export(row={row}, ru={ru}) 失败"));
        return false;
    }
    let (l, t, r, b) = (rect[0], rect[1], rect[2], rect[3]);
    let x = (l + r) / 2;
    let y = (t + b) / 2;
    let lp = ((y as isize) << 16) | (x as isize & 0xFFFF);
    logline(&format!("点击 候选窗 row={row} {} 侧 @({x},{y})", if ru { "俄语" } else { "中文" }));
    // wParam = MK_LBUTTON (0x0001)
    let _ = PostMessageW(h, WM_LBUTTONDOWN, WPARAM(1), LPARAM(lp));
    true
}

unsafe fn tick(h: HWND) {
    if QI < QUEUE.len() {
        if QI == 0 {
            focus_ours();
        }
        send_code(QUEUE[QI]);
        QI += 1;
        return;
    }
    match PHASE {
        0 => {
            // preload 已经挪到 main 开头（建 RichEdit 之前），这里只做 TSF 激活
            let base = std::env::var("LOCALAPPDATA").unwrap_or_default();
            let cfg = std::fs::read_to_string(format!("{base}\\Pangyi\\settings.txt"))
                .unwrap_or_else(|_| "(没有 settings.txt)".into());
            logline(&format!("测试用 settings.txt: {}", cfg.replace('\n', " ")));
            let _ = CoCreateInstance::<_, ITfThreadMgr>(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)
                .map(|tm| {
                    let _ = tm.Activate();
                });
            match CoCreateInstance::<_, ITfInputProcessorProfiles>(
                &CLSID_TF_InputProcessorProfiles,
                None,
                CLSCTX_INPROC_SERVER,
            ) {
                Ok(p) => PROFILES = Some(p),
                Err(e) => logline(&format!("拿不到 profiles: {e:?}")),
            }
            activate();
            logline("=== T1 基线: ni+空格 -> 你 ===");
            QUEUE = vec![0x4E, 0x49, 0x20];
            QI = 0;
            PHASE = 1;
        }
        1 => {
            let _ = result("T1", "「你」", text_of() == "你");
            clear_edit();
            logline("=== T2 右箭头选俄语侧: ni+→+空格 -> ты, вы ===");
            QUEUE = vec![0x4E, 0x49, 0x27, 0x20];
            QI = 0;
            PHASE = 2;
        }
        2 => {
            let _ = result("T2", "「ты, вы」", text_of() == "ты, вы");
            clear_edit();
            logline("=== T3 左箭头选回中文侧: ni+→+←+空格 -> 你 ===");
            QUEUE = vec![0x4E, 0x49, 0x27, 0x25, 0x20];
            QI = 0;
            PHASE = 3;
        }
        3 => {
            let _ = result("T3", "「你」", text_of() == "你");
            clear_edit();
            logline("=== T4 数字键也按选中的一侧: nihao+→+1 -> привет ===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x27, 0x31];
            QI = 0;
            PHASE = 4;
        }
        4 => {
            let _ = result("T4", "「привет」", text_of() == "привет");
            clear_edit();
            logline("=== T5 标点跟俄语走: nihao+→+。 -> привет。 ===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x27, 0xBE];
            QI = 0;
            PHASE = 5;
        }
        5 => {
            let _ = result("T5", "「привет。」", text_of() == "привет。");
            clear_edit();
            logline("=== T6 新候选自动回中文侧: wo+空格 -> 我 ===");
            QUEUE = vec![0x57, 0x4F, 0x20];
            QI = 0;
            PHASE = 6;
        }
        6 => {
            let _ = result("T6", "「我」", text_of() == "我");
            clear_edit();
            logline("=== T7 鼠标点中文词: ni + 点0行中文侧 -> 你 ===");
            QUEUE = vec![0x4E, 0x49];
            QI = 0;
            PHASE = 7;
        }
        7 => {
            click_cand(0, false);
            PHASE = 8;
        }
        8 => {
            let _ = result("T7", "「你」", text_of() == "你");
            clear_edit();
            logline("=== T8 鼠标点俄语: ni + 点0行俄语侧 -> ты, вы ===");
            QUEUE = vec![0x4E, 0x49];
            QI = 0;
            PHASE = 9;
        }
        9 => {
            click_cand(0, true);
            PHASE = 10;
        }
        10 => {
            let _ = result("T8", "「ты, вы」", text_of() == "ты, вы");
            clear_edit();
            logline("=== T9 鼠标点别的行: ni + 点1行中文侧 -> 泥 ===");
            QUEUE = vec![0x4E, 0x49];
            QI = 0;
            PHASE = 11;
        }
        11 => {
            click_cand(1, false);
            PHASE = 12;
        }
        12 => {
            let _ = result("T9", "「泥」", text_of() == "泥");
            clear_edit();
            logline("=== T10 无拼音时方向键直通: ← → ni+空格 -> 你 ===");
            QUEUE = vec![0x25, 0x27, 0x4E, 0x49, 0x20];
            QI = 0;
            PHASE = 13;
        }
        13 => {
            let _ = result("T10", "「你」", text_of() == "你");
            logline("=== 进程内 pangyi 模块（应只有 target\\release 的新 DLL）===");
            log_modules();
            logline("=== 完 ===");
            flush();
            let _ = DestroyWindow(h);
            PHASE = 99;
        }
        _ => {}
    }
}

extern "system" fn wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER => {
                tick(h);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, w, l),
        }
    }
}

fn main() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // 写测试配置：翻译关（只测两侧选择）、中文标点开、竖排。
        // 必须在激活之前写好 —— settings::load() 在 Activate 时读一次。
        let base = std::env::var("LOCALAPPDATA").unwrap_or_default();
        let dir = format!("{base}\\Pangyi");
        let _ = std::fs::create_dir_all(&dir);
        let cfg = "max=9\nscale=100\nlayout=v\ntheme=0\nenglish=0\ndebug=1\ncnpunct=1\ntrmode=0\n";
        let _ = std::fs::write(format!("{dir}\\settings.txt"), cfg);
        // 必须在创建任何 TSF 感知控件（RichEdit）之前抢注类工厂：
        // RichEdit 创建会触发 TSF 初始化并按注册表加载已安装的旧 DLL，
        // 晚了就抢不到了（v4.3.0 实测：注册表指向 Program Files 旧版时，
        // 第一拍再 preload 会眼睁睁看着旧 DLL 接管按键）。
        let dll = std::env::args().nth(1).unwrap_or_default();
        logline(&format!("预载 DLL: {dll}"));
        preload(&dll);
        let md = wide("Msftedit.dll");
        let _ = LoadLibraryW(PCWSTR(md.as_ptr()));
        let cls = wide("PangyiSideE2E");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(6isize),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = wide("Pangyi Side E2E");
        // 屏外 + 工具窗（fix_e2e 同款）：用户看不见、点不到，避免焦点被抢后键乱飞
        let h = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 as u32),
            PCWSTR(cls.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            -32000,
            -32000,
            760,
            420,
            None,
            None,
            None,
            None,
        );
        if h.0 == 0 {
            logline("建窗失败");
            flush();
            return Ok(());
        }
        MAIN = h;
        let ec = wide("RICHEDIT50W");
        let empty = wide("");
        EDIT = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(ec.as_ptr()),
            PCWSTR(empty.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL) as u32),
            0,
            0,
            720,
            360,
            h,
            None,
            None,
            None,
        );
        SetFocus(EDIT);
        SetTimer(h, 1, 260, None);
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    Ok(())
}
