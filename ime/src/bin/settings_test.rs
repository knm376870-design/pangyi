//! 设置面板自检：显示 12 秒后退出（用于截图核对）。
//! 可选参数 --theme light|dark：先写种子配置（theme=1/2）再开面板，截图确定性。
#![windows_subsystem = "windows"]
#![allow(non_snake_case)]

fn main() {
    // --theme light|dark：种子配置让主题确定（截图核对深浅两套色板）
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--theme") {
        let theme = match args.get(pos + 1).map(|s| s.as_str()) {
            Some("light") => 1,
            Some("dark") => 2,
            _ => 0,
        };
        let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
        let dir = format!("{base}\\Pangyi");
        let _ = std::fs::create_dir_all(&dir);
        let cfg = format!(
            "max=9\nscale=100\nlayout=v\ntheme={theme}\nenglish=0\ndebug=0\ncnpunct=1\ntrmode=0\n"
        );
        let _ = std::fs::write(format!("{dir}\\settings.txt"), cfg);
    }
    unsafe {
        pangyi_ime::settings::load();
        // toggle() 早就没了，开设置面板现在是 open_app()（2026-10-10，v4.2.0 顺手修）
        pangyi_ime::settings::open_app();
        let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < 12 {
            while windows::Win32::UI::WindowsAndMessaging::PeekMessageW(
                &mut msg, None, 0, 0,
                windows::Win32::UI::WindowsAndMessaging::PM_REMOVE).as_bool() {
                let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
                windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
}
