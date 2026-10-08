//! 旁译 输入法（独立软件版）
//! 全局键盘钩子 + 自绘候选窗。不吞字母（拼音照常显示），选中后退格替换成中文。
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
    cands: Vec<(String, String)>, // (中文, 俄语)
}

static ST: Mutex<Option<State>> = Mutex::new(None);
static mut HWND_CAND: HWND = HWND(0);
static mut HOOK: HHOOK = HHOOK(0);
const WM_TRAY: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

unsafe fn show_cands() {
    let st = ST.lock().unwrap();
    let st = st.as_ref().unwrap();
    let h = HWND_CAND;
    if st.cands.is_empty() {
        pangyi_ime::cand::hide();
        return;
    }
    if st.buf.is_empty() { return; }
    // 定位到光标附近
    let mut gi = GUITHREADINFO::default();
    gi.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
    let (mut x, mut y) = (400, 400);
    if GetGUIThreadInfo(0, &mut gi).is_ok() {
        let mut pt = POINT::default();
        if GetCaretPos(&mut pt).is_ok() {
            ClientToScreen(gi.hwndCaret, &mut pt);
            x = pt.x; y = pt.y + 24;
        }
    }
    let _ = h;
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
    let tip: Vec<u16> = "旁译 · 中俄输入（敲拼音，空格上屏）".encode_utf16().collect();
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
        let info: Vec<u16> = "在任何软件里敲拼音，候选窗会出现中文和俄语译词，空格上屏。托盘图标右键可退出。".encode_utf16().collect();
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
    let s: Vec<u16> = "退出旁译".encode_utf16().chain(std::iter::once(0)).collect();
    let _ = AppendMenuW(menu, MF_STRING, 1, PCWSTR(s.as_ptr()));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
    let _ = DestroyMenu(menu);
    if cmd.0 == 1 {
        tray_remove(hwnd);
        let _ = DestroyWindow(hwnd);
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
        if st.buf.is_empty() || idx >= st.cands.len() { return; }
        (st.cands[idx].0.clone(), st.buf.chars().count())
    };
    send_backspaces(plen);
    send_text(&word);
    let mut st = ST.lock().unwrap();
    if let Some(s) = st.as_mut() { s.buf.clear(); s.cands.clear(); }
    ShowWindow(HWND_CAND, SW_HIDE);
}

unsafe fn refresh() {
    let (_pinyin, _cands) = {
        let mut guard = ST.lock().unwrap();
        let st = guard.as_mut().unwrap();
        if st.buf.is_empty() {
            st.cands.clear();
            (String::new(), Vec::new())
        } else {
            let hits = pangyi_ime::dict::lookup(&st.buf);
            let items: Vec<pangyi_ime::cand::Item> = hits.iter().map(|h| pangyi_ime::cand::Item {
                word: h.word.clone(), pinyin: h.pinyin.clone(), ru: h.ru.clone() }).collect();
            pangyi_ime::cand::set_items(items);
            let v: Vec<(String, String)> = hits.into_iter().map(|h| (h.word, h.ru)).collect();
            st.cands = v.clone();
            (st.buf.clone(), v)
        }
    };
    show_cands();
}

unsafe extern "system" fn hook_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code < 0 { return CallNextHookEx(None, code, w, l); }
    let kb = &*(l.0 as *const KBDLLHOOKSTRUCT);
    let down = w.0 == WM_KEYDOWN as usize || w.0 == WM_SYSKEYDOWN as usize;
    if !down { return CallNextHookEx(None, code, w, l); }

    let vk = kb.vkCode as u8;
    // 有 Ctrl/Alt/Win 时放行（保留快捷键）
    let ctrl = (kb.flags.0 & 0x08u32) != 0;
    if ctrl || (kb.flags.0 & 0x20u32) != 0 { 
        let mut st = ST.lock().unwrap();
        if let Some(s) = st.as_mut() { s.buf.clear(); s.cands.clear(); }
        ShowWindow(HWND_CAND, SW_HIDE);
        return CallNextHookEx(None, code, w, l);
    }

    let is_letter = (0x41..=0x5A).contains(&vk);
    let is_digit = (0x31..=0x39).contains(&vk);
    let has = { ST.lock().unwrap().as_ref().map(|s| !s.buf.is_empty()).unwrap_or(false) };

    if is_letter {
        let ch = (vk as char).to_ascii_lowercase();
        { ST.lock().unwrap().as_mut().unwrap().buf.push(ch); }
        refresh();
        return CallNextHookEx(None, code, w, l);   // 放行：拼音照常显示
    }
    if has && (vk == 0x20 || is_digit) {
        let idx = if is_digit { (vk - 0x31) as usize } else { 0 };
        commit(idx);
        return LRESULT(1);                          // 吞掉空格/数字
    }
    if has && vk == 0x1B {
        let mut st = ST.lock().unwrap();
        if let Some(s) = st.as_mut() { s.buf.clear(); s.cands.clear(); }
        ShowWindow(HWND_CAND, SW_HIDE);
        return LRESULT(1);
    }
    if has && vk == 0x08 {
        let mut guard = ST.lock().unwrap();
        let st = guard.as_mut().unwrap();
        st.buf.pop();
        drop(guard);
        refresh();
        return CallNextHookEx(None, code, w, l);
    }
    if vk == 0x0D || vk == 0x09 {  // Enter/Tab 结束
        let mut st = ST.lock().unwrap();
        if let Some(s) = st.as_mut() { s.buf.clear(); s.cands.clear(); }
        ShowWindow(HWND_CAND, SW_HIDE);
    }
    CallNextHookEx(None, code, w, l)
}

unsafe extern "system" fn cand_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let mut rc = RECT::default();
            GetClientRect(h, &mut rc);
            let bg = CreateSolidBrush(COLORREF(0x002a3245));
            FillRect(dc, &rc, bg);
            DeleteObject(bg);
            SetBkMode(dc, TRANSPARENT);
            let font = CreateFontW(-18, 0, 0, 0, 500, 0, 0, 0, 134, 0, 0, 0, 0,
                w!("Microsoft YaHei UI"));
            let old = SelectObject(dc, font);
            let st = ST.lock().unwrap();
            if let Some(s) = st.as_ref() {
                for (i, (word, ru)) in s.cands.iter().take(9).enumerate() {
                    let y = 4 + (i as i32) * 30;
                    SetTextColor(dc, COLORREF(0x00f7a27a));
                    let idx = wide(&format!("{}", i + 1));
                    TextOutW(dc, 10, y, &idx[..idx.len() - 1]);
                    SetTextColor(dc, COLORREF(0x00f3ece9));
                    let ww = wide(word);
                    TextOutW(dc, 40, y, &ww[..ww.len() - 1]);
                    SetTextColor(dc, COLORREF(0x00ffa6c3));
                    let rr = wide(&ru.replace('|', "·"));
                    TextOutW(dc, 150, y, &rr[..rr.len() - 1]);
                }
            }
            SelectObject(dc, old);
            DeleteObject(font);
            EndPaint(h, &ps);
            LRESULT(0)
        }
        WM_TRAY => { if l.0 as u32 == WM_RBUTTONUP { tray_menu(h); } LRESULT(0) }
        WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

fn main() {
    unsafe {
        *ST.lock().unwrap() = Some(State::default());\n        pangyi_ime::cand::init();

        let hinst = GetModuleHandleW(None).unwrap();
        let cls = wide("PangyiCand");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(cand_proc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hInstance: hinst.into(),
            hbrBackground: HBRUSH(6isize),
            ..Default::default()
        };
        RegisterClassW(&wc);
        HWND_CAND = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            PCWSTR(cls.as_ptr()), w!("旁译"), WS_POPUP,
            0, 0, 560, 300, None, None, hinst, None);

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
