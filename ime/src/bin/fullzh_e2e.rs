//! 「全量中文」的端到端验证（2026-10-10，v4.1.0）。
//!
//! 和 fix_e2e.rs 同一个骨架（SendInput -> 系统队列 -> TSF -> 旁译 -> RichEdit
//! 全链路、LoadLibrary + CoRegisterClassObject 免安装用新 DLL、LOCALAPPDATA
//! 重定向到临时目录），剧本换成验证这次词典改动本身：
//!
//!   H1  yuebuqun + 1  → 岳不群（唯一候选，旧词典里整个词不存在，打不出来）
//!   H2  mianyang + 2  → 沔阳  （第 2 候选，无俄语释义，被排在有译文的绵羊之后）
//!   H3  yiliang + 1  → 一辆  （16 个候选里的第 1 个，无俄语释义）
//!   H4  henduo   + 1  → 很多  （有俄语释义的老词，回归：译文层不受影响）
//!   H5  womenmingtianqubeijing + 空格 → 我们明天去北京（整句，回归）
//!
//! 这 4 个词条在旧词典（只有俄语译文词条）里完全无法输入，现在打进去能进
//! 文档，同时 H4/H5 证明原有排序和句子模式没被稀释。
//!
//! 用法：fullzh_e2e.exe [新DLL路径]  （LOCALAPPDATA 由调用方重定向）
//!      FULLZH_INSTALLED=1 fullzh_e2e.exe
//!          → 不抢注，直接验收【注册表里已安装的】旁译（装完 setup 后跑这个）。
//!          此时必须能看到 Program Files 的 DLL 被加载，否则直接判「结论作废」。
//!
//! 两个环境相关的坑（2026-10-10 都踩了）：
//!   1. 抢注必须在建窗【之前】。否则 TSF 聚焦那一刻会先把已安装的旁译
//!      （同一个 CLSID）加载进来，整场测试都变成在测旧 DLL。日志里出现
//!      「混进旧 DLL」即作废。
//!   2. LOCALAPPDATA 是复用的临时目录时，要先写死 settings.txt(max=5) 并删掉
//!      learned.txt —— 引擎的自学习会把上次提交过的词顶到候选前面，位置断言会漂。
#![allow(non_snake_case)]
use windows::core::{s, GUID, HRESULT, Interface, IUnknown, PCWSTR, Result};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::ProcessStatus::{EnumProcessModules, GetModuleFileNameExW};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentProcess, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const OUR_CLSID: GUID = GUID::from_u128(0xB4E8F2A1_7C3D_4E5F_9A21_8D6C4B3E1F07);
const OUR_PROFILE: GUID = GUID::from_u128(0x5A7C9E12_3D4B_4F8A_B6E1_2C9D7A5E8F34);
const LANG: u16 = 0x0804;
const OUT: &str =
    r"C:\Users\33354\Documents\Codex\2026-10-08\github-yenharvey-qingjian\work\fullzh-e2e.txt";

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
static mut OK: u32 = 0;
static mut BAD: u32 = 0;
static mut HOOK: HHOOK = HHOOK(0);
static mut STALL: u32 = 0;

/// 只吃「打到我们自己窗口」的真人按键。测试全靠 SendInput 注入按键，注入
/// 事件带 LLKHF_INJECTED(0x10) 标志，真人按键没有 —— 前台是测试窗时
/// 没有标志的一律吞掉，不然用户在这台电脑上打字会混进测试队列（2026-10-10
/// 实测偶发 0/5 过，日志里混着 'd' 和 Alt 键，就是用户的键）。前台不是
/// 测试窗就放行。钩子进程死了自动摘除，Ctrl+Alt+Del 走安全桌面不受影响。
extern "system" fn ll_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            let info = &*(l.0 as *const KBDLLHOOKSTRUCT);
            if info.flags.0 & 0x10 == 0 && GetForegroundWindow() == MAIN {
                return LRESULT(1); // 真人按键打到测试窗，吃掉
            }
        }
        CallNextHookEx(HHOOK(0), code, w, l)
    }
}

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

/// 抢前台（复制自 fix_e2e：先抬到顶层再设焦点，多试几次）
/// 抢前台。用户点过别的窗口后 Windows 会挂前台锁，SetForegroundWindow
/// 直接被拒 —— 用 AttachThreadInput 把我们的输入队列挂到当前前台线程上
/// 再抢（app-test.ps1 同款手法），不然键全打进用户的别的应用。
unsafe fn focus_ours() {
    // 注意：GetWindowThreadProcessId 的返回值才是线程 id（出参写的是进程 id）
    let fg = GetForegroundWindow();
    let fg_tid = if fg != HWND(0) && fg != MAIN {
        GetWindowThreadProcessId(fg, None)
    } else {
        0
    };
    for _ in 0..3 {
        if GetForegroundWindow() == MAIN {
            break;
        }
        let me = GetCurrentThreadId();
        if fg_tid != 0 {
            let _ = AttachThreadInput(me, fg_tid, true);
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
        if fg_tid != 0 {
            let _ = AttachThreadInput(me, fg_tid, false);
        }
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
    let _ = SetWindowPos(MAIN, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
    SetFocus(EDIT);
    // 焦点刚到手时给 TSF 一点时间挂好按键接收器，键才不会白敲
    std::thread::sleep(std::time::Duration::from_millis(100));
}

/// 把 "yuebuqun1" 这样的脚本转成 VK 队列（小写字母/数字/空格）。
/// 手写 0x4D 这种码最容易抄错（第一版 H5 就把 ming 的 ing 抄丢了），
/// 直接用脚本文本，错了一眼能看出来。
unsafe fn keys(s: &str) -> Vec<u16> {
    s.chars()
        .map(|c| match c {
            'a'..='z' => c.to_ascii_uppercase() as u16,
            '0'..='9' | ' ' => c as u16,
            _ => 0,
        })
        .filter(|&v| v != 0)
        .collect()
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

/// 预载新 DLL 并把它的类工厂注册进本进程 —— TSF 激活 TIP 时进程内优先，
/// 于是不需要安装/改注册表就能用新 DLL 跑完整链路。
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
    let iid: GUID = GUID::from_u128(0x00000001_0000_0000_C000_000000000046);
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

/// 列出进程内所有 pangyi 相关模块 —— 用来确认 TSF 用的确实是预载的新 DLL
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
        let mut any = false;
        for m in mods.iter().take(n) {
            let mut buf = [0u16; 512];
            let len = GetModuleFileNameExW(me, *m, &mut buf);
            if len > 0 {
                let p = String::from_utf16_lossy(&buf[..len as usize]);
                if p.to_lowercase().contains("pangyi") {
                    logline(&format!("  模块: {p}"));
                    any = true;
                }
            }
        }
        if !any {
            logline("  （没找到 pangyi 模块？！）");
        }
    }
}

/// 进程里混进来的别的 pangyi DLL（比如 Program Files 里已安装的旧版）。
/// 非空 = 本轮结论作废：敲进去的字可能全走的旧版。
unsafe fn pollution() -> Vec<String> {
    let me = GetCurrentProcess();
    let mut mods = [HMODULE(0); 512];
    let mut need = 0u32;
    let mut bad = Vec::new();
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
                let low = p.to_lowercase();
                if low.contains("pangyi") && low.ends_with(".dll") && !low.contains("target\\release")
                {
                    bad.push(p);
                }
            }
        }
    }
    bad
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

/// 每个阶段的公共动作：读文本、判定、计数
unsafe fn check(tag: &str, expect: &str, want: &str) {
    let t = text_of();
    let ok = t.contains(want);
    if ok {
        OK += 1;
    } else {
        BAD += 1;
    }
    logline(&format!(
        "{tag}: 文档=[{t}]  期望含「{want}」（{expect}）  {}",
        if ok { "✓" } else { "✗ 不符合!" }
    ));
    flush();
}

/// 中止测试：记日志、结束窗口。
unsafe fn abort(h: HWND, why: &str) {
    logline(&format!("✗ {why}"));
    flush();
    let _ = DestroyWindow(h);
}

unsafe fn tick(h: HWND) {
    if QI < QUEUE.len() {
        // 每拍都确认前台还是自己再发键：中途被用户点走的话，SendInput 的
        // 键会打进用户正在用的别的应用。抢不回来就停发、超时中止，绝不乱发。
        if GetForegroundWindow() != MAIN {
            focus_ours();
            if GetForegroundWindow() != MAIN {
                STALL += 1;
                if STALL > 30 {
                    abort(h, "前台一直抢不回来（用户一直在别的窗口？），中止测试");
                }
                return; // 这一拍不发键，下一拍再试
            }
        }
        STALL = 0;
        send_code(QUEUE[QI]);
        QI += 1;
        return;
    }
    match PHASE {
        0 => {
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
            let bad = pollution();
            if std::env::var("FULLZH_INSTALLED").is_ok() {
                if bad.is_empty() {
                    logline("✗ 已安装的旁译 DLL 没被加载进来（注册表激活没生效）—— 结论作废");
                    flush();
                    let _ = DestroyWindow(h);
                    return;
                }
                for p in &bad {
                    logline(&format!("已安装 DLL: {p}"));
                }
            } else if !bad.is_empty() {
                for p in &bad {
                    logline(&format!("✗ 混进旧 DLL: {p}"));
                }
                logline("✗ 旧 DLL 被 TSF 先装进来了，敲的键可能全走旧版 —— 本轮结论作废");
                flush();
                let _ = DestroyWindow(h);
                return;
            }
            logline("环境干净，开始测");
            logline("=== H1: 打 yuebuqun 按 1 —— 「岳不群」全词无俄语释义，旧词典里打不出来 ===");
            QUEUE = keys("yuebuqun1");
            QI = 0;
            PHASE = 1;
        }
        1 => {
            check("H1", "无译文的词条现在能打出整词", "岳不群");
            clear_edit();
            logline("=== H2: 打 mianyang 按 2 —— 「沔阳」第 2 候选（第 1 是带译文的绵羊）===");
            QUEUE = keys("mianyang2");
            QI = 0;
            PHASE = 2;
        }
        2 => {
            check("H2", "候选排在有译文的词之后但可选", "沔阳");
            clear_edit();
            logline("=== H3: 打 yiliang 按 1 —— 「一辆」是 16 个候选里的第 1 ===");
            QUEUE = keys("yiliang1");
            QI = 0;
            PHASE = 3;
        }
        3 => {
            check("H3", "同一读音下大量新词不挡路", "一辆");
            clear_edit();
            logline("=== H4: 打 henduo 按 1 —— 带俄语释义的老词，回归 ===");
            QUEUE = keys("henduo1");
            QI = 0;
            PHASE = 4;
        }
        4 => {
            check("H4", "有译文的词仍然排第 1", "很多");
            clear_edit();
            logline("=== H5: 整句 womenmingtianqubeijing + 空格 —— 句子模式回归 ===");
            QUEUE = keys("womenmingtianqubeijing ");
            QI = 0;
            PHASE = 5;
        }
        5 => {
            check("H5", "新增的无译文词条没有拖坏整句切分", "我们明天去北京");
            logline(&format!(
                "=== 完：{OK} 过 / {BAD} 不过 ===",
            ));
            logline("=== 进程内 pangyi 模块（应只有 target\\release 的新 DLL）===");
            log_modules();
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
        let md = wide("Msftedit.dll");
        let _ = LoadLibraryW(PCWSTR(md.as_ptr()));
        // 固定测试配置（max=5），不依赖临时目录里残留的 settings.txt ——
        // 上一轮就被残留的 max=9 坑过：「按 6 不该选词」那条断言直接失真。
        let base = std::env::var("LOCALAPPDATA").unwrap_or_default();
        if !base.is_empty() {
            let dir = format!("{base}\\Pangyi");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(
                format!("{dir}\\settings.txt"),
                "max=5\nscale=100\nlayout=v\ntheme=0\nenglish=0\ndebug=0\n",
            );
            // 清掉上一轮的自学习记录。引擎会把以前提交过的词顶到候选最前面
            // （learned.txt 里有「绵阳 2 / 沔阳 1」这种计数），不清的话
            // 「按 2 选第 2 个候选」这种位置断言每一轮都在漂。
            let _ = std::fs::remove_file(format!("{dir}\\learned.txt"));
            let _ = std::fs::remove_file(format!("{dir}\\phrases.txt"));
        }
        // 抢注必须在【建窗/聚焦之前】。TSF 在窗口聚焦时会激活当前输入法；
        // 系统里装着旁译（同一个 CLSID）的话，那一下就把 Program Files 的旧
        // DLL 加载进来并生效，之后抢注也换不回来 —— 实测第二次跑整场都在用
        // 旧词典，H1 打 yuebuqun 出来的是旧版的「月不群」。
        let installed = std::env::var("FULLZH_INSTALLED").is_ok();
        let dll = std::env::args().nth(1).unwrap_or_default();
        if installed {
            // 验收模式：不抢注，让 TSF 按注册表加载【已安装】的旁译。
            logline("FULLZH_INSTALLED=1：走注册表里的已安装版本（不做进程内抢注）");
        } else {
            logline(&format!("预载 DLL: {dll}"));
            if !preload(&dll) {
                logline("预载失败，测试无法继续");
                flush();
                return Ok(());
            }
        }
        let cls = wide("PangyiFullZhE2E");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(6isize),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = wide("Pangyi Full-ZH E2E");
        // 窗口放在屏幕外 + 工具窗样式：用户看不见、任务栏没有按钮、点不到。
        // （普通窗口弹在屏幕上，用户会去点它/别的窗口，焦点一丢键就乱飞。）
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
        // 钩子要挂在同一个泵消息的线程上；装失败只影响抗干扰能力，不拦测试
        HOOK = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_hook), HMODULE(0), 0) {
            Ok(hk) => hk,
            Err(e) => {
                logline(&format!("装不上真人按键屏蔽钩子: {e:?}（用户打字可能混进测试）"));
                HHOOK(0)
            }
        };
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        if !HOOK.is_invalid() {
            let _ = UnhookWindowsHookEx(HOOK);
        }
    }
    Ok(())
}
