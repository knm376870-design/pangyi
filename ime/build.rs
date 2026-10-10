//! 构建脚本：把 pangyi.ico 与版本信息编成 Windows 资源，链接进 setup（=桌面上的「旁译.exe」）。
//!
//! - 资源里 ID 为 1 的图标会成为 exe 在资源管理器/任务栏/Alt+Tab 里显示的图标。
//! - VERSIONINFO 让「属性 → 详细信息」显示中文名与版本号。
//! - windres 来自 tools/llvm-mingw/bin（x86_64 / i686 各一个），没找到就退回 PATH 上的 windres。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 目标三元组 → windres 可执行文件名
fn windres_name(target: &str) -> &'static str {
    if target.starts_with("i686") || target.starts_with("i586") {
        "i686-w64-mingw32-windres.exe"
    } else if target.starts_with("aarch64") {
        "aarch64-w64-mingw32-windres.exe"
    } else {
        "x86_64-w64-mingw32-windres.exe"
    }
}

fn find_windres(manifest: &Path, target: &str) -> PathBuf {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(root) = manifest.parent() {
        let bin = root.join("tools").join("llvm-mingw").join("bin");
        cands.push(bin.join(windres_name(target)));
        cands.push(bin.join("llvm-windres.exe"));
        cands.push(bin.join("windres.exe"));
    }
    for c in &cands {
        if c.is_file() {
            return c.clone();
        }
    }
    PathBuf::from("windres")
}

/// "4.0.1" → (4, 0, 1)
fn parse_ver(v: &str) -> (u64, u64, u64) {
    let mut it = v
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

fn quote(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = manifest.join("payload").join("pangyi.ico");
    println!("cargo:rerun-if-changed=payload/pangyi.ico");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=payload/pangyi_ime.dll");
    println!("cargo:rerun-if-changed=payload/pangyi_ime-x86.dll");

    if !ico.is_file() {
        println!("cargo:warning=payload/pangyi.ico 不存在，跳过资源编译");
        return;
    }

    let target = std::env::var("TARGET").unwrap_or_default();
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let (v0, v1, v2) = parse_ver(&ver);

    // 资源脚本：1=图标（Windows 取编号最小的图标做 exe 图标），外加中英双语版本信息
    let rc_src = format!(
        "#pragma code_page(65001)\n\
         1 ICON \"{ico}\"\n\
         \n\
         VS_VERSION_INFO VERSIONINFO\n\
         \x20FILEVERSION {v0},{v1},{v2},0\n\
         \x20PRODUCTVERSION {v0},{v1},{v2},0\n\
         \x20FILEFLAGSMASK 0x3fL\n\
         \x20FILEFLAGS 0x0L\n\
         \x20FILEOS 0x40004L\n\
         \x20FILETYPE 0x1L\n\
         \x20FILESUBTYPE 0x0L\n\
         BEGIN\n\
         \x20   BLOCK \"StringFileInfo\"\n\
         \x20   BEGIN\n\
         \x20       BLOCK \"080404b0\"\n\
         \x20       BEGIN\n\
         \x20           VALUE \"CompanyName\", \"Pangyi\"\n\
         \x20           VALUE \"FileDescription\", \"\u{65c1}\u{8bd1} \u{2014} \u{4e2d}\u{6587}\u{2192}\u{4fc4}\u{8bed}\u{8f93}\u{5165}\u{6cd5}\"\n\
         \x20           VALUE \"FileVersion\", \"{ver}\"\n\
         \x20           VALUE \"InternalName\", \"pangyi\"\n\
         \x20           VALUE \"LegalCopyright\", \"Pangyi\"\n\
         \x20           VALUE \"OriginalFilename\", \"pangyi.exe\"\n\
         \x20           VALUE \"ProductName\", \"\u{65c1}\u{8bd1}\"\n\
         \x20           VALUE \"ProductVersion\", \"{ver}\"\n\
         \x20       END\n\
         \x20   END\n\
         \x20   BLOCK \"VarFileInfo\"\n\
         \x20   BEGIN\n\
         \x20       VALUE \"Translation\", 0x804, 1200\n\
         \x20   END\n\
         END\n",
        ico = quote(&ico)
    );

    let rc_path = out_dir.join("pangyi.rc");
    if std::fs::write(&rc_path, rc_src.as_bytes()).is_err() {
        println!("cargo:warning=写 pangyi.rc 失败，跳过资源编译");
        return;
    }

    let obj = out_dir.join("pangyi_res.o");
    let windres = find_windres(&manifest, &target);
    let status = Command::new(&windres)
        .arg("--codepage=65001")
        .arg("-O")
        .arg("coff")
        .arg("-i")
        .arg(&rc_path)
        .arg("-o")
        .arg(&obj)
        .status();

    match status {
        Ok(s) if s.success() && obj.is_file() => {
            // 只贴到 setup 这个 bin 上（桌面上的「旁译.exe」就是它），DLL 不带资源
            println!("cargo:rustc-link-arg-bin=setup={}", quote(&obj));
        }
        other => {
            println!(
                "cargo:warning=windres 资源编译失败({:?})，exe 将使用默认图标",
                other
            );
        }
    }
}


