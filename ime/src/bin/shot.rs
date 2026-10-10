//! 截图台架：把「旁译」设为当前线程输入法 → 经官方按键通道打 pengyou → 窗口挂 30 秒，
//! 供外部对屏幕截候选窗（README 用图）。按键全走 ITfKeystrokeMgr，不发 SendInput，
//! 不会漏到用户正在用的应用里。
#![allow(non_snake_case)]
use windows::core::{ComInterface, GUID, PCWSTR, Result};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};

// windows 0.52 的 HiDpi 模块在本机 target 被裁掉，直接自声明（user32 已链接）
unsafe extern "system" { fn SetProcessDPIAware() -> BOOL; }

const OUR_CLSID: GUID = GUID::from_u128(0xB4E8F2A1_7C3D_4E5F_9A21_8D6C4B3E1F07);
const OUR_PROFILE: GUID = GUID::from_u128(0x5A7C9E12_3D4B_4F8A_B6E1_2C9D7A5E8F34);
const LANG: u16 = 0x0804;
const LOGFILE: &str = r"C:\Users\33354\AppData\Local\Temp\shot-log.txt";

static mut EDIT: HWND = HWND(0);
static mut STEP: u32 = 0;
static mut KSM: Option<ITfKeystrokeMgr> = None;
static mut LOG: String = String::new();

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

fn logline(s: &str) {
    unsafe { LOG.push_str(s); LOG.push('\n'); }
    std::fs::write(LOGFILE, unsafe { LOG.as_bytes() }).ok();
}

unsafe fn send_ks(ks: &ITfKeystrokeMgr, c: u8) {
    let vk = WPARAM((c - 32) as usize);
    match ks.TestKeyDown(vk, LPARAM(0)) {
        Ok(b) => {
            logline(&format!("  TestKeyDown({}) -> eaten={}", c as char, b.as_bool()));
            if b.as_bool() { let _ = ks.KeyDown(vk, LPARAM(0)); }
        }
        Err(e) => logline(&format!("  TestKeyDown 失败 {e:?}")),
    }
}

unsafe fn focus_ours(h: HWND) {
    let fg = GetForegroundWindow();
    let fg_tid = GetWindowThreadProcessId(fg, None);
    let me = GetCurrentThreadId();
    let _ = AttachThreadInput(me, fg_tid, true);
    let _ = SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
    let _ = SetForegroundWindow(h);
    let _ = BringWindowToTop(h);
    let _ = SetFocus(EDIT);
    let _ = AttachThreadInput(me, fg_tid, false);
}

unsafe fn do_step(h: HWND) {
    STEP += 1;
    match STEP {
        1 => {
            for i in 0..3 {
                focus_ours(h);
                if GetForegroundWindow() == h { break; }
                logline(&format!("第 {} 次抢前台失败", i + 1));
                std::thread::sleep(std::time::Duration::from_millis(80));
            }
            logline(&format!("前台: {} (hwnd={:?} fg={:?})", GetForegroundWindow() == h, h, GetForegroundWindow()));
            if let Ok(tm) = CoCreateInstance::<_, ITfThreadMgr>(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) {
                match tm.Activate() { Ok(t) => logline(&format!("ThreadMgr Activate ok clientId={t}")), Err(e) => logline(&format!("Activate 失败 {e:?}")) }
                if let Ok(ks) = tm.cast::<ITfKeystrokeMgr>() { KSM = Some(ks); logline("拿到 KeystrokeMgr"); }
                // 不靠系统前台：重新 SetFocus 触发 RichEdit 自己连 TSF 并接管文档焦点
                let _ = SetFocus(EDIT);
            } else { logline("拿不到 ThreadMgr"); }
            if let Ok(p) = CoCreateInstance::<_, ITfInputProcessorProfiles>(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) {
                match p.ActivateLanguageProfile(&OUR_CLSID, LANG, &OUR_PROFILE) {
                    Ok(()) => logline("ActivateLanguageProfile(旁译) 成功"),
                    Err(e) => logline(&format!("ActivateLanguageProfile 失败 {e:?}")),
                }
            } else { logline("拿不到 profiles"); }
            SetTimer(h, 1, 1200, None);
        }
        2 => {
            if let Some(ks) = KSM.as_ref() {
                for c in b"pengyou" { send_ks(ks, *c); }
            } else { logline("没有 KeystrokeMgr"); }
            SetTimer(h, 1, 30000, None); // 挂 30 秒给外部截屏
        }
        _ => {
            let mut buf = [0u16; 512];
            let n = SendMessageW(EDIT, WM_GETTEXT, WPARAM(buf.len()), LPARAM(buf.as_mut_ptr() as isize)).0 as usize;
            let txt = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
            logline(&format!("RichEdit 内容: [{txt}]"));
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
        // 与截屏脚本同用物理像素坐标
        let _ = SetProcessDPIAware();
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _ = LoadLibraryW(PCWSTR(wide("Msftedit.dll").as_ptr()));

        let cls = wide("PangyiShotHost");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(6isize),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = wide("旁译 演示 — 打 pengyou");
        let h = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(cls.as_ptr()), PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE, 150, 120, 900, 520,
            None, None, None, None);
        if h.0 == 0 { return Ok(()); }

        let ec = wide("RICHEDIT50W");
        let empty = wide("");
        EDIT = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(ec.as_ptr()), PCWSTR(empty.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL) as u32),
            0, 0, 884, 481, h, None, None, None);
        SetFocus(EDIT);
        SetTimer(h, 1, 1500, None);

        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    Ok(())
}
