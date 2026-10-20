//! 本轮修复的端到端验证（2026-10-10）。
//!
//! 和 ime_e2e.rs 同一个骨架：真实 SendInput 按键，走
//!   SendInput -> 系统队列 -> TSF -> 旁译 -> RichEdit
//! 完整链路。区别在于：
//!
//! 1. **不依赖安装**：先把指定路径的新 DLL LoadLibrary 进本进程，
//!    再 CoRegisterClassObject 抢注类工厂 —— TSF 激活 CLSID 时
//!    进程内注册优先生效，用的是新 DLL，不动注册表。
//!    （日志里会列出进程内所有 pangyi 相关模块路径，若出现 Program Files
//!     的旧 DLL 说明抢注没生效，测试结论作废。）
//! 2. **不碰用户配置**：用 LOCALAPPDATA 指向临时目录，settings.txt 在那里，
//!    Shift 切换写盘也写到那里。
//! 3. 按固定剧本逐项验证本轮修复：
//!    A  ESC 取消输入无残留        （修复 8a）
//!    B  退格删到空无残留          （修复 8b）
//!    C  「单独按一下 Shift」→ 英文（修复 3）
//!    D  再按一下 Shift → 回中文   （修复 3）
//!    E  Shift+字母 输出大写 A、不误切换（v4.2.3 大写直通）
//!    F  E 之后再打字仍是中文（证明没被误切换）
//!    G2 拼音下按 1 正常选第一个候选（回归）
//!    G  「候选个数=5」时按 6 不选词、字母落到文档（修复 5）
//!    G3 拼音中 Shift+H：首选「你」上屏 + 大写 H 直通（v4.2.3）
//!    G7 Caps 在激活前就开好 + 打 A → 大写 A 直通（v4.13.0 影子状态）
//!    M1 gzuo+空格 → 工作（缩写+全拼混合，v4.13.0）
//!    M2 zgren+空格 → 中国人
//!    M3 gzuorenyuan+空格 → 工作人员
//!    M4 zhguoren+空格 → 中国人（zh 双声母混打）
//!    M5 gzuor+空格 → 工作（混打 + 尾巴补全）
//!    M6 gz+空格 → 工作（纯缩写回归，混打改动不得破坏老缩写）
//!    C1 womn + ← + e + 空格 → 我们（← 回打错处插入，v4.14.0 光标）
//!    C2 nihao + ←← + 退格 + 空格 → 鸟（退格删光标前字母）
//!    C3 nihaoo + ← + Del + 空格 → 你好（Del 删光标后字母）
//!    C4 ihao + Home + ←← + n + 空格 → 你好（Home 跳串首、← 在 0 位不动）
//!    C5 nihao + Home + End + ma + 空格 → 你好吗（End 回串尾再接着打）
//!    C6 nihaox + ←← + → + Del + 空格 → 你好（→ 右移光标再删尾）
//!    C7 nihao + Home + 退格 + 空格 → 你好（光标在 0 退格不动）
//!
//! 用法：fix_e2e.exe [新DLL路径]  （LOCALAPPDATA 由调用方重定向）
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
    r"C:\Users\33354\Documents\Codex\2026-10-08\github-yenharvey-qingjian\work\fix-e2e.txt";

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
static mut HOOK: HHOOK = HHOOK(0);
static mut STALL: u32 = 0;
static mut IN_TICK: bool = false;

/// 只吃「打到我们自己窗口」的真人按键：测试靠 SendInput 注入按键（带
/// LLKHF_INJECTED 0x10 标志，一律放行）。前台是测试窗时吞掉真人按键，
/// 防止混进测试队列（2026-10-10 实测偶发 0/5 过，日志里混着 'd' 和 Alt）；
/// 前台不是测试窗就放行 —— 用户该干嘛干嘛。
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
    // 先 ESC 取消可能残留的 TSF 组合（丢键干扰时上一用例的组合没提交，
    // 不清掉会串进下一个用例 —— 2026-10-18 实测 [nigzuo你工作] 连锁），
    // 再清文档。ESC 没有组合时只是直通，RichEdit 无副作用。
    send_code(0x1B);
    pump_ms(120);
    let e = wide("");
    SendMessageW(EDIT, WM_SETTEXT, WPARAM(0), LPARAM(e.as_ptr() as isize));
    // WM_SETTEXT 之后也泵一拍：宿主（RichEdit）的文本替换可能要等消息泵
    // 才落进 TSF 文本仓库，落在下一用例打字中途会把刚开的组合干掉
    // （2026-10-18 M3 出现 [gz工作人员]：组合在打第 3 个键前被宿主终止，
    // 前两个拼音字母变成正文）。清完再给 120ms 让它落地。
    pump_ms(120);
}

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
    // 焦点刚到手时给 TSF 一点时间挂好按键接收器，键才不会白敲
    std::thread::sleep(std::time::Duration::from_millis(100));
}

/// 手动泵消息 ms 毫秒：tick 处理器里同步干的活（注入按键、激活 TSF）之后
/// 队列里的消息要等 tick 返回才会被泵 —— 注入的按键在「激活前」需要先被
/// 宿主消化掉（比如 G7 的 Caps 要先过宿主队列，Activate 时的 GetKeyState
/// 快照才新鲜）。WM_TIMER 是低优先级消息，泵队列里没别的消息不会重入。
unsafe fn pump_ms(ms: u64) {
    let t0 = std::time::Instant::now();
    let mut m = MSG::default();
    while t0.elapsed().as_millis() < ms as u128 {
        while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
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
    type GetFn = unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut core::ffi::c_void) -> HRESULT;
    let get: GetFn = std::mem::transmute(f);
    // IClassFactory 的 IID
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

/// 每个阶段的公共动作：读文本、写阶段性结论
unsafe fn result(tag: &str, expect: &str, ok: bool) -> String {
    let t = text_of();
    logline(&format!(
        "{tag}: 文档=[{t}]  期望{expect}  {} ",
        if ok { "✓" } else { "✗ 不符合!" }
    ));
    flush();
    t
}

/// 中止测试：恢复用户 CapsLock 原状、记日志、结束窗口。
unsafe fn abort(h: HWND, why: &str) {
    logline(&format!("✗ {why}"));
    // 测试开过 CapsLock（G5/G6）的话，别把翻过来的状态留给用户
    if (GetKeyState(VK_CAPITAL.0 as i32) as u16 & 1) != 0 {
        send_code(0x14);
    }
    flush();
    let _ = DestroyWindow(h);
}

unsafe fn tick(h: HWND) {
    // 重入保护：tick 里的 pump_ms 会嵌套泵消息，260ms 定时器在里面又触发
    // WM_TIMER 的话会无限递归（2026-10-18 实测栈溢出）。重入的这一拍直接丢。
    if IN_TICK {
        return;
    }
    IN_TICK = true;
    // RAII 复位：tick 中途有多个提前 return（抢不到前台中止、旧 DLL 作废），
    // 手动在每个出口复位必漏。守卫随函数退出自动清。
    struct TickGuard;
    impl Drop for TickGuard {
        fn drop(&mut self) {
            unsafe { IN_TICK = false; }
        }
    }
    let _tg = TickGuard;
    if QI < QUEUE.len() {
        // 每拍都确认前台还是自己再发键：中途被用户点走的话，SendInput 的
        // 键会打进用户正在用的别的应用（2026-10-10 实测 4/12 —— C 只到
        // [ni]、F 以后全空，键全飞了）。抢不回来就停发、超时中止，绝不乱发。
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
            // G7 前置：输入法激活之前就把 Caps 开好 —— 复现「Caps 早就在
            // 别的应用/更早的时刻开了，打字输不进大写」的场景。
            // 注入后先泵消息让宿主消化掉 Caps 键（激活时 GetKeyState 快照
            // 才新鲜；真实场景里 Caps 在别的应用按完，焦点切过来时宿主
            // 早已泵过消息，时序就是这样）。
            focus_ours();
            send_code(0x14);
            pump_ms(200);
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
            if !bad.is_empty() {
                for p in &bad {
                    logline(&format!("✗ 混进旧 DLL: {p}"));
                }
                logline("✗ 旧 DLL 被 TSF 先装进来了，敲的键可能全走旧版 —— 本轮结论作废");
                flush();
                let _ = DestroyWindow(h);
                return;
            }
            logline("进程内只有新 DLL，开始测");
            logline("=== G7: 激活前 Caps 已开，打 A —— 大写 A 直通 ===");
            QUEUE = vec![0x41]; // A
            QI = 0;
            PHASE = 1;
        }
        1 => {
            let t = text_of();
            let _ = result("G7", "正好一个大写 A（激活前开好的 Caps 也能直通）", t == "A");
            clear_edit();
            send_code(0x14); // Caps 关（影子状态靠按键沿翻转）
            logline("=== A: 打 nihao 后按 ESC，文档应为空（旧版残留 nihao）===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x1B]; // n i h a o ESC
            QI = 0;
            PHASE = 2;
        }
        2 => {
            let _ = result("A", "空（ESC 取消无残留）", text_of().is_empty());
            clear_edit();
            logline("=== B: 打 ni 后退格两次删空，文档应为空 ===");
            QUEUE = vec![0x4E, 0x49, 0x08, 0x08]; // n i BS BS
            QI = 0;
            PHASE = 3;
        }
        3 => {
            let _ = result("B", "空（删空无残留）", text_of().is_empty());
            clear_edit();
            logline("=== C: 单独按一下 Shift，再打 nihao+空格，应是英文直通 ===");
            QUEUE = vec![0x10]; // Shift 单独按一下
            QI = 0;
            PHASE = 4;
        }
        4 => {
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x20]; // n i h a o 空格
            QI = 0;
            PHASE = 5;
        }
        5 => {
            let t = text_of();
            let en = t.contains("nihao") && !t.contains('你');
            let _ = result("C", "英文直通 nihao、不出「你」", en);
            clear_edit();
            logline("=== D: 再按一下 Shift，回到中文 ===");
            QUEUE = vec![0x10];
            QI = 0;
            PHASE = 6;
        }
        6 => {
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x20];
            QI = 0;
            PHASE = 7;
        }
        7 => {
            let t = text_of();
            let _ = result("D", "出「你好」（切回中文）", t.contains('你'));
            clear_edit();
            logline("=== E: Shift 按住 + 字母 A + 松 Shift + ESC —— 大写 A 直通、不误切换 ===");
            // Shift 按下（只按不松）→ A 一按一松 → Shift 抬起 → ESC
            QUEUE = vec![0x8010, 0x41, 0x4010, 0x1B];
            QI = 0;
            PHASE = 8;
        }
        8 => {
            let t = text_of();
            let _ = result("E", "正好一个大写 A（直通输出，且 Shift+A 不算单独按 Shift）", t == "A");
            clear_edit();
            logline("=== G2: 打 ni 按 1，应选第一个候选「你」 ===");
            QUEUE = vec![0x4E, 0x49, 0x31]; // n i '1'
            QI = 0;
            PHASE = 9;
        }
        9 => {
            let t = text_of();
            let _ = result("G2", "出「你」", t.contains('你'));
            clear_edit();
            logline("=== F: E 之后再打 nihao+空格，应仍是中文「你好」 ===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x20];
            QI = 0;
            PHASE = 10;
        }
        10 => {
            let t = text_of();
            let _ = result("F", "出「你好」（Shift+A 没有误切换）", t.contains('你'));
            clear_edit();
            logline("=== G: 候选个数=5 时，打 ni 按 6：不应选中第 6 个候选「拟」，数字 6 应落到文档 ===");
            // 末尾补一个 ESC：把残余拼音 ni 取消掉，别污染下一个阶段
            QUEUE = vec![0x4E, 0x49, 0x36, 0x1B]; // n i '6' ESC
            QI = 0;
            PHASE = 11;
        }
        11 => {
            let t = text_of();
            let _ = result("G", "含字符 '6'、不含「拟」（旧版会提交看不见的第 6 候选）", t.contains('6') && !t.contains('拟'));
            clear_edit();
            logline("=== G3: 打 ni 按 Shift+H —— 首选「你」上屏 + 大写 H 直通 ===");
            // n i Shift按下 H Shift抬起
            QUEUE = vec![0x4E, 0x49, 0x8010, 0x48, 0x4010];
            QI = 0;
            PHASE = 12;
        }
        12 => {
            let t = text_of();
            let _ = result("G3", "正好「你H」（首选上屏 + 大写直通）", t == "你H");
            clear_edit();
            logline("=== G4: 打 ni 按逗号 —— 首选「你」上屏 + 逗号不丢（旧版标点被 TSF 吞掉）===");
            QUEUE = vec![0x4E, 0x49, 0xBC]; // n i ','
            QI = 0;
            PHASE = 13;
        }
        13 => {
            let t = text_of();
            // v4.3.0 起中文标点默认开：逗号转全角。这里断言的是「不丢 + 首选上屏」，
            // 标点跟随当前版本的全角行为。
            let _ = result("G4", "正好「你，」（首选上屏 + 全角逗号）", t == "你，");
            clear_edit();
            logline("=== G5: Caps 开 + 字母 A + Caps 关 —— 大写 A 直通（Caps 不吞字母）===");
            QUEUE = vec![0x14, 0x41, 0x14]; // Caps A Caps
            QI = 0;
            PHASE = 14;
        }
        14 => {
            let t = text_of();
            let _ = result("G5", "正好一个大写 A（Caps 直通）", t == "A");
            clear_edit();
            logline("=== G6: 打 ni、Caps 开、按 H、Caps 关 —— 首选「你」上屏 + 大写 H ===");
            QUEUE = vec![0x4E, 0x49, 0x14, 0x48, 0x14]; // n i Caps H Caps
            QI = 0;
            PHASE = 15;
        }
        15 => {
            let t = text_of();
            let _ = result("G6", "正好「你H」（Caps 下首选上屏 + 大写）", t == "你H");
            clear_edit();
            logline("=== M1: gzuo + 空格 —— 缩写+全拼混合 → 工作（v4.13.0）===");
            QUEUE = vec![0x47, 0x5A, 0x55, 0x4F, 0x20]; // g z u o 空格
            QI = 0;
            PHASE = 16;
        }
        16 => {
            let t = text_of();
            let _ = result("M1", "正好「工作」", t == "工作");
            clear_edit();
            logline("=== M2: zgren + 空格 —— 单字母缩写+全拼+全拼 → 中国人 ===");
            QUEUE = vec![0x5A, 0x47, 0x52, 0x45, 0x4E, 0x20]; // z g r e n 空格
            QI = 0;
            PHASE = 17;
        }
        17 => {
            let t = text_of();
            let _ = result("M2", "正好「中国人」", t == "中国人");
            clear_edit();
            logline("=== M3: gzuorenyuan + 空格 —— 混打整句 → 工作人员 ===");
            QUEUE = vec![0x47, 0x5A, 0x55, 0x4F, 0x52, 0x45, 0x4E, 0x59, 0x55, 0x41, 0x4E, 0x20]; // g z u o r e n y u a n 空格
            QI = 0;
            PHASE = 18;
        }
        18 => {
            let t = text_of();
            let _ = result("M3", "正好「工作人员」", t == "工作人员");
            clear_edit();
            logline("=== M4: zhguoren + 空格 —— zh 双声母混打 → 中国人 ===");
            QUEUE = vec![0x5A, 0x48, 0x47, 0x55, 0x4F, 0x52, 0x45, 0x4E, 0x20]; // z h g u o r e n 空格
            QI = 0;
            PHASE = 19;
        }
        19 => {
            let t = text_of();
            let _ = result("M4", "正好「中国人」", t == "中国人");
            clear_edit();
            logline("=== M5: gzuor + 空格 —— 混打 + 尾巴补全，首选仍是整词 → 工作 ===");
            QUEUE = vec![0x47, 0x5A, 0x55, 0x4F, 0x52, 0x20]; // g z u o r 空格
            QI = 0;
            PHASE = 20;
        }
        20 => {
            let t = text_of();
            let _ = result("M5", "正好「工作」", t == "工作");
            clear_edit();
            logline("=== M6: gz + 空格 —— 纯缩写回归 → 工作 ===");
            QUEUE = vec![0x47, 0x5A, 0x20]; // g z 空格
            QI = 0;
            PHASE = 21;
        }
        21 => {
            let t = text_of();
            let _ = result("M6", "正好「工作」", t == "工作");
            clear_edit();
            logline("=== C1: womn + ← + e + 空格 —— 回到打错处插入 → 我们（v4.14.0 光标）===");
            QUEUE = vec![0x57, 0x4F, 0x4D, 0x4E, 0x25, 0x45, 0x20]; // w o m n ← e 空格
            QI = 0;
            PHASE = 22;
        }
        22 => {
            let t = text_of();
            let _ = result("C1", "正好「我们」", t == "我们");
            clear_edit();
            logline("=== C2: nihao + ←← + 退格 + 空格 —— 退格删光标前字母 → 鸟 ===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x25, 0x25, 0x08, 0x20]; // n i h a o ← ← BS 空格
            QI = 0;
            PHASE = 23;
        }
        23 => {
            let t = text_of();
            let _ = result("C2", "正好「鸟」", t == "鸟");
            clear_edit();
            logline("=== C3: nihaoo + ← + Del + 空格 —— Del 删光标后字母 → 你好 ===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x4F, 0x25, 0x2E, 0x20]; // n i h a o o ← Del 空格
            QI = 0;
            PHASE = 24;
        }
        24 => {
            let t = text_of();
            let _ = result("C3", "正好「你好」", t == "你好");
            clear_edit();
            logline("=== C4: ihao + Home + ←← + n + 空格 —— Home 跳串首、← 在 0 位不动 → 你好 ===");
            QUEUE = vec![0x49, 0x48, 0x41, 0x4F, 0x24, 0x25, 0x25, 0x4E, 0x20]; // i h a o Home ← ← n 空格
            QI = 0;
            PHASE = 25;
        }
        25 => {
            let t = text_of();
            let _ = result("C4", "正好「你好」", t == "你好");
            clear_edit();
            logline("=== C5: nihao + Home + End + ma + 空格 —— End 回串尾接着打 → 你好吗 ===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x24, 0x23, 0x4D, 0x41, 0x20]; // n i h a o Home End m a 空格
            QI = 0;
            PHASE = 26;
        }
        26 => {
            let t = text_of();
            let _ = result("C5", "正好「你好吗」", t == "你好吗");
            clear_edit();
            logline("=== C6: nihaox + ←← + → + Del + 空格 —— → 右移光标再删尾 → 你好 ===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x58, 0x25, 0x25, 0x27, 0x2E, 0x20]; // n i h a o x ← ← → Del 空格
            QI = 0;
            PHASE = 27;
        }
        27 => {
            let t = text_of();
            let _ = result("C6", "正好「你好」", t == "你好");
            clear_edit();
            logline("=== C7: nihao + Home + 退格 + 空格 —— 光标在 0 退格不动 → 你好 ===");
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x24, 0x08, 0x20]; // n i h a o Home BS 空格
            QI = 0;
            PHASE = 28;
        }
        28 => {
            let t = text_of();
            let _ = result("C7", "正好「你好」", t == "你好");
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
        let md = wide("Msftedit.dll");
        let _ = LoadLibraryW(PCWSTR(md.as_ptr()));
        // 固定测试配置（max=5）——G 那条「候选个数=5」的断言依赖它。
        let base = std::env::var("LOCALAPPDATA").unwrap_or_default();
        if !base.is_empty() {
            let dir = format!("{base}\\Pangyi");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(
                format!("{dir}\\settings.txt"),
                "max=5\nscale=100\nlayout=v\ntheme=0\nenglish=0\ndebug=0\ncnpunct=1\n",
            );
        }
        // 抢注必须在【建窗/聚焦之前】—— 否则 TSF 聚焦时会先把已安装的旁译
        // （同一个 CLSID）从 Program Files 加载进来，之后抢注换不回来，
        // 整场测试都在用旧 DLL。（2026-10-10 实测踩到）
        let dll = std::env::args().nth(1).unwrap_or_default();
        logline(&format!("预载 DLL: {dll}"));
        if !preload(&dll) {
            logline("预载失败，测试无法继续");
            flush();
            return Ok(());
        }
        let cls = wide("PangyiFixE2E");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(6isize),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = wide("Pangyi Fix E2E");
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
