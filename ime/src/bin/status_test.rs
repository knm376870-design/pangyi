//! 状态条自检：显示 8 秒后退出（用于截图核对外观）。
#![windows_subsystem = "windows"]
#![allow(non_snake_case)]

fn main() {
    unsafe {
        pangyi_ime::status::init();
        pangyi_ime::status::show();
        let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < 8 {
            while windows::Win32::UI::WindowsAndMessaging::PeekMessageW(
                &mut msg, None, 0, 0,
                windows::Win32::UI::WindowsAndMessaging::PM_REMOVE).as_bool() {
                let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
                windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        pangyi_ime::status::hide();
    }
}