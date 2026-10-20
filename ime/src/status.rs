//! 状态轮询 + 切换提示（v4.15.0；v4.16.0 加设置热重载 + 僵尸候选窗看门狗）。
//!
//! 挂在宿主 UI 线程上的一个消息级隐藏窗口 + 50ms 定时器，干四件事：
//!
//! 1. **Caps 影子对账**：GetKeyState 的开关位在「取过按键消息的线程」上是
//!    新鲜的 —— 本窗口挂在宿主路由按键的 UI 线程上，宿主就算把 Caps 键
//!    消化掉（浏览器/聊天软件），它的消息泵也摸过这个键，快照照样刷。
//!    影子在这里对表，caps_on() 再当场读一次 GetKeyState 兜底，两道都
//!    不依赖「Caps 键进 TSF」—— 消化宿主里大写英文照样出得来。
//! 2. **Shift 单击切中/英**（微软同款）：GetAsyncKeyState 读全局态，宿主
//!    消化键也看得到。单按 Shift（没夹别的键、40~500ms 内松开）翻英文模式，
//!    弹一个胶囊提示「中文模式 / EN 英文模式」—— 让切换有肉眼可见的反应。
//! 3. **设置热重载**（v4.16.0）：面板在别的进程写 settings.txt，本进程
//!    激活时读的那份就旧了。每 500ms 查一次文件变化时间，变了就重读
//!    english/emoji 两个开关（settings::reload_modes），翻转弹胶囊 ——
//!    面板里开表情模式，正在打的宿主 0.5 秒内就出表情。
//! 4. **僵尸候选窗看门狗**（v4.16.0）：打字打到一半直接关掉宿主窗口，
//!    浏览器/聊天软件进程往往还常驻（窗口没了进程还在），TSF 的通知
//!    时灵时不灵，候选窗就挂屏幕上变僵尸。候选窗是 WS_EX_NOACTIVATE，
//!    永远不会自己当前台 —— 前台窗口的进程不是本进程 = 宿主丢了前台
//!    （窗口关了/切走了）→ 立即收窗。只收窗不动组合状态，宿主回来
//!    接着打，下一个键候选窗照常出现。
//!
//! 不泵消息的宿主定时器不跑，就退回 track_caps/caps_on 的路径；胶囊提示
//! 是 WS_EX_NOACTIVATE，不抢前台不抢焦点，不打扰正在打字的宿主。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, VK_CAPITAL, VK_CONTROL, VK_LMENU, VK_LSHIFT,
    VK_LWIN, VK_MENU, VK_RETURN, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// 轮询窗口（消息级，宿主 UI 线程上；进程内只建一次）
static POLLER: AtomicBool = AtomicBool::new(false);
/// IME 是否处于激活态（没激活时轮询照跑但不切换、不弹提示）
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Shift 按下起点（ms）与「已夹别的键」标记
struct TapState {
    t0: u32,
    dirty: bool,
}
static TAP: Mutex<Option<TapState>> = Mutex::new(None);

static mut TOAST_HWND: HWND = HWND(0);
static mut TOAST_FONT: HFONT = HFONT(0);
static mut TOAST_TEXT: Vec<u16> = Vec::new();
static TOAST_UNTIL: AtomicU32 = AtomicU32::new(0);
/// 每拍加一的计数器（第 10 拍 = 500ms 查一次设置文件）
static TICK_CNT: AtomicU32 = AtomicU32::new(0);
/// 上次看到的 settings.txt 修改时间（Unix 毫秒）
static CFG_MTIME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn now_ms() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_millis() & 0xFFFF_FFFF) as u32)
        .unwrap_or(0)
}

/// activate() 时调：确保宿主 UI 线程上有轮询窗口（重复调用无害）
pub fn ensure_poller() {
    if POLLER.swap(true, Ordering::Relaxed) {
        return;
    }
    unsafe {
        let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
        let cls: Vec<u16> = "PangyiStatusPoll\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(poll_proc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hInstance: hinst.into(),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        let h = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(cls.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            0, 0, 0, 0,
            HWND_MESSAGE, None, hinst, None,
        );
        if h.0 != 0 {
            let _ = SetTimer(h, 1, 50, None);
            crate::log("status 轮询窗口已挂（50ms）");
        }
    }
}

pub fn set_active(v: bool) {
    ACTIVE.store(v, Ordering::Relaxed);
}

extern "system" fn poll_proc(h: HWND, msg: u32, _w: WPARAM, _l: LPARAM) -> LRESULT {
    if msg == WM_TIMER {
        unsafe { status_tick(); }
        return LRESULT(0);
    }
    if msg == WM_NCDESTROY {
        // 轮询窗口死了就允许重建
        POLLER.store(false, Ordering::Relaxed);
    }
    unsafe { DefWindowProcW(h, msg, _w, _l) }
}

/// 定时器每次进来干的事（50ms 一拍；开销 = 几十次 GetAsyncKeyState，可忽略）
unsafe fn status_tick() {
    // 1) Caps 开关位对账 —— 本线程取过宿主 UI 线程队列里的按键，快照新鲜。
    //    真变化才写影子（和 track_caps 共用 CAPS_GS_LAST 基线，两边不会打架）。
    let gs = GetKeyState(VK_CAPITAL.0 as i32) as u16 & 0x0001 != 0;
    if gs != crate::CAPS_GS_LAST.swap(gs, Ordering::Relaxed) {
        crate::CAPS_SHADOW.store(gs, Ordering::Relaxed);
        crate::log(&format!("status 轮询对账 Caps 影子<-{gs}"));
        if ACTIVE.load(Ordering::Relaxed) {
            toast(if gs { "Caps 开 · 大写英文" } else { "Caps 关" });
        }
    }

    // 2) Shift 单击 = 切中/英。全局异步键态，宿主消化 Shift 也看得到。
    let sh = (GetAsyncKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0
        || (GetAsyncKeyState(VK_LSHIFT.0 as i32) as u16 & 0x8000) != 0
        || (GetAsyncKeyState(VK_RSHIFT.0 as i32) as u16 & 0x8000) != 0;
    let now = now_ms();
    let mut tap = TAP.lock().unwrap();
    match &mut *tap {
        None => {
            if sh {
                // 一次新按住开始：清掉「有键进过 TSF」的旧账，本段 Shift
                // 期间从头记（KEY_ROUTED 在 OnTestKeyDown 当场置位）
                crate::KEY_ROUTED.store(false, Ordering::Relaxed);
                *tap = Some(TapState { t0: now, dirty: false });
            }
        }
        Some(t) => {
            if sh {
                // 夹了别的键（Shift+字母 / Ctrl+Shift 之类）= 正常组合键，不算单击。
                // 两道判据：轮询采样看得到的物理键（other_key_down）+ TSF 路由
                // 当场置位的标志（KEY_ROUTED，快按快松的字母采样会漏、路由不漏）
                if !t.dirty && (other_key_down() || crate::KEY_ROUTED.load(Ordering::Relaxed)) {
                    t.dirty = true;
                }
            } else {
                let dt = now.wrapping_sub(t.t0);
                if !t.dirty && (40..=500).contains(&dt) && ACTIVE.load(Ordering::Relaxed) {
                    crate::settings::set_english(!crate::settings::english());
                    let on = crate::settings::english();
                    crate::log(&format!("status Shift 单击 英文模式->{on}"));
                    toast(if on { "EN 英文模式" } else { "中文模式" });
                }
                *tap = None;
            }
        }
    }

    // 3) 设置热重载：每 500ms（10 拍）查 settings.txt 修改时间，变了就重读
    //    english/emoji 开关。面板进程写盘后，宿主不用重开应用，开关当场
    //    生效；翻转弹胶囊给肉眼反馈。
    if TICK_CNT.fetch_add(1, Ordering::Relaxed) % 10 == 0 {
        let path = crate::settings::cfg_file();
        let stamp = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        if stamp != CFG_MTIME.swap(stamp, Ordering::Relaxed) {
            let (oe, om) = (crate::settings::english(), crate::settings::emoji());
            crate::settings::reload_modes();
            let (ne, nm) = (crate::settings::english(), crate::settings::emoji());
            if ACTIVE.load(Ordering::Relaxed) && (ne != oe || nm != om) {
                // 两个开关同时翻转时合成一条胶囊（toast 只有一条文本槽）
                let mut parts: Vec<&str> = Vec::new();
                if ne != oe {
                    parts.push(if ne { "EN 英文模式" } else { "中文模式" });
                }
                if nm != om {
                    parts.push(if nm { "表情模式 开" } else { "表情模式 关" });
                }
                toast(&parts.join(" · "));
            }
            crate::log(&format!("status 设置文件变化 -> 热重载 english={ne} emoji={nm}"));
        }
    }

    // 4) 僵尸候选窗看门狗（v4.16.0，见模块头）。候选窗可见且前台窗口的
    //    进程不是本进程 → 宿主丢了前台，收窗。前台窗口在切窗瞬间可能
    //    短暂为 0，也按「宿主不在前台」处理 —— 收窗无害，下个键又出来。
    if crate::cand::visible() {
        let fg = GetForegroundWindow();
        let mut fg_pid: u32 = 0;
        if fg.0 != 0 {
            let _ = GetWindowThreadProcessId(fg, Some(&mut fg_pid));
        }
        if fg_pid != std::process::id() {
            crate::cand::hide();
            crate::log(&format!("status 看门狗收候选窗（前台 pid {fg_pid} 不是本进程）"));
        }
    }
}

/// Shift 按住期间有没有别的键按着（字母/数字/空格/回车/修饰键都算）
fn other_key_down() -> bool {
    unsafe {
        for vk in 0x41u32..=0x5A {
            if (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 {
                return true;
            }
        }
        for vk in 0x30u32..=0x39 {
            if (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 {
                return true;
            }
        }
        for vk in [
            VK_SPACE.0, VK_RETURN.0, VK_CONTROL.0, VK_MENU.0, VK_LMENU.0,
            VK_RMENU.0, VK_LWIN.0, VK_RWIN.0,
        ] {
            if (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 {
                return true;
            }
        }
        false
    }
}

/// 弹一个胶囊提示（约 1.2 秒）。窗口没建过就现在建（在轮询线程上）。
unsafe fn toast(text: &str) {
    let h = ensure_toast();
    if h.0 == 0 {
        return;
    }
    TOAST_TEXT = text.encode_utf16().collect();
    TOAST_TEXT.push(0);
    // 位置：主屏底部中间偏上（别挡输入光标区太多）
    let cx = GetSystemMetrics(SM_CXSCREEN);
    let cy = GetSystemMetrics(SM_CYSCREEN);
    let w = 240;
    let ht = 44;
    let _ = SetWindowPos(
        h, HWND_TOPMOST,
        (cx - w) / 2, cy - 160, w, ht,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
    let _ = InvalidateRect(h, None, TRUE);
    TOAST_UNTIL.store(now_ms().wrapping_add(1200), Ordering::Relaxed);
    let _ = SetTimer(h, 2, 150, None);
}

unsafe fn ensure_toast() -> HWND {
    if TOAST_HWND.0 != 0 && IsWindow(TOAST_HWND).as_bool() {
        return TOAST_HWND;
    }
    let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
    let cls: Vec<u16> = "PangyiStatusToast\0".encode_utf16().collect();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(toast_proc),
        lpszClassName: PCWSTR(cls.as_ptr()),
        hInstance: hinst.into(),
        hbrBackground: HBRUSH(0),
        ..Default::default()
    };
    let _ = RegisterClassW(&wc);
    let h = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
        PCWSTR(cls.as_ptr()),
        PCWSTR::null(),
        WS_POPUP,
        0, 0, 240, 44,
        None, None, hinst, None,
    );
    TOAST_HWND = h;
    h
}

extern "system" fn toast_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER => {
                if now_ms() >= TOAST_UNTIL.load(Ordering::Relaxed) {
                    let _ = KillTimer(h, 2);
                    let _ = ShowWindow(h, SW_HIDE);
                }
                return LRESULT(0);
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                let mut r = RECT::default();
                let _ = GetClientRect(h, &mut r);
                // 胶囊：圆角矩形 + 强调色底
                let a = &crate::settings::ACCENTS
                    [crate::settings::accent() as usize][crate::cand::is_dark() as usize];
                let brush = CreateSolidBrush(a.strong);
                let rgn = CreateRoundRectRgn(r.left, r.top, r.right, r.bottom, 22, 22);
                FillRgn(dc, rgn, brush);
                let _ = DeleteObject(rgn);
                let _ = DeleteObject(brush);
                if TOAST_FONT.0 == 0 {
                    TOAST_FONT = CreateFontW(
                        18, 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0,
                        windows::core::w!("Segoe UI"),
                    );
                }
                let old = SelectObject(dc, TOAST_FONT);
                let _ = SetBkMode(dc, TRANSPARENT);
                let _ = SetTextColor(dc, a.on_strong);
                let mut rt = r;
                rt.left += 6;
                rt.right -= 6;
                // windows-rs 0.52 的 DrawTextW 吃 &mut [u16]（长度即 cch），
                // 去掉结尾的 \0 再喂，避免空字符被算进绘制范围
                let n = TOAST_TEXT.len().saturating_sub(1);
                let _ = DrawTextW(dc, &mut TOAST_TEXT[..n], &mut rt, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                SelectObject(dc, old);
                EndPaint(h, &ps);
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            _ => {}
        }
        DefWindowProcW(h, msg, w, l)
    }
}
