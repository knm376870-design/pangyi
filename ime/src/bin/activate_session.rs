//! 在会话级别激活「旁译」，让新开的程序默认用它。
use windows::core::{GUID, Result};
use windows::Win32::System::Com::*;
use windows::Win32::UI::TextServices::*;

const OUR_CLSID: GUID = GUID::from_u128(0xB4E8F2A1_7C3D_4E5F_9A21_8D6C4B3E1F07);
const OUR_PROFILE: GUID = GUID::from_u128(0x5A7C9E12_3D4B_4F8A_B6E1_2C9D7A5E8F34);
const LANG: u16 = 0x0804;

fn main() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mgr: ITfInputProcessorProfileMgr =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        println!("[ok] 拿到 ITfInputProcessorProfileMgr");

        for (name, flags) in [
            ("ENABLEPROFILE", TF_IPPMF_ENABLEPROFILE),
            ("FORSESSION|ENABLE", TF_IPPMF_FORSESSION | TF_IPPMF_ENABLEPROFILE),
            ("FORPROCESS|ENABLE", TF_IPPMF_FORPROCESS | TF_IPPMF_ENABLEPROFILE),
        ] {
            match mgr.ActivateProfile(TF_PROFILETYPE_INPUTPROCESSOR, LANG, &OUR_CLSID, &OUR_PROFILE, HKL(0), flags) {
                Ok(()) => println!("[ok] ActivateProfile({name}) 成功"),
                Err(e) => println!("[FAIL] ActivateProfile({name}): {e:?}"),
            }
        }

        // 查一下这个线程当前的活动 profile，确认到底切没切过去
        if let Ok(p) = CoCreateInstance::<_, ITfInputProcessorProfiles>(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) {
            let mut lid: u16 = 0;
            let mut prof = GUID::zeroed();
            match p.GetActiveLanguageProfile(&OUR_CLSID, &mut lid, &mut prof) {
                Ok(()) => println!("\n查询结果: langid=0x{lid:04X} profile={prof:?}\n  是我们的 profile 吗: {}", prof == OUR_PROFILE),
                Err(e) => println!("GetActiveLanguageProfile 失败: {e:?}（说明我们不是当前活动输入法）"),
            }
        }
    }
    Ok(())
}
