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

const DIR: &str = r"C:\Program Files\Pangyi";
const DLL64_NAME: &str = "pangyi_ime.dll";
const DLL32_NAME: &str = "pangyi_ime-x86.dll";
const ICO_NAME: &str = "pangyi.ico";
const SETTINGS_NAME: &str = "pangyi-settings.exe";
/// 单一入口程序名（安装 + 设置都在这一个 exe 里）
const APP_NAME: &str = "pangyi.exe";
/// 桌面上的那个软件名
const DESKTOP_APP: &str = "旁译.exe";
/// 版本标记：装完后写进安装目录，用来判断"已经是当前版本了"
const VERSION_FILE: &str = "version.txt";
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
    // 开始菜单只留【一个】入口，指向那个单一软件
    let app = format!("{DIR}\\{APP_NAME}");
    let _ = std::fs::remove_file(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\旁译 设置.lnk");
    make_shortcut(
        r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\旁译.lnk",
        &app,
        DIR,
    );
}

/// 写组件；如果目标文件被正在运行的程序占用，就换个带时间戳的名字写，
/// 再把注册表指过去（否则升级会"装了个寂寞"）。
fn write_or_versioned(dir: &Path, name: &str, data: &[u8]) -> String {
    let p = dir.join(name);
    if std::fs::write(&p, data).is_ok() {
        return p.to_string_lossy().to_string();
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stem = name.trim_end_matches(".dll");
    let alt = dir.join(format!("{stem}-{stamp}.dll"));
    let _ = std::fs::write(&alt, data);
    alt.to_string_lossy().to_string()
}

/// 删不掉的文件（被运行中的程序占用）排到"重启后删除"。
/// 不这样做的话，旧 DLL 会永远留在安装目录里，"删干净"就成了空话。
fn remove_or_schedule(p: &Path) {
    if std::fs::remove_file(p).is_ok() {
        return;
    }
    use std::os::windows::ffi::OsStrExt;
    let mut w: Vec<u16> = p.as_os_str().encode_wide().collect();
    w.push(0);
    unsafe {
        // MOVEFILE_DELAY_UNTIL_REBOOT = 0x4；第二个参数传 NULL 表示删除
        let _ = windows::Win32::Storage::FileSystem::MoveFileExW(
            windows::core::PCWSTR(w.as_ptr()),
            windows::core::PCWSTR::null(),
            windows::Win32::Storage::FileSystem::MOVE_FILE_FLAGS(0x4),
        );
    }
}

/// 把旧版本的所有痕迹清掉。装新版时调用一次。
/// 1) 安装目录里所有不是本次注册的 pangyi_ime*.dll
/// 2) 用户目录里早期"用户级注册"留下的副本、旧原型 exe、旧日志
/// 3) 指向旧路径的桌面快捷方式
/// 仍被运行中程序占用的文件删不掉，跳过即可（下次安装或重启后会自动清掉）。
fn purge_old_versions(dir: &Path, keep: &[String]) {
    let is_keep = |p: &Path| {
        let s = p.to_string_lossy().to_string();
        keep.iter().any(|k| k.eq_ignore_ascii_case(&s))
    };

    // 1) 安装目录：只留这一轮注册的那两个 DLL，外加"那一个软件"
    //    （旧的 pangyi-settings.exe / pangyi-setup.exe 现在都并进单一软件了，要删掉）
    let keep_names = [DLL64_NAME, DLL32_NAME, APP_NAME, ICO_NAME, VERSION_FILE];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let low = name.to_ascii_lowercase();
            let is_old_dll = low.starts_with("pangyi_ime") && low.ends_with(".dll") && !is_keep(&e.path());
            let is_obsolete = keep_names.iter().all(|k| !k.eq_ignore_ascii_case(&name))
                && (low.ends_with(".exe") || name.starts_with("旁译"));
            if is_old_dll || is_obsolete {
                remove_or_schedule(&e.path());
            }
        }
    }

    // 2) 用户目录：现在已经是系统级安装，这些副本一个都不该留
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        let user_dir = Path::new(&base).join("Pangyi");
        if let Ok(rd) = std::fs::read_dir(&user_dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let low = name.to_ascii_lowercase();
                let stale = (low.starts_with("pangyi_ime") && low.ends_with(".dll"))
                    || low == "pangyi-settings.exe"
                    || low.ends_with(".log")
                    || legacy_desktop_names().iter().any(|k| *k == name);
                if stale {
                    remove_or_schedule(&e.path());
                }
            }
        }
    }

    // 3) 指向旧路径的桌面快捷方式
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let _ = std::fs::remove_file(
            Path::new(&profile).join("Desktop").join("旁译 · 中俄输入.lnk"),
        );
    }
}

/// 桌面上已知的旧版旁译文件名（精确匹配，只删这些）。
///
/// ⚠️ 绝不能用「旁译」前缀匹配清桌面：用户桌面上可能有「旁译-对话存档」这种
/// 以旁译开头的目录或文件，前缀匹配会误删（对目录还会排队"重启后删除"）。
fn legacy_desktop_names() -> [&'static str; 5] {
    [
        DESKTOP_APP, // 上一版的那个软件，重新复制前先删掉
        "旁译 设置.lnk",
        "旁译 · 中俄输入.lnk",
        "旁译设置.exe",
        "旁译安装.exe",
    ]
}

/// 把"旁译"做成【桌面上的一个软件】。
///
/// 做法：先清掉桌面/公共桌面上清单里的旧旁译文件（安装包、设置程序、旧快捷方式），
/// 再只放一个「旁译.exe」。以后每次升级也走这里，所以桌面上永远只有一个。
fn place_single_app() {
    let me = std::env::current_exe().ok();

    let mut desks: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        desks.push(Path::new(&profile).join("Desktop"));
    }
    desks.push(Path::new(r"C:\Users\Public\Desktop").to_path_buf());

    for (i, d) in desks.iter().enumerate() {
        if !d.is_dir() {
            continue;
        }
        // 只删清单里的旧文件（精确匹配 + 只动文件，目录一律不碰）
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let Ok(ft) = e.file_type() else { continue };
                if !ft.is_file() {
                    continue;
                }
                let name = e.file_name().to_string_lossy().to_string();
                if legacy_desktop_names().iter().any(|k| *k == name) {
                    remove_or_schedule(&e.path());
                }
            }
        }
        // 只在【用户桌面】放那个唯一的软件；公共桌面不放（避免出现两份）
        if i == 0 {
            if let Some(m) = me.as_ref() {
                let _ = std::fs::copy(m, d.join(DESKTOP_APP));
            }
        }
    }
}

fn install() {
    let dir = Path::new(DIR);
    if std::fs::create_dir_all(dir).is_err() {
        msg("旁译 安装失败", "无法创建安装目录。");
        return;
    }
    let ico = dir.join(ICO_NAME);
    let _ = std::fs::write(&ico, ICO);
    unsafe { shortcuts(); }

    let d64s = write_or_versioned(dir, DLL64_NAME, DLL64);
    let d32s = write_or_versioned(dir, DLL32_NAME, DLL32);
    let d64 = std::path::PathBuf::from(&d64s);
    let d32 = std::path::PathBuf::from(&d32s);
    // 把自己放进安装目录（这就是"那个软件"），卸载时也用它
    if let Ok(me) = std::env::current_exe() {
        let _ = std::fs::copy(&me, dir.join(APP_NAME));
        let _ = std::fs::remove_file(dir.join("pangyi-setup.exe"));
    }
    // 版本标记：下次再双击时，是"开设置"还是"装新版"就看它
    let _ = std::fs::write(dir.join(VERSION_FILE), env!("CARGO_PKG_VERSION"));
    icacls_grant(DIR);

    let d64s = d64.to_string_lossy().to_string();
    let d32s = d32.to_string_lossy().to_string();
    let icos = ico.to_string_lossy().to_string();

    // 每次装新版都把旧版清干净：只留注册表这一轮指向的两个 DLL。
    // 放在「写完新文件之后」执行，任何时刻注册表指向的文件都在，
    // 不会出现先删旧、后写新中间那个空窗。
    purge_old_versions(dir, &[d64s.clone(), d32s.clone()]);

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
    let inst_dir = Path::new(DIR);
    let app_path = inst_dir.join(APP_NAME);
    let uninstall_cmd = format!("\"{}\" --uninstall", app_path.to_string_lossy());
    reg_set(HKEY_LOCAL_MACHINE, &un, "UninstallString", &uninstall_cmd);
    reg_set_dword(HKEY_LOCAL_MACHINE, &un, "NoModify", 1);
    reg_set_dword(HKEY_LOCAL_MACHINE, &un, "NoRepair", 1);

    // 清掉用户级覆盖（会盖住 HKLM）——64 位和 32 位两处都要清
    reg_del_tree(HKEY_CURRENT_USER, &format!("Software\\Classes\\CLSID\\{CLSID_STR}"));
    reg_del_tree(HKEY_CURRENT_USER, &format!("Software\\Classes\\WOW6432Node\\CLSID\\{CLSID_STR}"));

    place_single_app();

    msg(
        "旁译 安装完成",
        "旁译已安装。\n\n下一步：注销后重新登录（或重启一次），然后在任意程序里按 Win+空格 切到「旁译」即可。\n\n设置：双击桌面上的「旁译」，或运行 C:\\Program Files\\Pangyi\\pangyi.exe",
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
    let _ = std::fs::remove_file(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\旁译.lnk");
    let _ = std::fs::remove_file(r"C:\Users\Public\Desktop\旁译 设置.lnk");
    // 桌面上的那个软件也删掉
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let _ = std::fs::remove_file(Path::new(&profile).join("Desktop").join(DESKTOP_APP));
    }
    for f in [DLL64_NAME, DLL32_NAME, ICO_NAME, SETTINGS_NAME, "pangyi-setup.exe", APP_NAME, VERSION_FILE] {
        let _ = std::fs::remove_file(dir.join(f));
    }
    let _ = std::fs::remove_dir(dir);
    msg("旁译 已卸载", "旁译已卸载。注销或重启后，输入法列表里就不会再有它了。");
}

/// 已经装好了吗？（安装目录里有 version.txt 且版本号与当前 exe 一致）
fn installed_current() -> bool {
    let dir = Path::new(DIR);
    let v = match std::fs::read_to_string(dir.join(VERSION_FILE)) {
        Ok(v) => v,
        Err(_) => return false,
    };
    v.trim() == env!("CARGO_PKG_VERSION") && dir.join(DLL64_NAME).is_file()
}

/// 打开设置窗口（不需要管理员权限）。
/// 这就是"那一个软件"的日常用途 —— 双击就开设置。
unsafe fn open_settings() -> ! {
    pangyi_ime::settings::load();
    pangyi_ime::settings::open_app();
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    std::process::exit(0);
}

fn main() {
    unsafe {
        let args: Vec<String> = std::env::args().collect();
        let do_uninstall = args.iter().any(|a| a == "--uninstall");
        let force_install = args.iter().any(|a| a == "--install");

        // 已经是最新版 → 直接开设置窗口（不需要 UAC）
        if !do_uninstall && !force_install && installed_current() {
            open_settings();
        }

        if !elevated() {
            relaunch_elevated(do_uninstall);
            return;
        }
        if do_uninstall {
            uninstall();
        } else {
            install();
            // 装完顺手把设置窗口打开，用户点一次就什么都齐了
            open_settings();
        }
    }
}
