//! 旁译 输入法（独立软件版）
//!
//! 两种模式：
//!   伴随模式（默认）——你继续用系统输入法（微软拼音等）打字，旁译在旁边显示俄语译词，不抢键、不上屏。
//!   自带上屏模式 ——旁译自己把拼音换成中文上屏（需要把系统输入法切到英文）。
#![windows_subsystem = "windows"]
#![allow(non_snake_case)]

use std::sync::Mutex;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Default)]
struct State {
    buf: String,
    cands: Vec<(String, String)>,
    companion: bool,
}

static ST: Mutex<Option<State>> = Mutex::new(None);
static mut HWND_CAND: HWND = HWND(0);
static mut HOOK: HHOOK = HHOOK(0);
const WM_TRAY: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;
const TIMER_HIDE: usize = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn is_companion() -> bool {
    ST.lock().unwrap().as_ref().map(|s| s.companion).unwrap_or(true)
}

unsafe fn clear_buf() {
    let mut st = ST.lock().unwrap();
    if let Some(s) = st.as_mut() {
        s.buf.clear();
        s.cands.clear();
    }
}

unsafe fn arm_autohide(ms: u32) {
    if HWND_CAND.0 != 0 {
        let _ = SetTimer(HWND_CAND, TIMER_HIDE, ms, None);
    }
}

fn exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

unsafe fn autostart_enabled() -> bool {
    use windows::Win32::System::Registry::*;
    let sub: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0".encode_utf16().collect();
    let mut h = HKEY::default();
    if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), 0, KEY_QUERY_VALUE, &mut h).is_err() {
        return false;
    }
    let name: Vec<u16> = "Pangyi\0".encode_utf16().collect();
    let mut ty = REG_SZ;
    let mut buf = [0u8; 1024];
    let mut cb = buf.len() as u32;
    let ok = RegQueryValueExW(h, PCWSTR(name.as_ptr()), None, Some(&mut ty),
        Some(buf.as_mut_ptr()), Some(&mut cb)).is_ok();
    let _ = RegCloseKey(h);
    ok
}

unsafe fn set_autostart(on: bool) {
    use windows::Win32::System::Registry::*;
    let sub: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0".encode_utf16().collect();
    let mut h = HKEY::default();
    if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), 0, KEY_SET_VALUE, &mut h).is_err() {
        return;
    }
    let name: Vec<u16> = "Pangyi\0".encode_utf16().collect();
    if on {
        let v = format!("\"{}\"", exe_path());
        let val: Vec<u16> = v.encode_utf16().collect();
        let bytes = std::slice::from_raw_parts(val.as_ptr() as *const u8, val.len() * 2);
        let _ = RegSetValueExW(h, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(bytes));
    } else {
        let _ = RegDeleteValueW(h, PCWSTR(name.as_ptr()));
    }
    let _ = RegCloseKey(h);
}

unsafe fn show_cands() {
    let st = ST.lock().unwrap();
    let st = st.as_ref().unwrap();
    if st.cands.is_empty() || st.buf.is_empty() {
        pangyi_ime::cand::hide();
        return;
    }
    let mut gi = GUITHREADINFO::default();
    gi.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
    let (mut x, mut y) = (400, 400);
    if GetGUIThreadInfo(0, &mut gi).is_ok() {
        let mut pt = POINT::default();
        if GetCaretPos(&mut pt).is_ok() {
            ClientToScreen(gi.hwndCaret, &mut pt);
            x = pt.x;
            y = pt.y + if st.companion { 76 } else { 26 };
        }
    }
    pangyi_ime::cand::show_at(x, y);
}

unsafe fn tray_add(hwnd: HWND, first: bool) {
    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_ID;
    nid.uCallbackMessage = WM_TRAY;
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.hIcon = LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
    let tip: Vec<u16> = "旁译 · 中俄输入（跟随系统输入法显示俄语）".encode_utf16().collect();
    let n = tip.len().min(nid.szTip.len());
    nid.szTip[..n].copy_from_slice(&tip[..n]);
    let _ = Shell_NotifyIconW(if first { NIM_ADD } else { NIM_MODIFY }, &nid);

    if first {
        let mut ball = nid;
        ball.uFlags = NIF_INFO;
        ball.dwInfoFlags = NIIF_INFO;
        let title: Vec<u16> = "旁译 已启动".encode_utf16().collect();
        let m = title.len().min(ball.szInfoTitle.len());
        ball.szInfoTitle[..m].copy_from_slice(&title[..m]);
        let info: Vec<u16> = "照常用系统输入法打中文，旁译会在旁边显示俄语译词。右键托盘图标可切换模式或退出。".encode_utf16().collect();
        let k = info.len().min(ball.szInfo.len());
        ball.szInfo[..k].copy_from_slice(&info[..k]);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &ball);
    }
}

unsafe fn tray_remove(hwnd: HWND) {
    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_ID;
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

unsafe fn tray_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let comp = is_companion();
    let a = wide("跟随系统输入法（只显示俄语）");
    let b = wide("旁译自己上屏（中俄输入）");
    let c = wide("开机自动启动");
    let d = wide("退出旁译");
    let fa = if comp { MF_STRING | MF_CHECKED } else { MF_STRING };
    let fb = if comp { MF_STRING } else { MF_STRING | MF_CHECKED };
    let fc = if autostart_enabled() { MF_STRING | MF_CHECKED } else { MF_STRING };
    let _ = AppendMenuW(menu, fa, 1, PCWSTR(a.as_ptr()));
    let _ = AppendMenuW(menu, fb, 2, PCWSTR(b.as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, fc, 3, PCWSTR(c.as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, 4, PCWSTR(d.as_ptr()));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
    let _ = DestroyMenu(menu);
    match cmd.0 {
        1 => {
            if let Some(s) = ST.lock().unwrap().as_mut() { s.companion = true; }
            pangyi_ime::cand::hide();
        }
        2 => {
            if let Some(s) = ST.lock().unwrap().as_mut() { s.companion = false; }
            pangyi_ime::cand::hide();
        }
        3 => {
            let on = autostart_enabled();
            set_autostart(!on);
        }
        4 => {
            tray_remove(hwnd);
            let _ = DestroyWindow(hwnd);
        }
        _ => {}
    }
}

unsafe fn send_backspaces(n: usize) {
    let mut inputs: Vec<INPUT> = Vec::new();
    for _ in 0..n {
        for up in [false, true] {
            let mut i = INPUT::default();
            i.r#type = INPUT_KEYBOARD;
            i.Anonymous.ki = KEYBDINPUT {
                wVk: VK_BACK,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            };
            inputs.push(i);
        }
    }
    if !inputs.is_empty() {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
    std::thread::sleep(std::time::Duration::from_millis(60));
}

unsafe fn send_text(s: &str) {
    let mut inputs: Vec<INPUT> = Vec::new();
    for u in s.encode_utf16() {
        for up in [false, true] {
            let mut i = INPUT::default();
            i.r#type = INPUT_KEYBOARD;
            i.Anonymous.ki = KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: u,
                dwFlags: if up { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP } else { KEYEVENTF_UNICODE },
                time: 0,
                dwExtraInfo: 0,
            };
            inputs.push(i);
        }
    }
    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    std::thread::sleep(std::time::Duration::from_millis(80));
}

unsafe fn commit(idx: usize) {
    let (word, plen) = {
        let st = ST.lock().unwrap();
        let st = st.as_ref().unwrap();
        if st.buf.is_empty() || idx >= st.cands.len() {
            return;
        }
        (st.cands[idx].0.clone(), st.buf.chars().count())
    };
    send_backspaces(plen);
    send_text(&word);
    clear_buf();
    pangyi_ime::cand::hide();
}

unsafe fn refresh() {
    {
        let mut guard = ST.lock().unwrap();
        let st = guard.as_mut().unwrap();
        if st.buf.is_empty() {
            st.cands.clear();
        } else {
            let hits = pangyi_ime::dict::lookup(&st.buf);
            let items: Vec<pangyi_ime::cand::Item> = hits
                .iter()
                .map(|h| pangyi_ime::cand::Item {
                    word: h.word.clone(),
                    pinyin: h.pinyin.clone(),
                    ru: h.ru.clone(),
                    fresh: false,
                })
                .collect();
            pangyi_ime::cand::set_items(items);
            st.cands = hits.into_iter().map(|h| (h.word, h.ru)).collect();
        }
    }
    show_cands();
}

unsafe extern "system" fn hook_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(None, code, w, l);
    }
    let kb = &*(l.0 as *const KBDLLHOOKSTRUCT);
    let down = w.0 == WM_KEYDOWN as usize || w.0 == WM_SYSKEYDOWN as usize;
    if !down {
        return CallNextHookEx(None, code, w, l);
    }

    let vk = kb.vkCode as u8;
    let ctrl = (kb.flags.0 & 0x08u32) != 0;
    let alt = (kb.flags.0 & 0x20u32) != 0;
    let companion = is_companion();

    if ctrl || alt {
        clear_buf();
        pangyi_ime::cand::hide();
        return CallNextHookEx(None, code, w, l);
    }

    let is_letter = (0x41..=0x5A).contains(&vk);
    let is_digit = (0x31..=0x39).contains(&vk);
    let has = {
        ST.lock()
            .unwrap()
            .as_ref()
            .map(|s| !s.buf.is_empty())
            .unwrap_or(false)
    };

    if is_letter {
        let ch = (vk as char).to_ascii_lowercase();
        {
            ST.lock().unwrap().as_mut().unwrap().buf.push(ch);
        }
        refresh();
        arm_autohide(4000);
        return CallNextHookEx(None, code, w, l);
    }

    if has && vk == 0x08 {
        {
            ST.lock().unwrap().as_mut().unwrap().buf.pop();
        }
        let empty = {
            ST.lock()
                .unwrap()
                .as_ref()
                .map(|s| s.buf.is_empty())
                .unwrap_or(true)
        };
        if empty {
            pangyi_ime::cand::hide();
        } else {
            refresh();
            arm_autohide(4000);
        }
        return CallNextHookEx(None, code, w, l);
    }

    if has && vk == 0x1B {
        clear_buf();
        pangyi_ime::cand::hide();
        if companion {
            return CallNextHookEx(None, code, w, l);
        }
        return LRESULT(1);
    }

    if has && (vk == 0x20 || is_digit) {
        if companion {
            clear_buf();
            arm_autohide(2600);
            return CallNextHookEx(None, code, w, l);
        }
        let idx = if is_digit { (vk - 0x31) as usize } else { 0 };
        commit(idx);
        return LRESULT(1);
    }

    if has && (vk == 0x0D || vk == 0x09) {
        clear_buf();
        if companion {
            arm_autohide(1200);
        } else {
            pangyi_ime::cand::hide();
        }
    }
    CallNextHookEx(None, code, w, l)
}

unsafe extern "system" fn cand_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            if l.0 as u32 == WM_RBUTTONUP {
                tray_menu(h);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if w.0 == TIMER_HIDE {
                let _ = KillTimer(h, TIMER_HIDE);
                pangyi_ime::cand::hide();
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

fn main() {
    unsafe {
        let mut st = State::default();
        st.companion = true;
        *ST.lock().unwrap() = Some(st);
        pangyi_ime::cand::init();

        let hinst = GetModuleHandleW(None).unwrap();
        let cls = wide("PangyiTray");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(cand_proc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hInstance: hinst.into(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        HWND_CAND = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            PCWSTR(cls.as_ptr()),
            w!("旁译"),
            WS_POPUP,
            0, 0, 0, 0,
            None, None, hinst, None);

        tray_add(HWND_CAND, true);
        HOOK = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), hinst, 0).unwrap();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(HOOK);
    }
}