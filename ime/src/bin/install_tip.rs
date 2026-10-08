//! 用 InstallLayoutOrTip 给当前用户装输入法（动态加载 input.dll）
use windows::core::{PCSTR, PCWSTR};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

fn main() {
    let tip = "0804:{B4E8F2A1-7C3D-4E5F-9A21-8D6C4B3E1F07}{5A7C9E12-3D4B-4F8A-B6E1-2C9D7A5E8F34}";
    let w = wide(tip);
    let dll = wide("input.dll");
    unsafe {
        let Ok(h) = LoadLibraryW(PCWSTR(dll.as_ptr())) else { println!("load input.dll 失败"); return };
        println!("input.dll 已加载");
        let a = b"InstallLayoutOrTip\0";
        if let Some(p) = GetProcAddress(h, PCSTR(a.as_ptr())) {
            let f: unsafe extern "system" fn(*const u16, u32) -> i32 = std::mem::transmute(p);
            println!("InstallLayoutOrTip(\"{tip}\") -> {}  (1=成功)", f(w.as_ptr(), 0));
        } else { println!("找不到 InstallLayoutOrTip"); }
        let b = b"InstallLayoutOrTipUserReg\0";
        if let Some(p) = GetProcAddress(h, PCSTR(b.as_ptr())) {
            let f: unsafe extern "system" fn(*const u16,*const u16,*const u16,u32) -> i32 = std::mem::transmute(p);
            println!("InstallLayoutOrTipUserReg -> {}  (1=成功)",
                f(std::ptr::null(), std::ptr::null(), w.as_ptr(), 0));
        } else { println!("找不到 InstallLayoutOrTipUserReg"); }
    }
}
