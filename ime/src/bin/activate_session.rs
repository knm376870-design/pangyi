//! 在会话级别切换输入法：默认切到「旁译」，加参数 ms 则切回微软拼音。
use windows::core::{GUID, Result};
use windows::Win32::System::Com::*;
use windows::Win32::UI::TextServices::*;

const OUR_CLSID: GUID = GUID::from_u128(0xB4E8F2A1_7C3D_4E5F_9A21_8D6C4B3E1F07);
const OUR_PROFILE: GUID = GUID::from_u128(0x5A7C9E12_3D4B_4F8A_B6E1_2C9D7A5E8F34);
const MS_CLSID: GUID = GUID::from_u128(0x81D4E9C9_1D3B_41BC_9E6C_4B40BF79E35E);
const MS_PROFILE: GUID = GUID::from_u128(0xFA550B04_5AD7_411F_A5AC_CA038EC515D7);
const LANG: u16 = 0x0804;

fn main() -> Result<()> {
    unsafe {
        let use_ms = std::env::args().any(|a| a == "ms");
        let (clsid, profile, who) = if use_ms {
            (MS_CLSID, MS_PROFILE, "微软拼音")
        } else {
            (OUR_CLSID, OUR_PROFILE, "旁译")
        };
        println!("目标输入法: {who}");

        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mgr: ITfInputProcessorProfileMgr =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        println!("[ok] 拿到 ITfInputProcessorProfileMgr");

        for (name, flags) in [
            ("ENABLEPROFILE", TF_IPPMF_ENABLEPROFILE),
            ("FORSESSION|ENABLE", TF_IPPMF_FORSESSION | TF_IPPMF_ENABLEPROFILE),
            ("FORPROCESS|ENABLE", TF_IPPMF_FORPROCESS | TF_IPPMF_ENABLEPROFILE),
        ] {
            match mgr.ActivateProfile(TF_PROFILETYPE_INPUTPROCESSOR, LANG, &clsid, &profile, HKL(0), flags) {
                Ok(()) => println!("[ok] ActivateProfile({name}) 成功"),
                Err(e) => println!("[FAIL] ActivateProfile({name}): {e:?}"),
            }
        }

        if let Ok(p) = CoCreateInstance::<_, ITfInputProcessorProfiles>(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) {
            let mut lid: u16 = 0;
            let mut prof = GUID::zeroed();
            match p.GetActiveLanguageProfile(&clsid, &mut lid, &mut prof) {
                Ok(()) => println!("\n当前活动 profile={prof:?}\n  是目标吗: {}", prof == profile),
                Err(e) => println!("GetActiveLanguageProfile 失败: {e:?}"),
            }
        }
    }
    Ok(())
}