//! 候选翻页的端到端验证（2026-10-10，v4.2.0「像微软那样翻页看全部候选」）。
//!
//! 骨架和 fullzh_e2e.rs 完全一样：SendInput -> 系统队列 -> TSF -> 旁译 ->
//! RichEdit 全链路、LoadLibrary + CoRegisterClassObject 免安装用新 DLL、
//! LOCALAPPDATA 重定向到临时目录、抢注必须在建窗之前、删 learned/settings 防漂移。
//!
//! 期望值不是写死的词，而是进程内直接调 `pangyi_ime::dict::lookup()` 算出候选
//! 序列的第 N 条 —— 测试和被测引擎共用同一份词典和排序代码，以后词典更新了
//! 位置断言也不会过期。前提是各用例用互不相干的读音组：引擎的自学习会把你
//! 提交过的词顶到前面，同一个组连着测第二次位置就会漂。
//!
//!   P1  yi + PageDown + 1        → 第 2 页第 1 条（总第 10 条）
//!   P2  ji + `=` + 1             → 「=」翻页同款，第 2 页第 1 条
//!   P3  yu + PageDown×2 + 3      → 第 3 页第 3 条（总第 21 条）
//!   P4  fu + PageDown + ↑ + 空格 → 上键撞到页首自动翻回第 1 页并选最后一条（第 9 条）
//!   P5  li + PageDown + `-` + 1  → 「-」翻回来，第 1 页第 1 条
//!   P6  xi + 9                   → 回归：页内数字选词（第 9 条）
//!   B1  min + 退格 + 1           → 删掉一个字母后文档里的拼音变成 mi，按 1 选的是 mi 组第 1 条
//!   B2  nih + 退格 + 3           → 删掉一个字母后按 3 选的是 ni 组第 3 条（候选按剩余拼音重算了）
//!   I1  gz + 1                   → 首字母缩写首选「工作」（硬编码期望，不看自己的 lookup）
//!   I2  zhrmghg + 1              → 7 字母长缩写「中华人民共和国」（旧版 6 字母上限直接没候选）
//!   I3  zgrmjfj + 1              → 7 字母长缩写「中国人民解放军」= lookup 副本第 1 条（走线校验）
//!   I4  zhongguorenm + 1         → 长拼音尾巴补全：文档必须正好是「中国人」，不能是旧版「中国人呣」
//!
//! 用法：page_e2e.exe [新DLL路径]   （LOCALAPPDATA 由调用方重定向）
//!      FULLZH_INSTALLED=1 page_e2e.exe
//!          → 不抢注，直接验收【注册表里已安装的】旁译；进程里没看到已安装
//!            DLL 就判「结论作废」（和 fullzh_e2e 同一个开关）。
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
    r"C:\Users\33354\Documents\Codex\2026-10-08\github-yenharvey-qingjian\work\page-e2e.txt";

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

/// 只吃「打到我们自己窗口」的真人按键：测试靠 SendInput 注入按键（带
/// LLKHF_INJECTED 0x10 标志，一律放行）。前台是测试窗时吞掉真人按键，
/// 防止混进测试队列（2026-10-10 实测偶发翻车，日志里混着真人键）；
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

// 期望值：进程内用自己的词典副本算出来的候选序列（词），每条用例一套
static mut E_YI: Vec<String> = Vec::new();
static mut E_JI: Vec<String> = Vec::new();
static mut E_YU: Vec<String> = Vec::new();
static mut E_FU: Vec<String> = Vec::new();
static mut E_LI: Vec<String> = Vec::new();
static mut E_XI: Vec<String> = Vec::new();
static mut E_MI: Vec<String> = Vec::new();
static mut E_NI: Vec<String> = Vec::new();
static mut E_ZGRM: Vec<String> = Vec::new();

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

/// 脚本 → VK 队列。小写字母 / 数字 / 空格照打；特殊键用方括号：
///   [pd] PageDown  [pu] PageUp  [up]/[down] 方向键  [minus] `-`  [equal] `=`  [enter]
///   [bs] 退格（删掉组合里最后一个拼音字母）
/// 例："yi[pd]1"、"[equal]1"、"min[bs]1"
unsafe fn keys(s: &str) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '[' {
            let mut name = String::new();
            for n in it.by_ref() {
                if n == ']' {
                    break;
                }
                name.push(n);
            }
            out.push(match name.as_str() {
                "pd" => 0x22,
                "pu" => 0x21,
                "up" => 0x26,
                "down" => 0x28,
                "minus" => 0xBD,
                "equal" => 0xBB,
                "enter" => 0x0D,
                "bs" => 0x08,
                _ => continue,
            });
            continue;
        }
        match c {
            'a'..='z' => out.push(c.to_ascii_uppercase() as u16),
            '0'..='9' | ' ' => out.push(c as u16),
            _ => {}
        }
    }
    out
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
        // 方向键 / PgUp / PgDn 是扩展键，带上 EXTENDEDKEY 位更贴近真键盘
        if matches!(vk, 0x21 | 0x22 | 0x25..=0x28) {
            i.Anonymous.ki.dwFlags = KEYEVENTF_EXTENDEDKEY;
        }
        ins.push(i);
    }
    if !down_only {
        let mut i = INPUT::default();
        i.r#type = INPUT_KEYBOARD;
        i.Anonymous.ki.wVk = VIRTUAL_KEY(vk);
        i.Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
        if matches!(vk, 0x21 | 0x22 | 0x25..=0x28) {
            i.Anonymous.ki.dwFlags |= KEYEVENTF_EXTENDEDKEY;
        }
        ins.push(i);
    }
    let _ = SendInput(&ins, std::mem::size_of::<INPUT>() as i32);
}

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

/// 进程里混进来的别的 pangyi DLL（比如 Program Files 里已安装的）。
/// 非空（非安装模式）= 本轮结论作废：敲进去的键可能全走的旧版。
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

/// 判定：文档里出现期望词
unsafe fn check(tag: &str, note: &str, want: &str, idx: usize, total: usize) {
    let t = text_of();
    let ok = !want.is_empty() && t.contains(want);
    if ok {
        OK += 1;
    } else {
        BAD += 1;
    }
    logline(&format!(
        "{tag}: 文档=[{t}]  期望含「{want}」（第 {idx} 条 / 共 {total} 条候选；{note}）  {}",
        if ok { "✓" } else { "✗ 不符合!" }
    ));
    flush();
}

/// 候选序列的第 i 条（越界返回空串，check 会判不过）
unsafe fn at(list: &[String], i: usize) -> String {
    list.get(i).cloned().unwrap_or_default()
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
            // 期望值：直接用自己的词典副本算（此时还没提交过任何词，
            // learned.txt 已在 main 里删掉，序列是干净的）
            let seq = |k: &str| -> Vec<String> {
                pangyi_ime::dict::lookup(k).into_iter().map(|hh| hh.word).collect()
            };
            E_YI = seq("yi");
            E_JI = seq("ji");
            E_YU = seq("yu");
            E_FU = seq("fu");
            E_LI = seq("li");
            E_XI = seq("xi");
            E_MI = seq("mi");
            E_NI = seq("ni");
            E_ZGRM = seq("zgrmjfj");
            logline(&format!(
                "候选池：yi={} ji={} yu={} fu={} li={} xi={} mi={} ni={} zgrmjfj={}（每页 9 个）",
                E_YI.len(), E_JI.len(), E_YU.len(), E_FU.len(), E_LI.len(), E_XI.len(),
                E_MI.len(), E_NI.len(), E_ZGRM.len()
            ));
            // 前提：这几组必须真的超过一页，否则下面的翻页断言没有意义；
            // 长缩写组至少要有 1 条（旧版 6 字母上限下这里是 0）
            if E_YI.len() >= 100 && E_JI.len() >= 30 && E_YU.len() >= 30 && E_FU.len() >= 30
                && E_LI.len() >= 30 && E_XI.len() >= 30 && E_MI.len() >= 3 && E_NI.len() >= 3
                && E_ZGRM.len() >= 1
            {
                OK += 1;
                logline("候选池规模检查（≥ 多页）✓");
            } else {
                BAD += 1;
                logline("✗ 候选池不够翻页 —— 结论作废");
                flush();
                let _ = DestroyWindow(h);
                return;
            }
            logline("环境干净，开始测");
            logline("=== P1: 打 yi 翻一页按 1 —— 第 2 页第 1 条 ===");
            QUEUE = keys("yi[pd]1");
            QI = 0;
            PHASE = 1;
        }
        1 => {
            check("P1", "PageDown 翻页", &at(&E_YI, 9), 10, E_YI.len());
            clear_edit();
            logline("=== P2: 打 ji 按 = 翻页再按 1 ===");
            QUEUE = keys("ji[equal]1");
            QI = 0;
            PHASE = 2;
        }
        2 => {
            check("P2", "「=」翻页", &at(&E_JI, 9), 10, E_JI.len());
            clear_edit();
            logline("=== P3: 打 yu 连翻两页按 3 ===");
            QUEUE = keys("yu[pd][pd]3");
            QI = 0;
            PHASE = 3;
        }
        3 => {
            check("P3", "连翻两页 + 第 3 条", &at(&E_YU, 20), 21, E_YU.len());
            clear_edit();
            logline("=== P4: 打 fu 翻页后按 ↑ —— 应翻回第 1 页选最后一条 ===");
            QUEUE = keys("fu[pd][up] ");
            QI = 0;
            PHASE = 4;
        }
        4 => {
            check("P4", "↑ 撞页首自动翻回上一页", &at(&E_FU, 8), 9, E_FU.len());
            clear_edit();
            logline("=== P5: 打 li 翻页后按 - 再按 1 —— 应回到第 1 页 ===");
            QUEUE = keys("li[pd][minus]1");
            QI = 0;
            PHASE = 5;
        }
        5 => {
            check("P5", "「-」翻回来", &at(&E_LI, 0), 1, E_LI.len());
            clear_edit();
            logline("=== P6: 打 xi 直接按 9 —— 页内数字选词回归 ===");
            QUEUE = keys("xi9");
            QI = 0;
            PHASE = 6;
        }
        6 => {
            check("P6", "页内数字选词没被翻页改坏", &at(&E_XI, 8), 9, E_XI.len());
            clear_edit();
            logline("=== B1: 打 min 按退格 —— 文档里的拼音应变成 mi ===");
            QUEUE = keys("min[bs]");
            QI = 0;
            PHASE = 7;
        }
        7 => {
            // 组合中段检查：文档里应该是去掉最后一个字母的拼音（mi），
            // 而不是原样 min、也不是空
            let t = text_of();
            let ok = t == "mi";
            if ok {
                OK += 1;
            } else {
                BAD += 1;
            }
            logline(&format!(
                "B1 中段: 文档=[{t}]  期望=[mi]（删一个字母拼音就该短一截）  {}",
                if ok { "✓" } else { "✗ 不符合!" }
            ));
            flush();
            QUEUE = keys("1");
            QI = 0;
            PHASE = 8;
        }
        8 => {
            check("B1", "退格后按 1 选的是 mi 组第 1 条", &at(&E_MI, 0), 1, E_MI.len());
            clear_edit();
            logline("=== B2: 打 nih 按退格再按 3 —— 候选应按剩余拼音 ni 重算 ===");
            QUEUE = keys("nih[bs]3");
            QI = 0;
            PHASE = 9;
        }
        9 => {
            check("B2", "退格后按 3 选的是 ni 组第 3 条", &at(&E_NI, 2), 3, E_NI.len());
            clear_edit();
            logline("=== I1: 打 gz 按 1 —— 首字母缩写首选应是「工作」 ===");
            QUEUE = keys("gz1");
            QI = 0;
            PHASE = 10;
        }
        10 => {
            check("I1", "缩写 gz 首选工作", "工作", 1, 1);
            clear_edit();
            logline("=== I2: 打 zhrmghg 按 1 —— 7 字母长缩写应出中华人民共和国 ===");
            QUEUE = keys("zhrmghg1");
            QI = 0;
            PHASE = 11;
        }
        11 => {
            check("I2", "长缩写 zhrmghg 首选中华人民共和国", "中华人民共和国", 1, 1);
            clear_edit();
            logline("=== I3: 打 zgrmjfj 按 1 —— 长缩写候选走线（lookup 副本第 1 条） ===");
            QUEUE = keys("zgrmjfj1");
            QI = 0;
            PHASE = 12;
        }
        12 => {
            check("I3", "长缩写 zgrmjfj 第 1 条", &at(&E_ZGRM, 0), 1, E_ZGRM.len());
            clear_edit();
            logline("=== I4: 打 zhongguorenm 按 1 —— 文档必须正好是「中国人」（不能是 中国人呣） ===");
            QUEUE = keys("zhongguorenm1");
            QI = 0;
            PHASE = 13;
        }
        13 => {
            // contains 会放过「中国人呣」，这里必须全等
            let t = text_of();
            let ok = t == "中国人";
            if ok {
                OK += 1;
            } else {
                BAD += 1;
            }
            logline(&format!(
                "I4: 文档=[{t}]  期望正好=[中国人]（长拼音尾巴补全）  {}",
                if ok { "✓" } else { "✗ 不符合!" }
            ));
            logline(&format!("=== 完：{OK} 过 / {BAD} 不过 ==="));
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
        let md = wide("Msftedit.dll");
        let _ = LoadLibraryW(PCWSTR(md.as_ptr()));
        // 固定测试配置（max=9，正是用户看到"最多九个"的那个档），
        // 并清掉自学习/短语，保证位置断言可重复
        let base = std::env::var("LOCALAPPDATA").unwrap_or_default();
        if !base.is_empty() {
            let dir = format!("{base}\\Pangyi");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(
                format!("{dir}\\settings.txt"),
                "max=9\nscale=100\nlayout=v\ntheme=0\nenglish=0\ndebug=0\n",
            );
            let _ = std::fs::remove_file(format!("{dir}\\learned.txt"));
            let _ = std::fs::remove_file(format!("{dir}\\phrases.txt"));
        }
        // 抢注必须在【建窗/聚焦之前】（见 fullzh_e2e 的注释）
        let installed = std::env::var("FULLZH_INSTALLED").is_ok();
        let dll = std::env::args().nth(1).unwrap_or_default();
        if installed {
            logline("FULLZH_INSTALLED=1：走注册表里的已安装版本（不做进程内抢注）");
        } else {
            logline(&format!("预载 DLL: {dll}"));
            if !preload(&dll) {
                logline("预载失败，测试无法继续");
                flush();
                return Ok(());
            }
        }
        let cls = wide("PangyiPageE2E");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(6isize),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = wide("Pangyi Page E2E");
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
