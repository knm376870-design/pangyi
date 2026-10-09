//! 快捷键回归测试：验证 Ctrl+A 这类组合键不会再被输入法吞掉。
//!
//! 做法：把编辑框内容设成 "AAA" → 发 Ctrl+A（全选）→ 发一个 "7"。
//!   如果结果是 "7"    → Ctrl+A 生效了（选中被替换）   ✅
//!   如果结果是 "AAA7" → Ctrl+A 被输入法吃掉了          ❌
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
const LANG: u16 = 0x0804;
const OUT: &str = r"C:\Users\33354\Documents\Codex\2026-10-08\github-yenharvey-qingjian\work\shortcut-result.txt";

static mut MAIN: HWND = HWND(0);
static mut EDIT: HWND = HWND(0);
static mut PHASE: u32 = 0;
static mut LOG: String = String::new();

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }
fn logline(s: &str) { unsafe { LOG.push_str(s); LOG.push('\n'); } }

unsafe fn text_of() -> String {
    let mut buf = [0u16; 512];
    let n = SendMessageW(EDIT, WM_GETTEXT, WPARAM(buf.len()), LPARAM(buf.as_mut_ptr() as isize)).0 as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}
unsafe fn set_text(s: &str) {
    let w = wide(s);
    SendMessageW(EDIT, WM_SETTEXT, WPARAM(0), LPARAM(w.as_ptr() as isize));
}
unsafe fn focus_ours() {
    for _ in 0..8 {
        if GetForegroundWindow() == MAIN { break; }
        let _ = SetWindowPos(MAIN, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
        let _ = SetForegroundWindow(MAIN);
        let _ = BringWindowToTop(MAIN);
        SetFocus(EDIT);
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    let _ = SetWindowPos(MAIN, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
    SetFocus(EDIT);
}
unsafe fn key(vk: u16, down: bool) {
    let mut ins = INPUT::default();
    ins.r#type = INPUT_KEYBOARD;
    ins.Anonymous.ki.wVk = VIRTUAL_KEY(vk);
    ins.Anonymous.ki.dwFlags = if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP };
    let _ = SendInput(&[ins], std::mem::size_of::<INPUT>() as i32);
    std::thread::sleep(std::time::Duration::from_millis(45));
}

unsafe fn tick(h: HWND) {
    PHASE += 1;
    match PHASE {
        1 => {
            focus_ours();
            if let Ok(tm) = CoCreateInstance::<_, ITfThreadMgr>(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) {
                let _ = tm.Activate();
            }
            if let Ok(p) = CoCreateInstance::<_, ITfInputProcessorProfiles>(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) {
                let _ = p.ActivateLanguageProfile(&OUR_CLSID, LANG, &OUR_PROFILE);
            }
            std::thread::sleep(std::time::Duration::from_millis(700));
            focus_ours();
            set_text("AAA");
            logline("编辑框设成 [AAA]（中文模式）");
            SetTimer(h, 1, 700, None);
        }
        2 => {
            focus_ours();
            logline("发送 Ctrl+A（全选）");
            key(0x11, true);   // Ctrl down
            key(0x41, true);   // A down
            key(0x41, false);
            key(0x11, false);  // Ctrl up
            SetTimer(h, 1, 600, None);
        }
        3 => {
            focus_ours();
            logline("发送 7");
            key(0x37, true);
            key(0x37, false);
            SetTimer(h, 1, 700, None);
        }
        _ => {
            let t = text_of();
            let ok = t.trim() == "7";
            logline(&format!("编辑框结果: [{t}]"));
            logline(&format!("Ctrl+A 是否生效: {ok}"));
            std::fs::write(OUT, LOG.as_bytes()).ok();
            DestroyWindow(h);
        }
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
        let cls = wide("PangyiShortcutTest");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(6isize),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = wide("Pangyi Shortcut Test");
        let h = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(cls.as_ptr()), PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE, 160, 160, 700, 380, None, None, None, None);
        if h.0 == 0 { logline("建窗失败"); std::fs::write(OUT, LOG.as_bytes()).ok(); return Ok(()); }
        MAIN = h;
        let ec = wide("RICHEDIT50W");
        let empty = wide("");
        EDIT = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(ec.as_ptr()), PCWSTR(empty.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL) as u32),
            0, 0, 660, 320, h, None, None, None);
        SetFocus(EDIT);
        SetTimer(h, 1, 900, None);
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    Ok(())
}