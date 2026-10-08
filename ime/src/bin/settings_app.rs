//! 旁译 设置（独立程序）：打开带标题栏的设置窗口，关掉即退出。
#![windows_subsystem = "windows"]
#![allow(non_snake_case)]

fn main() {
    unsafe {
        pangyi_ime::settings::load();
        pangyi_ime::settings::open_app();
        let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        while windows::Win32::UI::WindowsAndMessaging::GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }
    }
}