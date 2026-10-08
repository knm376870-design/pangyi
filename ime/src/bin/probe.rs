//! 问 TSF：我们的输入法到底注册进去了没有？
use windows::core::{Result, GUID};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfiles, TF_LANGUAGEPROFILE,
};

const OUR_CLSID: GUID = GUID::from_u128(0xB4E8F2A1_7C3D_4E5F_9A21_8D6C4B3E1F07);
const OUR_PROFILE: GUID = GUID::from_u128(0x5A7C9E12_3D4B_4F8A_B6E1_2C9D7A5E8F34);
const LANG_ZH_CN: u16 = 0x0804;

fn main() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let profiles: ITfInputProcessorProfiles =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        println!("[ok] 拿到 ITfInputProcessorProfiles");

        let en = profiles.EnumLanguageProfiles(LANG_ZH_CN)?;
        let mut buf: [TF_LANGUAGEPROFILE; 1] = std::mem::zeroed();
        let mut fetched = 0u32;
        let mut n = 0;
        let mut found = false;
        loop {
            en.Next(&mut buf, &mut fetched)?;
            if fetched == 0 {
                break;
            }
            let p = &buf[0];
            n += 1;
            let mine = p.clsid == OUR_CLSID;
            if mine {
                found = true;
            }
            println!(
                "  #{n} clsid={:?} profile={:?} active={} {}",
                p.clsid,
                p.guidProfile,
                p.fActive.as_bool(),
                if mine { "  <== 我们的！" } else { "" }
            );
        }
        println!("共 {n} 个中文(简体)输入法；我们的在里面吗: {found}");

        match profiles.ActivateLanguageProfile(&OUR_CLSID, LANG_ZH_CN, &OUR_PROFILE) {
            Ok(()) => println!("\n[ok] ActivateLanguageProfile(我们的) 成功 —— TSF 认识我们"),
            Err(e) => println!("\n[FAIL] ActivateLanguageProfile(我们的) 失败: {e:?}"),
        }
    }
    Ok(())
}
