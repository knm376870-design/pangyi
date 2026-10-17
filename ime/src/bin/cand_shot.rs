//! 候选窗截图台架（v4.7.0）：不走 TSF —— 直接 rlib 调用 cand::init/set_items/show_at，
//! 窗口挂 30 秒给外部截屏（cap-top.ps1 抓图 + mapc.ps1 ASCII 色图验收新卡片风）。
//! 主题/布局由重定向后的 LOCALAPPDATA\Pangyi\settings.txt 控制（theme=1 浅 / 2 深）。
//! argv[1] = "ru" 时选中条挪到俄语侧（验收两侧选中条）；默认中文侧。
//! argv[2] = 悬停行号（默认 1；横排变体传 0 —— 横排只有一行）。
//! 全程不发任何真实按键（悬停用 PostMessageW 发给自己的窗口），不打扰用户。
#![allow(non_snake_case)]
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// windows 0.52 的 HiDpi 模块在本机 target 被裁掉，直接自声明（user32 已链接）
unsafe extern "system" { fn SetProcessDPIAware() -> BOOL; }

fn main() {
    unsafe {
        let _ = SetProcessDPIAware();
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        pangyi_ime::cand::init();
        pangyi_ime::cand::set_preedit("pengyou");
        pangyi_ime::cand::set_sentence(Some("朋友 一起 学习".to_string()));
        if std::env::args().nth(1).as_deref() == Some("nogloss") {
            // v4.8.0 无释义条目变体：选中第 0 条（岳不群无俄语释义），
            // 横排下第二行应为空（去拼音后没内容可画，不能留一行空白）
            pangyi_ime::cand::set_items(vec![
                pangyi_ime::cand::Item { word: "岳不群".into(), ru: "".into(), fresh: false },
                pangyi_ime::cand::Item { word: "苹果".into(), ru: "яблоко".into(), fresh: false },
            ]);
        } else {
            pangyi_ime::cand::set_items(vec![
                pangyi_ime::cand::Item { word: "朋友".into(), ru: "друг · товарищ".into(), fresh: false },
                pangyi_ime::cand::Item { word: "苹果".into(), ru: "яблоко".into(), fresh: false },
                pangyi_ime::cand::Item { word: "便宜".into(), ru: "дешёвый · выгодный".into(), fresh: true },
            ]);
        }
        pangyi_ime::cand::set_page(1, 5);
        if std::env::args().nth(1).as_deref() == Some("ru") {
            // 验收俄语侧选中条（←→ 后的样子）
            pangyi_ime::cand::set_ru_sel(true);
        }
        pangyi_ime::cand::show_at(600, 400);
        // 模拟鼠标悬停：竖排悬停 hrow 行中文侧；横排所有条目共一行，
        // 用跨度导出悬停第 1 条（第 0 条就是选中条，悬停条被压住看不到）
        let hrow: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(1);
        let ch = HWND(pangyi_ime::cand::cand_hwnd_export());
        let mut out = [0i32; 4];
        let (mut cx, mut cy) = (0, 0);
        let mut got = false;
        if pangyi_ime::cand::cand_span_export(1, out.as_mut_ptr()) != 0 {
            // 横排：第 1 条跨度中心 + 首行 y（行高矩形由 hit_rect 行 0 提供）
            cx = (out[0] + out[1]) / 2;
            if pangyi_ime::cand::cand_hit_rect_export(0, 0, out.as_mut_ptr()) != 0 {
                cy = (out[1] + out[3]) / 2;
                got = true;
            }
        } else if pangyi_ime::cand::cand_hit_rect_export(hrow, 0, out.as_mut_ptr()) != 0 {
            cx = (out[0] + out[2]) / 2;
            cy = (out[1] + out[3]) / 2;
            got = true;
        }
        if got {
            let lp = ((cy << 16) | (cx & 0xFFFF)) as isize;
            let _ = PostMessageW(ch, WM_MOUSEMOVE, WPARAM(0), LPARAM(lp));
        }
        // 挂 30 秒：泵消息让窗口正常重绘，供外部截屏
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < 30 {
            let mut m = MSG::default();
            if PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&m);
                let _ = DispatchMessageW(&m);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        pangyi_ime::cand::hide();
    }
}
