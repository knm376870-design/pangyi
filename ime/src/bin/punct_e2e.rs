//! v4.3.0「中文标点（智能全角）」端到端验证。
//!
//! 骨架和 fix_e2e.rs 一样：进程内预载新 DLL（CoRegisterClassObject 抢注），
//! 真实 SendInput 按键走 系统队列 -> TSF -> 旁译 -> RichEdit 完整链路，
//! LOCALAPPDATA 由调用方重定向，不碰用户配置。
//!
//! 用例（settings.txt 需 cnpunct=1）：
//!   T1  ','  -> ，
//!   T2  '.'  -> 。
//!   T3  '/'  -> 、
//!   T4  '[' ']' -> 【】
//!   T5  ''' ''' -> ‘’（引号配对）
//!   T6  'ni'+',' -> 你，
//!   T7  Shift+',' -> <（半角放行，方便打代码）
//!   T8  'ni'+'.' -> 你。
//!   T9  'ni'+''' -> 你‘
//!   T10 Shift 切英文 -> ',' 半角；切回中文 -> ',' 全角
//!   T11 Ctrl+. 关 -> ',' 半角；Ctrl+. 开 -> ',' 全角
//!   T12 'ni'+'-' 仍翻页不提交 -> ESC -> 空（回归：- 还是翻页键）
//!
//! 用法：punct_e2e.exe [新DLL路径]
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
    r"C:\Users\33354\Documents\Codex\2026-10-08\github-yenharvey-qingjian\work\punct-e2e.txt";

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
            logline("=== T1: 无拼音按逗号 → ， ===");
            QUEUE = vec![0xBC];
            QI = 0;
            PHASE = 1;
        }
        1 => {
            let _ = result("T1", "「，」", text_of() == "，");
            clear_edit();
            logline("=== T2: 无拼音按句点 → 。 ===");
            QUEUE = vec![0xBE];
            QI = 0;
            PHASE = 2;
        }
        2 => {
            let _ = result("T2", "「。」", text_of() == "。");
            clear_edit();
            logline("=== T3: 无拼音按 / → 、 ===");
            QUEUE = vec![0xBF];
            QI = 0;
            PHASE = 3;
        }
        3 => {
            let _ = result("T3", "「、」", text_of() == "、");
            clear_edit();
            logline("=== T4: 无拼音按 [ ] → 【】 ===");
            QUEUE = vec![0xDB, 0xDD];
            QI = 0;
            PHASE = 4;
        }
        4 => {
            let _ = result("T4", "「【】」", text_of() == "【】");
            clear_edit();
            logline("=== T5: 无拼音按 ' ' → ‘ ’（引号配对）===");
            QUEUE = vec![0xDE, 0xDE];
            QI = 0;
            PHASE = 5;
        }
        5 => {
            let _ = result("T5", "「‘’」", text_of() == "‘’");
            clear_edit();
            logline("=== T6: 有拼音 ni+逗号 → 你， ===");
            QUEUE = vec![0x4E, 0x49, 0xBC];
            QI = 0;
            PHASE = 6;
        }
        6 => {
            let _ = result("T6", "「你，」", text_of() == "你，");
            clear_edit();
            logline("=== T7: Shift+逗号 → <（半角放行，不转全角）===");
            QUEUE = vec![0x8010, 0xBC, 0x4010];
            QI = 0;
            PHASE = 7;
        }
        7 => {
            let _ = result("T7", "「<」", text_of() == "<");
            clear_edit();
            logline("=== T8: 有拼音 ni+句点 → 你。 ===");
            QUEUE = vec![0x4E, 0x49, 0xBE];
            QI = 0;
            PHASE = 8;
        }
        8 => {
            let _ = result("T8", "「你。」", text_of() == "你。");
            clear_edit();
            logline("=== T9: 有拼音 ni+' → 你‘ ===");
            QUEUE = vec![0x4E, 0x49, 0xDE];
            QI = 0;
            PHASE = 9;
        }
        9 => {
            let _ = result("T9", "「你‘」", text_of() == "你‘");
            clear_edit();
            logline("=== T10a: Shift 单独按一下 → 英文模式，逗号 → , 半角 ===");
            QUEUE = vec![0x10];
            QI = 0;
            PHASE = 10;
        }
        10 => {
            QUEUE = vec![0xBC];
            QI = 0;
            PHASE = 11;
        }
        11 => {
            let _ = result("T10a", "「,」（英文模式标点不转）", text_of() == ",");
            clear_edit();
            logline("=== T10b: 再按 Shift 回中文，逗号 → ， ===");
            QUEUE = vec![0x10];
            QI = 0;
            PHASE = 12;
        }
        12 => {
            QUEUE = vec![0xBC];
            QI = 0;
            PHASE = 13;
        }
        13 => {
            let _ = result("T10b", "「，」（切回中文恢复全角）", text_of() == "，");
            clear_edit();
            logline("=== T11a: Ctrl+. 关掉中文标点，逗号 → , 半角 ===");
            QUEUE = vec![0x8011, 0xBE, 0x4011];
            QI = 0;
            PHASE = 14;
        }
        14 => {
            QUEUE = vec![0xBC];
            QI = 0;
            PHASE = 15;
        }
        15 => {
            let _ = result("T11a", "「,」（Ctrl+. 已关中文标点）", text_of() == ",");
            clear_edit();
            logline("=== T11b: Ctrl+. 再开，逗号 → ， ===");
            QUEUE = vec![0x8011, 0xBE, 0x4011];
            QI = 0;
            PHASE = 16;
        }
        16 => {
            QUEUE = vec![0xBC];
            QI = 0;
            PHASE = 17;
        }
        17 => {
            let _ = result("T11b", "「，」（Ctrl+. 已开回中文标点）", text_of() == "，");
            clear_edit();
            logline("=== T12: ni + '-' 仍翻页不提交 → ESC → 空（回归）===");
            QUEUE = vec![0x4E, 0x49, 0xBD, 0x1B];
            QI = 0;
            PHASE = 18;
        }
        18 => {
            let _ = result("T12", "空（- 是翻页键，ESC 取消无残留）", text_of().is_empty());
            logline("=== 进程内 pangyi 模块（应只有 target\\release 的新 DLL）===");
            log_modules();
            logline("=== 完 ===");
            flush();
            DestroyWindow(h);
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
        // 必须在创建任何 TSF 感知控件（RichEdit）之前抢注类工厂：
        // RichEdit 创建会触发 TSF 初始化并按注册表加载已安装的旧 DLL，
        // 晚了就抢不到了（v4.3.0 实测：注册表指向 Program Files 旧版时，
        // 第一拍再 preload 会眼睁睁看着旧 DLL 接管按键）。
        let dll = std::env::args().nth(1).unwrap_or_default();
        logline(&format!("预载 DLL: {dll}"));
        preload(&dll);
        let md = wide("Msftedit.dll");
        let _ = LoadLibraryW(PCWSTR(md.as_ptr()));
        let cls = wide("PangyiPunctE2E");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(6isize),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = wide("Pangyi Punct E2E");
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
