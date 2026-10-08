//! 设置面板自检：显示 12 秒后退出（用于截图核对）。
#![windows_subsystem = "windows"]
#![allow(non_snake_case)]

fn main() {
    unsafe {
        pangyi_ime::settings::load();
        pangyi_ime::settings::toggle();
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