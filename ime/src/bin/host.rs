//! TSF 宿主：自己把「旁译」设为当前线程的输入法，再用 RichEdit 接收文字。
//! 完全不依赖 Win+Space 切换。
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
// 对照组：青简（已知可用）
const QJ_CLSID: GUID = GUID::from_u128(0x4FDCA82D_E923_49BF_9E75_BB906B93B8BB);
const QJ_PROFILE: GUID = GUID::from_u128(0x8119F8E0_CF81_423B_9189_C0D7374324B3);
static mut USE_QJ: bool = false;
const LANG: u16 = 0x0804;
const OUT: &str = r"C:\Users\33354\Documents\Codex\2026-10-08\github-yenharvey-qingjian\work\host-result.txt";

static mut EDIT: HWND = HWND(0);
static mut STEP: u32 = 0;
static mut LOG: String = String::new();
static mut KSM: Option<ITfKeystrokeMgr> = None;

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

fn logline(s: &str) {
    unsafe { LOG.push_str(s); LOG.push('\n'); }
}

unsafe fn send_key(vk: u8) {
    keybd_event(vk, 0, KEYEVENTF_KEYUP, 0);
    keybd_event(vk, 0, KEYEVENTF_KEYUP, 0);
}

unsafe fn do_step(h: HWND) {
    STEP += 1;
    match STEP {
        1 => {
            SetForegroundWindow(h);
            let fg = GetForegroundWindow();
            logline(&format!("抢前台: {} (hwnd={:?} fg={:?})", fg == h, h, fg));
            if fg != h { logline("!! 不是前台，按键会发错地方"); }
            logline("--- 初始化 TSF 并激活我们的 Profile ---");
            match CoCreateInstance::<_, ITfThreadMgr>(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) {
                Ok(tm) => {
                    match tm.Activate() { Ok(t) => logline(&format!("ITfThreadMgr::Activate ok clientId={t}")), Err(e) => logline(&format!("Activate 失败 {e:?}")) }
                    match tm.cast::<ITfKeystrokeMgr>() {
                        Ok(ks) => { KSM = Some(ks); logline("拿到 ITfKeystrokeMgr（走官方按键通道）"); }
                        Err(e) => logline(&format!("拿不到 ITfKeystrokeMgr: {e:?}")),
                    }
                }
                Err(e) => logline(&format!("拿不到 ITfThreadMgr: {e:?}")),
            }
            match CoCreateInstance::<_, ITfInputProcessorProfiles>(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) {
                Ok(p) => match p.ActivateLanguageProfile(
                    if USE_QJ { &QJ_CLSID } else { &OUR_CLSID }, LANG,
                    if USE_QJ { &QJ_PROFILE } else { &OUR_PROFILE }) {
                    Ok(()) => logline(if USE_QJ { "ActivateLanguageProfile(青简 对照组) 成功" } else { "ActivateLanguageProfile(旁译) 成功" }),
                    Err(e) => logline(&format!("ActivateLanguageProfile 失败 {e:?}")),
                },
                Err(e) => logline(&format!("拿不到 profiles: {e:?}")),
            }
            SetTimer(h, 1, 800, None);
        }
        2 => {
            SetForegroundWindow(h);
            logline(&format!("发送前确认前台: {} ", GetForegroundWindow() == h));
            logline("--- 通过 TestKeyDown/KeyDown 发送 kaifa ---");
            if let Some(ks) = KSM.as_ref() {
                for c in b"kaifa" {
                    let vk = WPARAM((*c - 32) as usize);
                    match ks.TestKeyDown(vk, LPARAM(0)) {
                        Ok(b) => {
                            logline(&format!("  TestKeyDown({}) -> eaten={}", *c as char, b.as_bool()));
                            if b.as_bool() { let _ = ks.KeyDown(vk, LPARAM(0)); }
                        }
                        Err(e) => logline(&format!("  TestKeyDown 失败 {e:?}")),
                    }
                }
            } else { logline("没有 ITfKeystrokeMgr"); }
            SetTimer(h, 1, 900, None);
        }
        3 => {
            logline("--- 发送空格 ---");
            if let Some(ks) = KSM.as_ref() {
                let vk = WPARAM(0x20);
                match ks.TestKeyDown(vk, LPARAM(0)) {
                    Ok(b) => { logline(&format!("  TestKeyDown(space) -> eaten={}", b.as_bool())); if b.as_bool() { let _ = ks.KeyDown(vk, LPARAM(0)); } }
                    Err(e) => logline(&format!("  TestKeyDown(space) 失败 {e:?}")),
                }
            }
            SetTimer(h, 1, 2000, None);
        }
        _ => {
            let mut buf = [0u16; 512];
            let n = SendMessageW(EDIT, WM_GETTEXT, WPARAM(buf.len()), LPARAM(buf.as_mut_ptr() as isize)).0 as usize;
            let txt = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
            logline(&format!("RichEdit 内容: [{txt}]"));
            logline(&format!("成功上屏: {}", txt.contains('开')));
            std::fs::write(OUT, LOG.as_bytes()).ok();
            DestroyWindow(h);
        }
    }
}

extern "system" fn wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER => { do_step(h); LRESULT(0) }
            WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }
            _ => DefWindowProcW(h, msg, w, l),
        }
    }
}

fn main() -> Result<()> {
    unsafe {
        USE_QJ = std::env::args().any(|a| a == "qingjian");
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let md = wide("Msftedit.dll");
        let _ = LoadLibraryW(PCWSTR(md.as_ptr()));

        let cls = wide("PangyiTsfHost");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(6isize),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = wide("Pangyi TSF Host");
        let h = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(cls.as_ptr()), PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE, 120, 120, 820, 460,
            None, None, None, None);
        if h.0 == 0 { logline("建窗失败"); std::fs::write(OUT, LOG.as_bytes()).ok(); return Ok(()); }

        let ec = wide("RICHEDIT50W");
        let empty = wide("");
        EDIT = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(ec.as_ptr()), PCWSTR(empty.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL) as u32),
            0, 0, 780, 400, h, None, None, None);
        SetFocus(EDIT);
        logline(&format!("窗口已建, hwnd={:?}, edit={:?}", h, EDIT));
        SetTimer(h, 1, 1500, None);

        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    Ok(())
}
