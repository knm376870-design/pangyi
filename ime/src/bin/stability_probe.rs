//! Headless checks against a built DLL. Does not register the IME, create windows,
//! activate an input profile, or send keyboard input to other applications.
use windows::core::{s, ComInterface, Error, Interface, Result, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, E_FAIL, HMODULE, LPARAM, S_OK, WPARAM};
use windows::Win32::System::Com::IClassFactory;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::TextServices::{ITfKeyEventSink, ITfTextInputProcessorEx};

const CLSID: GUID = GUID::from_u128(0xB4E8F2A1_7C3D_4E5F_9A21_8D6C4B3E1F07);

struct LoadedDll(HMODULE);
impl Drop for LoadedDll {
    fn drop(&mut self) { unsafe { let _ = FreeLibrary(self.0); } }
}

fn main() -> Result<()> {
    let path = std::env::args().nth(1)
        .ok_or_else(|| Error::new(E_FAIL, "Usage: stability_probe.exe DLL_PATH".into()))?;
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let dll = LoadedDll(LoadLibraryW(PCWSTR(wide.as_ptr()))?);
        type GetClass = unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut core::ffi::c_void) -> HRESULT;
        type CanUnload = unsafe extern "system" fn() -> HRESULT;
        let get: GetClass = std::mem::transmute(
            GetProcAddress(dll.0, s!("DllGetClassObject")).ok_or_else(Error::from_win32)?
        );
        let can_unload: CanUnload = std::mem::transmute(
            GetProcAddress(dll.0, s!("DllCanUnloadNow")).ok_or_else(Error::from_win32)?
        );
        assert_eq!(can_unload(), S_OK, "DLL should initially be idle");
        let mut raw = std::ptr::null_mut();
        get(&CLSID, &IClassFactory::IID, &mut raw).ok()?;
        if raw.is_null() { return Err(E_FAIL.into()); }
        let factory = IClassFactory::from_raw(raw);
        let service: ITfTextInputProcessorEx = factory.CreateInstance(None)?;
        let sink: ITfKeyEventSink = service.cast()?;
        assert_eq!(can_unload(), HRESULT(1), "Live text service must prevent DLL unloading");

        let mut checked = 0;
        let keys = (0x60..=0x6F).chain(0x70..=0x87)
            .chain(0x21..=0x28).chain([0x08, 0x09, 0x0D, 0x1B, 0x20, 0x2D, 0x2E]);
        for vk in keys {
            assert!(!sink.OnTestKeyDown(None, WPARAM(vk), LPARAM(0))?.as_bool(),
                "TestKeyDown swallowed VK {vk:#x}");
            assert!(!sink.OnKeyDown(None, WPARAM(vk), LPARAM(0))?.as_bool(),
                "KeyDown swallowed VK {vk:#x}");
            checked += 1;
        }
        drop(sink);
        drop(service);
        drop(factory);
        assert_eq!(can_unload(), S_OK, "Text service references leaked");
        println!("PASS: {}-bit DLL; {checked} keys passed through both callbacks; COM lifecycle passed", usize::BITS);
        println!("DLL: {path}");
    }
    Ok(())
}
