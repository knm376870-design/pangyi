//! v4.6.0 设置面板美化后的点击回归验证。
//!
//! 不碰 TSF：直接预载新 DLL（拿导出函数），在进程里开真实面板窗口，
//! 用 SendMessageW 同步投 WM_LBUTTONDOWN 到 layout() 算出的药丸矩形中心，
//! 断言 settings.txt 里对应配置真的变了 —— 专防「画的是一块、点的是另一块」
//! （v4.5.0 的教训）和「点药丸把窗口点关了」这类回归。
//! v4.10.0 加「配色」行后补 T6（点「粉」→ accent=1，点回「青」收尾）。
//!
//! 用例：
//!   T1 点「5」        -> max=5
//!   T2 点「横排」      -> layout=h
//!   T3 点「深色」      -> theme=2（顺带跑一遍深色重绘路径）
//!   T4 点「整句」      -> trmode=2
//!   T5 点「标点 关」    -> cnpunct=0
//!   T6 点「粉」       -> accent=1（v4.10.0 配色行；点完点回「青」收尾）
//!   T7 点「关闭」      -> 窗口真关（IsWindow=false）
//!
//! 用法：settings_e2e.exe [新DLL路径]（LOCALAPPDATA 重定向由启动器环境做）
#![allow(non_snake_case)]
use windows::core::{s, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::*;

const OUT: &str =
    r"C:\Users\33354\Documents\Codex\2026-10-08\github-yenharvey-qingjian\work\settings-e2e.txt";

static mut LOG: String = String::new();
static mut PASS: u32 = 0;
static mut FAIL: u32 = 0;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
unsafe fn logline(s: &str) {
    LOG.push_str(s);
    LOG.push('\n');
    std::fs::write(OUT, LOG.as_bytes()).ok();
}

/// 消息泵：面板窗口过程在本线程，PostMessage 的点击要泵才会被处理
unsafe fn pump(ms: u32) {
    let start = std::time::Instant::now();
    let mut m = MSG::default();
    while start.elapsed().as_millis() < ms as u128 {
        while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

unsafe fn settings_text() -> String {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_default();
    std::fs::read_to_string(format!("{base}\\Pangyi\\settings.txt")).unwrap_or_default()
}

unsafe fn check(tag: &str, expect: &str) {
    let s = settings_text();
    let ok = s.contains(expect);
    logline(&format!("{tag}: 期望 settings.txt 含「{expect}」  {} ", if ok { "✓" } else { "✗ 不符合!" }));
    if !ok {
        logline(&format!("  文件实际内容: {}", s.replace('\n', " | ")));
    }
    if ok { PASS += 1; } else { FAIL += 1; }
}

fn main() {
    unsafe {
        // 种子配置（和 launch 环境重定向的 LOCALAPPDATA 配合）
        let base = std::env::var("LOCALAPPDATA").unwrap_or_default();
        let dir = format!("{base}\\Pangyi");
        let _ = std::fs::create_dir_all(&dir);
        let cfg = "max=9\nscale=100\nlayout=v\ntheme=0\nenglish=0\ndebug=0\ncnpunct=1\ntrmode=0\naccent=0\n";
        let _ = std::fs::write(format!("{dir}\\settings.txt"), cfg);

        let dll = std::env::args().nth(1).unwrap_or_default();
        logline(&format!("预载 DLL: {dll}"));
        let w = wide(&dll);
        let h = match LoadLibraryW(PCWSTR(w.as_ptr())) {
            Ok(h) => h,
            Err(e) => {
                logline(&format!("预载新 DLL 失败: {e:?}"));
                std::process::exit(1);
            }
        };
        type OpenFn = unsafe extern "system" fn() -> isize;
        type RectFn = unsafe extern "system" fn(u32, *mut i32) -> u32;
        type HwndFn = unsafe extern "system" fn() -> isize;
        let Some(fo) = GetProcAddress(h, s!("settings_open_export")) else {
            logline("DLL 没有导出 settings_open_export");
            std::process::exit(1);
        };
        let Some(fr) = GetProcAddress(h, s!("settings_action_rect_export")) else {
            logline("DLL 没有导出 settings_action_rect_export");
            std::process::exit(1);
        };
        let open: OpenFn = std::mem::transmute(fo);
        let get_rect: RectFn = std::mem::transmute(fr);

        let hwnd = HWND(open());
        if hwnd.0 == 0 {
            let err = GetLastError();
            logline(&format!("面板窗口没开出来（句柄 0）GetLastError={err:?}"));
            // DLL 内部 WND 交叉核对：open 返回 0 到底是没建成还是别的原因
            if let Some(fh) = GetProcAddress(h, s!("settings_hwnd_export")) {
                let gh: HwndFn = std::mem::transmute(fh);
                logline(&format!("DLL 内 WND={}", gh()));
            }
            std::process::exit(1);
        }
        logline(&format!("面板已开 hwnd={}", hwnd.0));
        pump(300);

        // 点击：layout() 给矩形 → 中心点 → 真 WM_LBUTTONDOWN
        macro_rules! click {
            ($act:expr) => {{
                let mut r = [0i32; 4];
                if get_rect($act, r.as_mut_ptr()) == 0 {
                    logline(&format!("settings_action_rect_export(act={}) 失败", $act));
                    std::process::exit(1);
                }
                let (l, t, rr, b) = (r[0], r[1], r[2], r[3]);
                let x = (l + rr) / 2;
                let y = (t + b) / 2;
                let lp = ((y as isize) << 16) | (x as isize & 0xFFFF);
                logline(&format!("点击 药丸 act={} @({x},{y}) rect=({l},{t},{rr},{b})", $act));
                // 同步发送：直达窗口过程、立即返回结果，不依赖队列泵
                let ret = SendMessageW(hwnd, WM_LBUTTONDOWN, WPARAM(1), LPARAM(lp));
                logline(&format!("  SendMessageW 返回 {}", ret.0));
                pump(200);
            }};
        }

        logline("=== T1 点「5」: 候选个数=5 ===");
        click!(1);
        check("T1", "max=5");

        logline("=== T2 点「横排」: 排版=横 ===");
        click!(8);
        check("T2", "layout=h");

        logline("=== T3 点「深色」: 主题=深色（顺带跑深色重绘）===");
        click!(11);
        check("T3", "theme=2");

        logline("=== T4 点「整句」: 翻译模式=整句 ===");
        click!(17);
        check("T4", "trmode=2");

        logline("=== T5 点「标点 关」: 中文标点=关 ===");
        click!(14);
        check("T5", "cnpunct=0");

        logline("=== T6 点「粉」: 配色=粉（accent=1）===");
        click!(19);
        check("T6", "accent=1");
        logline("  点回「青」收尾（accent=0，老用户默认色）");
        click!(18);

        logline("=== T7 点「关闭」: 窗口真关 ===");
        click!(0);
        let gone = IsWindow(hwnd).as_bool() == false;
        logline(&format!("T7: IsWindow={}  {} ", !gone, if gone { "✓" } else { "✗ 窗口还活着!" }));
        if gone { PASS += 1; } else { FAIL += 1; }

        logline(&format!("=== 完：{PASS} 过 / {FAIL} 不过 ==="));
        std::process::exit(if FAIL == 0 { 0 } else { 1 });
    }
}
