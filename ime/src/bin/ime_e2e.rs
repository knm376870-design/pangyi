//! 端到端回归测试：真实按键（SendInput）走完整条系统链路
//!   SendInput -> 系统输入队列 -> TSF -> 旁译(我们的 TIP) -> RichEdit
//!
//! 每次定时器只发一个键，发完立刻回到消息循环 —— TSF 的同步编辑会话
//! （RequestEditSession(TF_ES_SYNC)）需要消息循环还能跑，不能一口气灌进去。
//!
//! 覆盖用户报的场景：第 1 轮打字 → 切到微软拼音 → 切回旁译 → 第 2 轮再打字。
#![allow(non_snake_case)]
use windows::core::{ComInterface, GUID, PCWSTR, Result};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const OUR_CLSID: GUID = GUID::from_u128(0xB4E8F2A1_7C3D_4E5F_9A21_8D6C4B3E1F07);
const OUR_PROFILE: GUID = GUID::from_u128(0x5A7C9E12_3D4B_4F8A_B6E1_2C9D7A5E8F34);
const MS_CLSID: GUID = GUID::from_u128(0x81D4E9C9_1D3B_41BC_9E6C_4B40BF79E35E);
const MS_PROFILE: GUID = GUID::from_u128(0xFA550B04_5AD7_411F_A5AC_CA038EC515D7);
const LANG: u16 = 0x0804;
const OUT: &str = r"C:\Users\33354\Documents\Codex\2026-10-08\github-yenharvey-qingjian\work\ime-e2e.txt";

static mut MAIN: HWND = HWND(0);
static mut EDIT: HWND = HWND(0);
static mut PHASE: u32 = 0;      // 0 初始化 1 第一轮 2 读1 3 切MS 4 切回 5 第二轮 6 读2 7 结束
static mut QI: usize = 0;
static mut QUEUE: Vec<u16> = Vec::new();
static mut LOG: String = String::new();
static mut PROFILES: Option<ITfInputProcessorProfiles> = None;
static mut R1: String = String::new();

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }
fn logline(s: &str) { unsafe { LOG.push_str(s); LOG.push('\n'); } }

unsafe fn text_of() -> String {
    let mut buf = [0u16; 1024];
    let n = SendMessageW(EDIT, WM_GETTEXT, WPARAM(buf.len()), LPARAM(buf.as_mut_ptr() as isize)).0 as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}
unsafe fn clear_edit() {
    let e = wide("");
    SendMessageW(EDIT, WM_SETTEXT, WPARAM(0), LPARAM(e.as_ptr() as isize));
}
unsafe fn focus_ours() { SetForegroundWindow(MAIN); SetFocus(EDIT); }

unsafe fn send_one(vk: u16) {
    let mut ins = [INPUT::default(); 2];
    ins[0].r#type = INPUT_KEYBOARD;
    ins[0].Anonymous.ki.wVk = VIRTUAL_KEY(vk);
    ins[1] = ins[0];
    ins[1].Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
    let _ = SendInput(&ins, std::mem::size_of::<INPUT>() as i32);
}

unsafe fn activate(clsid: &GUID, profile: &GUID, who: &str) {
    match PROFILES.as_ref() {
        Some(p) => match p.ActivateLanguageProfile(clsid, LANG, profile) {
            Ok(()) => logline(&format!("切到 {who}: ok")),
            Err(e) => logline(&format!("切到 {who}: 失败 {e:?}")),
        },
        None => logline("没有 ITfInputProcessorProfiles"),
    }
    // 切完把焦点放回编辑框（真实用户也是这么做的）
    std::thread::sleep(std::time::Duration::from_millis(120));
    focus_ours();
}

/// 每次定时器：如果需要发键就发一个，否则推进阶段
unsafe fn tick(h: HWND) {
    let qlen = QUEUE.len();
    if (PHASE == 1 || PHASE == 5) && QI < qlen {
        let vk = QUEUE[QI];
        QI += 1;
        if QI == 1 { focus_ours(); }
        send_one(vk);
        return;
    }
    match PHASE {
        0 => {
            focus_ours();
            logline(&format!("前台是我们的窗口: {}", GetForegroundWindow() == MAIN));
            if let Ok(tm) = CoCreateInstance::<_, ITfThreadMgr>(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) {
                let _ = tm.Activate();
            }
            match CoCreateInstance::<_, ITfInputProcessorProfiles>(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) {
                Ok(p) => PROFILES = Some(p),
                Err(e) => logline(&format!("拿不到 profiles: {e:?}")),
            }
            activate(&OUR_CLSID, &OUR_PROFILE, "旁译");
            QUEUE = vec![0x4B, 0x41, 0x49, 0x46, 0x41, 0x20]; // k a i f a 空格
            QI = 0;
            PHASE = 1;
            logline("--- 第 1 轮：真实按键 kaifa + 空格 ---");
        }
        1 => {
            R1 = text_of();
            logline(&format!("第 1 轮结果: [{R1}]  出字={}", R1.contains('开')));
            activate(&MS_CLSID, &MS_PROFILE, "微软拼音");
            PHASE = 3;
        }
        3 => {
            logline("--- 切回旁译（用户报的问题就在这之后）---");
            activate(&OUR_CLSID, &OUR_PROFILE, "旁译(第二次)");
            clear_edit();
            QUEUE = vec![0x4E, 0x49, 0x48, 0x41, 0x4F, 0x20]; // n i h a o 空格
            QI = 0;
            PHASE = 5;
            logline("--- 第 2 轮：真实按键 nihao + 空格 ---");
        }
        5 => {
            let t = text_of();
            logline(&format!("第 2 轮结果: [{t}]  出字={}", t.contains('你')));
            logline("=== 结论 ===");
            logline(&format!("切走再切回后仍能打字: {}", t.contains('你')));
            std::fs::write(OUT, LOG.as_bytes()).ok();
            DestroyWindow(h);
        }
        _ => {}
    }
}

extern "system" fn wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER => { tick(h); LRESULT(0) }
            WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }
            _ => DefWindowProcW(h, msg, w, l),
        }
    }
}

fn main() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let md = wide("Msftedit.dll");
        let _ = LoadLibraryW(PCWSTR(md.as_ptr()));
        let cls = wide("PangyiE2E");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(6isize),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = wide("Pangyi E2E");
        let h = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(cls.as_ptr()), PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE, 140, 140, 760, 420, None, None, None, None);
        if h.0 == 0 { logline("建窗失败"); std::fs::write(OUT, LOG.as_bytes()).ok(); return Ok(()); }
        MAIN = h;
        let ec = wide("RICHEDIT50W");
        let empty = wide("");
        EDIT = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(ec.as_ptr()), PCWSTR(empty.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL) as u32),
            0, 0, 720, 360, h, None, None, None);
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