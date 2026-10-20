//! v4.4.0「自动翻译（逐字/整句）」端到端验证。
//!
//! 骨架和 punct_e2e.rs 一样：进程内预载新 DLL（CoRegisterClassObject 抢注），
//! 真实 SendInput 按键走 系统队列 -> TSF -> 旁译 -> RichEdit 完整链路。
//! settings.txt 由本台架自己写入重定向后的 LOCALAPPDATA（trmode=1 起步），
//! 翻译模式切换用 Ctrl+, 走真实快捷键（和用户手按同一条路径）。
//!
//! 用例：
//!   T1  逐字: ni+空格       -> 你 ты
//!   T2  逐字: nihao+空格    -> 你好 привет
//!   T3  逐字: nihao+。      -> 你好 привет。
//!   T4  整句: wo ai ni+。   -> 我爱你。 я любить ты.（v4.11.0 义项拆到
//!                            单词级：я; мне; мой -> я、ты, вы -> ты）
//!   T5  整句: nihao+回车    -> 你好 привет（回车收尾，不换行）
//!   T6  整句: nihao+。      -> 你好。 привет.
//!   T7  关:   nihao+，      -> 你好，（无翻译）
//!   T8  逐字: jintian+空格  -> 今天 сегодня
//!   T9  整句: ni+ESC+。     -> 。（ESC 取消不残留，空句不加翻译）
//!   T10 关:   ni+空格       -> 你（回归：关模式纯净上屏）
//!   T11 逐字: fan+空格      -> 反 противоположность（v4.11.0 拆单词：
//!                            反的义项0原来是 "противоположность; анти"）
//!
//! 用法：trans_e2e.exe [新DLL路径]
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
    r"C:\Users\33354\Documents\Codex\2026-10-08\github-yenharvey-qingjian\work\trans-e2e.txt";

static mut MAIN: HWND = HWND(0);
static mut EDIT: HWND = HWND(0);
static mut PHASE: u32 = 0;
static mut QI: usize = 0;
static mut QUEUE: Vec<u16> = Vec::new();
static mut WARMUP: bool = false;
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
            // 预热：链式启动器里前一套台架进程刚退出、TSF 还在清理，首拍按键
            // 会被吃掉（v4.13.0 实测 trans 排链里跑 T1 必空文档，单跑绿）。
            // 先打一发 ni+空格+nihao+空格 喂给清理期再清空，T1 才稳。
            logline("=== 预热: ni+空格+nihao+空格 -> 清空 ===");
            QUEUE = vec![0x4E, 0x49, 0x20, 0x4E, 0x49, 0x48, 0x41, 0x4F, 0x20];
            QI = 0;
            WARMUP = true;
            PHASE = 1;
        }
        1 => {
            if WARMUP {
                WARMUP = false;
                clear_edit();
                logline("=== T1 逐字: ni+空格 -> 你 ты ===");
                QUEUE = vec![0x4E, 0x49, 0x20];
                QI = 0;
                return;
            }
            let _ = result("T1", "「你 ты」", text_of() == "你 ты");
            clear_edit();
            logline("=== T2 逐字: nihao+空格 -> 你好 привет ===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x20];
            QI = 0;
            PHASE = 2;
        }
        2 => {
            let _ = result("T2", "「你好 привет」", text_of() == "你好 привет");
            clear_edit();
            logline("=== T3 逐字: nihao+。 -> 你好 привет。 ===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0xBE];
            QI = 0;
            PHASE = 3;
        }
        3 => {
            let _ = result("T3", "「你好 привет。」", text_of() == "你好 привет。");
            clear_edit();
            logline("=== Ctrl+, 切到整句模式 ===");
            QUEUE = vec![0x8011, 0xBC, 0x4011];
            QI = 0;
            PHASE = 4;
        }
        4 => {
            logline("=== T4 整句: wo ai ni+。 -> 我爱你。 я; мне; мой любить ты, вы. ===");
            QUEUE = vec![0x57, 0x4F, 0x20, 0x41, 0x49, 0x20, 0x4E, 0x49, 0x20, 0xBE];
            QI = 0;
            PHASE = 5;
        }
        5 => {
            let _ = result("T4", "「我爱你。 я любить ты.」", text_of() == "我爱你。 я любить ты.");
            clear_edit();
            logline("=== T5 整句: nihao+回车 -> 你好 привет（回车收尾不上屏换行）===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x20, 0x0D];
            QI = 0;
            PHASE = 6;
        }
        6 => {
            let _ = result("T5", "「你好 привет」", text_of() == "你好 привет");
            clear_edit();
            logline("=== T6 整句: nihao+。 -> 你好。 привет. ===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x20, 0xBE];
            QI = 0;
            PHASE = 7;
        }
        7 => {
            let _ = result("T6", "「你好。 привет.」", text_of() == "你好。 привет.");
            clear_edit();
            logline("=== Ctrl+, 切回关 ===");
            QUEUE = vec![0x8011, 0xBC, 0x4011];
            QI = 0;
            PHASE = 8;
        }
        8 => {
            logline("=== T7 关: nihao+， -> 你好，（无翻译）===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x20, 0xBC];
            QI = 0;
            PHASE = 9;
        }
        9 => {
            let _ = result("T7", "「你好，」", text_of() == "你好，");
            clear_edit();
            logline("=== Ctrl+, 切到逐字 ===");
            QUEUE = vec![0x8011, 0xBC, 0x4011];
            QI = 0;
            PHASE = 10;
        }
        10 => {
            logline("=== T8 逐字: jintian+空格 -> 今天 сегодня ===");
            QUEUE = vec![0x4A, 0x49, 0x4E, 0x54, 0x49, 0x41, 0x4E, 0x20];
            QI = 0;
            PHASE = 11;
        }
        11 => {
            let _ = result("T8", "「今天 сегодня」", text_of() == "今天 сегодня");
            clear_edit();
            logline("=== Ctrl+, 切到整句 ===");
            QUEUE = vec![0x8011, 0xBC, 0x4011];
            QI = 0;
            PHASE = 12;
        }
        12 => {
            logline("=== T9 整句: ni+ESC+。 -> 。（ESC 取消，空句不加翻译）===");
            QUEUE = vec![0x4E, 0x49, 0x1B, 0xBE];
            QI = 0;
            PHASE = 13;
        }
        13 => {
            let _ = result("T9", "「。」", text_of() == "。");
            clear_edit();
            logline("=== Ctrl+, 切回关 ===");
            QUEUE = vec![0x8011, 0xBC, 0x4011];
            QI = 0;
            PHASE = 14;
        }
        14 => {
            logline("=== T10 关: ni+空格 -> 你（回归：关模式纯净上屏）===");
            QUEUE = vec![0x4E, 0x49, 0x20];
            QI = 0;
            PHASE = 15;
        }
        15 => {
            let _ = result("T10", "「你」", text_of() == "你");
            clear_edit();
            logline("=== Ctrl+, 切到逐字 ===");
            QUEUE = vec![0x8011, 0xBC, 0x4011];
            QI = 0;
            PHASE = 16;
        }
        16 => {
            logline("=== T11 逐字拆单词: fan+空格 -> 反 противоположность ===");
            QUEUE = vec![0x46, 0x41, 0x4E, 0x20];
            QI = 0;
            PHASE = 17;
        }
        17 => {
            let _ = result("T11", "「反 противоположность」", text_of() == "反 противоположность");
            logline("=== 进程内 pangyi 模块（应只有 target\\release 的新 DLL）===");
            log_modules();
            logline("=== 完 ===");
            flush();
            let _ = DestroyWindow(h);
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
        // 写测试配置：逐字模式起步（trmode=1），中文标点开。
        // 必须在激活之前写好 —— settings::load() 在 Activate 时读一次。
        let base = std::env::var("LOCALAPPDATA").unwrap_or_default();
        let dir = format!("{base}\\Pangyi");
        let _ = std::fs::create_dir_all(&dir);
        let cfg = "max=9\nscale=100\nlayout=v\ntheme=0\nenglish=0\ndebug=0\ncnpunct=1\ntrmode=1\n";
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
        let cls = wide("PangyiTransE2E");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(6isize),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = wide("Pangyi Trans E2E");
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
