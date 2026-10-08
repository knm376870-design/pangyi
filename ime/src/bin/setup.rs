//! 旁译 安装程序 / 卸载程序（单个 exe，自带组件）
//!
//! 双击 = 安装（自动请求管理员权限）；`pangyi-setup.exe --uninstall` = 卸载。
//! 装完会出现在「设置 → 应用」里。
#![windows_subsystem = "windows"]
#![allow(non_snake_case)]

use std::ffi::c_void;
use std::path::Path;
use windows::core::{PCWSTR, GUID};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

// 组件直接打进安装程序里（构建前由脚本放到 ime/payload/）
const DLL64: &[u8] = include_bytes!("../../payload/pangyi_ime.dll");
const DLL32: &[u8] = include_bytes!("../../payload/pangyi_ime-x86.dll");
const ICO: &[u8] = include_bytes!("../../payload/pangyi.ico");
const SETTINGS64: &[u8] = include_bytes!("../../payload/pangyi-settings.exe");

const DIR: &str = r"C:\Program Files\Pangyi";
const DLL64_NAME: &str = "pangyi_ime.dll";
const DLL32_NAME: &str = "pangyi_ime-x86.dll";
const ICO_NAME: &str = "pangyi.ico";
const SETTINGS_NAME: &str = "pangyi-settings.exe";
const CLSID_STR: &str = "{B4E8F2A1-7C3D-4E5F-9A21-8D6C4B3E1F07}";
const PROFILE_STR: &str = "{5A7C9E12-3D4B-4F8A-B6E1-2C9D7A5E8F34}";
const CLSID: GUID = GUID::from_u128(0xB4E8F2A1_7C3D_4E5F_9A21_8D6C4B3E1F07);

const CATS: [&str; 8] = [
    "{046B8C80-1647-40F7-9B21-B93B81AABC1B}",
    "{13A016DF-560B-46CD-947A-4C3AF1E0E35D}",
    "{25504FB4-7BAB-4BC1-9C69-CF81890F0EF5}",
    "{34745C63-B2F0-4784-8B67-5E12C8701A31}",
    "{364215D9-75BC-11D7-A6EF-00065B84435C}",
    "{49D2F9CE-1F5E-11D7-A6D3-00065B84435C}",
    "{49D2F9CF-1F5E-11D7-A6D3-00065B84435C}",
    "{CCF05DD7-4A87-11D7-A6E2-00065B84435C}",
];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn msg(title: &str, text: &str) {
    unsafe {
        let t = wide(text);
        let c = wide(title);
        let _ = MessageBoxW(HWND(0), PCWSTR(t.as_ptr()), PCWSTR(c.as_ptr()), MB_OK | MB_ICONINFORMATION);
    }
}

unsafe fn elevated() -> bool {
    // 试着往 Program Files 里写一下：写不进去就说明权限不够
    let probe = format!("{DIR}\\.writetest");
    if std::fs::create_dir_all(DIR).is_err() {
        return false;
    }
    match std::fs::write(&probe, b"x") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

unsafe fn relaunch_elevated(uninstall: bool) {
    let exe = std::env::current_exe().unwrap_or_default();
    let exe_w = wide(&exe.to_string_lossy());
    let args = if uninstall { wide("--uninstall") } else { wide("") };
    let verb = wide("runas");
    ShellExecuteW(
        HWND(0),
        PCWSTR(verb.as_ptr()),
        PCWSTR(exe_w.as_ptr()),
        PCWSTR(args.as_ptr()),
        PCWSTR::null(),
        SW_SHOWNORMAL,
    );
}

use windows::Win32::System::Registry::*;

fn reg_set(root: HKEY, sub: &str, name: &str, value: &str) {
    unsafe {
        let mut h = HKEY::default();
        let sub_w = wide(sub);
        if RegCreateKeyExW(
            root,
            PCWSTR(sub_w.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_WOW64_64KEY,
            None,
            &mut h,
            None,
        )
        .is_err()
        {
            return;
        }
        let name_w = wide(name);
        let val: Vec<u16> = value.encode_utf16().collect();
        let bytes = std::slice::from_raw_parts(val.as_ptr() as *const u8, val.len() * 2);
        let _ = RegSetValueExW(h, PCWSTR(name_w.as_ptr()), 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(h);
    }
}

fn reg_set_dword(root: HKEY, sub: &str, name: &str, v: u32) {
    unsafe {
        let mut h = HKEY::default();
        let sub_w = wide(sub);
        if RegCreateKeyExW(
            root,
            PCWSTR(sub_w.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_WOW64_64KEY,
            None,
            &mut h,
            None,
        )
        .is_err()
        {
            return;
        }
        let name_w = wide(name);
        let bytes = std::slice::from_raw_parts(&v as *const u32 as *const u8, 4);
        let _ = RegSetValueExW(h, PCWSTR(name_w.as_ptr()), 0, REG_DWORD, Some(bytes));
        let _ = RegCloseKey(h);
    }
}

fn reg_set_32(root: HKEY, sub: &str, name: &str, value: &str) {
    unsafe {
        let mut h = HKEY::default();
        let sub_w = wide(sub);
        if RegCreateKeyExW(
            root,
            PCWSTR(sub_w.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_WOW64_32KEY,
            None,
            &mut h,
            None,
        )
        .is_err()
        {
            return;
        }
        let name_w = wide(name);
        let val: Vec<u16> = value.encode_utf16().collect();
        let bytes = std::slice::from_raw_parts(val.as_ptr() as *const u8, val.len() * 2);
        let _ = RegSetValueExW(h, PCWSTR(name_w.as_ptr()), 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(h);
    }
}

fn reg_del_tree(root: HKEY, sub: &str) {
    unsafe {
        let sub_w = wide(sub);
        let _ = RegDeleteTreeW(root, PCWSTR(sub_w.as_ptr()));
    }
}

fn icacls_grant(dir: &str) {
    // 给打包应用(UWP)读+执行权限，否则新版记事本这类应用加载不了输入法
    let mut cmd = std::process::Command::new("icacls.exe");
    cmd.args([dir, "/grant", "*S-1-15-2-1:(OI)(CI)RX", "/T", "/C", "/Q"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let _ = cmd.status();
}

/// 建快捷方式（开始菜单 + 公共桌面）
unsafe fn make_shortcut(lnk: &str, target: &str, workdir: &str) {
    // 用 WScript.Shell 建快捷方式，省得折腾 COM
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath='{}'; $s.WorkingDirectory='{}'; $s.Description='旁译 · 中文→俄语输入法'; $s.Save()",
        lnk, target, workdir
    );
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let _ = cmd.status();
}

unsafe fn shortcuts() {
    let set = format!("{DIR}\\{SETTINGS_NAME}");
    let common_start = r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\旁译 设置.lnk";
    make_shortcut(common_start, &set, DIR);
    let public_desktop = r"C:\Users\Public\Desktop\旁译 设置.lnk";
    make_shortcut(public_desktop, &set, DIR);
}
fn install() {
    let dir = Path::new(DIR);
    if std::fs::create_dir_all(dir).is_err() {
        msg("旁译 安装失败", "无法创建安装目录。");
        return;
    }
    let d64 = dir.join(DLL64_NAME);
    let d32 = dir.join(DLL32_NAME);
    let ico = dir.join(ICO_NAME);
    let set = dir.join(SETTINGS_NAME);
    if std::fs::write(&d64, DLL64).is_err()
        || std::fs::write(&d32, DLL32).is_err()
        || std::fs::write(&ico, ICO).is_err()
        || std::fs::write(&set, SETTINGS64).is_err()
    {
        msg("旁译 安装失败", "复制组件失败。");
        return;
    }
    // 把自己也放进安装目录，卸载时用得上
    if let Ok(me) = std::env::current_exe() {
        let _ = std::fs::copy(me, dir.join("pangyi-setup.exe"));
    }
    icacls_grant(DIR);

    let d64s = d64.to_string_lossy().to_string();
    let d32s = d32.to_string_lossy().to_string();
    let icos = ico.to_string_lossy().to_string();

    // COM 注册（64 位 + 32 位）
    let clsid_key = format!("SOFTWARE\\Classes\\CLSID\\{CLSID_STR}");
    reg_set(HKEY_LOCAL_MACHINE, &clsid_key, "", "旁译");
    reg_set(HKEY_LOCAL_MACHINE, &format!("{clsid_key}\\InprocServer32"), "", &d64s);
    reg_set(HKEY_LOCAL_MACHINE, &format!("{clsid_key}\\InprocServer32"), "ThreadingModel", "Apartment");
    let clsid32 = format!("SOFTWARE\\Classes\\WOW6432Node\\CLSID\\{CLSID_STR}");
    reg_set(HKEY_LOCAL_MACHINE, &clsid32, "", "旁译");
    reg_set_32(HKEY_LOCAL_MACHINE, &format!("SOFTWARE\\Classes\\CLSID\\{CLSID_STR}\\InprocServer32"), "", &d32s);
    reg_set_32(HKEY_LOCAL_MACHINE, &format!("SOFTWARE\\Classes\\CLSID\\{CLSID_STR}\\InprocServer32"), "ThreadingModel", "Apartment");

    // TSF 文本服务
    let tip = format!("SOFTWARE\\Microsoft\\CTF\\TIP\\{CLSID_STR}");
    reg_set_dword(HKEY_LOCAL_MACHINE, &tip, "Enable", 1);
    for c in CATS {
        let sub = format!("{tip}\\Category\\Category\\{c}\\{CLSID_STR}");
        reg_set(HKEY_LOCAL_MACHINE, &sub, "", "");
        let sub2 = format!("{tip}\\Category\\Item\\{CLSID_STR}\\{c}");
        reg_set(HKEY_LOCAL_MACHINE, &sub2, "", "");
    }
    let lp = format!("{tip}\\LanguageProfile\\0x00000804\\{PROFILE_STR}");
    reg_set(HKEY_LOCAL_MACHINE, &lp, "Description", "旁译");
    reg_set(HKEY_LOCAL_MACHINE, &lp, "IconFile", &icos);
    reg_set_dword(HKEY_LOCAL_MACHINE, &lp, "IconIndex", 0);

    // 卸载入口（设置 → 应用）
    let un = format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Pangyi");
    reg_set(HKEY_LOCAL_MACHINE, &un, "DisplayName", "旁译 中文→俄语输入法");
    reg_set(HKEY_LOCAL_MACHINE, &un, "DisplayVersion", env!("CARGO_PKG_VERSION"));
    reg_set(HKEY_LOCAL_MACHINE, &un, "Publisher", "Pangyi");
    reg_set(HKEY_LOCAL_MACHINE, &un, "InstallLocation", DIR);
    reg_set(HKEY_LOCAL_MACHINE, &un, "DisplayIcon", &icos);
    let uninstall_cmd = format!("\"{}\" --uninstall", set.parent().unwrap().join("pangyi-setup.exe").to_string_lossy());
    reg_set(HKEY_LOCAL_MACHINE, &un, "UninstallString", &uninstall_cmd);
    reg_set_dword(HKEY_LOCAL_MACHINE, &un, "NoModify", 1);
    reg_set_dword(HKEY_LOCAL_MACHINE, &un, "NoRepair", 1);

    // 清掉以前按版本命名的旧组件（它们锁在已开的程序里就跳过）
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("pangyi_ime-") && name.ends_with(".dll") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    unsafe { shortcuts(); }

    // 清掉用户级覆盖（会盖住 HKLM）
    let hkcu = format!("Software\\Classes\\CLSID\\{CLSID_STR}");
    reg_del_tree(HKEY_CURRENT_USER, &hkcu);

    msg(
        "旁译 安装完成",
        "旁译已安装。\n\n下一步：注销后重新登录（或重启一次），然后在任意程序里按 Win+空格 切到「旁译」即可。\n\n设置：点状态条上的齿轮，或运行 C:\\Program Files\\Pangyi\\pangyi-settings.exe",
    );
}

fn uninstall() {
    let dir = Path::new(DIR);
    let clsid_key = format!("SOFTWARE\\Classes\\CLSID\\{CLSID_STR}");
    reg_del_tree(HKEY_LOCAL_MACHINE, &clsid_key);
    reg_del_tree(HKEY_LOCAL_MACHINE, &format!("SOFTWARE\\Classes\\WOW6432Node\\CLSID\\{CLSID_STR}"));
    reg_del_tree(HKEY_LOCAL_MACHINE, &format!("SOFTWARE\\Microsoft\\CTF\\TIP\\{CLSID_STR}"));
    reg_del_tree(HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Pangyi");
    reg_del_tree(HKEY_CURRENT_USER, &format!("Software\\Classes\\CLSID\\{CLSID_STR}"));
    let _ = std::fs::remove_file(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\旁译 设置.lnk");
    let _ = std::fs::remove_file(r"C:\Users\Public\Desktop\旁译 设置.lnk");
    for f in [DLL64_NAME, DLL32_NAME, ICO_NAME, SETTINGS_NAME, "pangyi-setup.exe"] {
        let _ = std::fs::remove_file(dir.join(f));
    }
    let _ = std::fs::remove_dir(dir);
    msg("旁译 已卸载", "旁译已卸载。注销或重启后，输入法列表里就不会再有它了。");
}

fn main() {
    unsafe {
        let do_uninstall = std::env::args().any(|a| a == "--uninstall");
        if !elevated() {
            relaunch_elevated(do_uninstall);
            return;
        }
        if do_uninstall {
            uninstall();
        } else {
            install();
        }
    }
}
