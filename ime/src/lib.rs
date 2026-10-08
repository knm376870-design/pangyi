//! 旁译 Pangyi —— Windows TSF 输入法（骨架）
//!
//! 里程碑 1：被 TSF 加载、收到按键、按空格把中文上屏。
//! 待办：编辑会话（ITfEditSession）、候选窗口、拼音下划线。

#![allow(non_snake_case)]

mod dict;

use std::cell::{Cell, RefCell};
use std::os::raw::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::core::{implement, ComInterface, Result, GUID, HRESULT};
use windows::Win32::Foundation::{BOOL, CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER, LPARAM, S_OK, WPARAM};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::TextServices::{
    ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfTextInputProcessor_Impl,
    ITfRangeACP, ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl, ITfThreadMgr, TF_SELECTION,
};

pub const CLSID_PANGYI: GUID = GUID::from_u128(0xB4E8F2A1_7C3D_4E5F_9A21_8D6C4B3E1F07);
pub const GUID_PROFILE: GUID = GUID::from_u128(0x5A7C9E12_3D4B_4F8A_B6E1_2C9D7A5E8F34);

static LIVE: AtomicU32 = AtomicU32::new(0);
const S_FALSE_HR: HRESULT = HRESULT(1);
const CLASS_E_CLASSNOTAVAILABLE_HR: HRESULT = HRESULT(0x80040111u32 as i32);

fn log(msg: &str) {
    use std::io::Write;
    let _ = std::fs::create_dir_all(r"C:\ProgramData\Pangyi");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(r"C:\ProgramData\Pangyi\ime.log")
    {
        let _ = writeln!(f, "{msg}");
    }
}

// ============================ 文本服务 ============================

#[implement(ITfTextInputProcessorEx, ITfKeyEventSink)]
struct TextService {
    client_id: Cell<u32>,
    buf: RefCell<String>,
}

impl TextService {
    fn new() -> Self {
        Self { client_id: Cell::new(0), buf: RefCell::new(String::new()) }
    }
}

impl ITfTextInputProcessor_Impl for TextService {
    fn Activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        self.activate(ptim, tid)
    }
    fn Deactivate(&self) -> Result<()> {
        self.deactivate()
    }
}

impl ITfTextInputProcessorEx_Impl for TextService {
    fn ActivateEx(&self, ptim: Option<&ITfThreadMgr>, tid: u32, _dwflags: u32) -> Result<()> {
        self.activate(ptim, tid)
    }
}

impl TextService {
    fn activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        log("Activate");
        if let Some(tm) = ptim {
            unsafe {
                self.client_id.set(tid);
                let ks: ITfKeystrokeMgr = tm.cast()?;
                let sink: ITfKeyEventSink = self.cast()?;
                match ks.AdviseKeyEventSink(tid, &sink, BOOL(1)) {
                    Ok(()) => log(&format!("AdviseKeyEventSink ok tid={tid}")),
                    Err(e) => log(&format!("AdviseKeyEventSink 失败 {e:?}")),
                }
            }
        }
        Ok(())
    }

    fn deactivate(&self) -> Result<()> {
        log("Deactivate");
        Ok(())
    }
}

impl ITfKeyEventSink_Impl for TextService {
    fn OnSetFocus(&self, _fg: BOOL) -> Result<()> { Ok(()) }

    fn OnTestKeyDown(&self, _pic: Option<&ITfContext>, wparam: WPARAM, _lp: LPARAM) -> Result<BOOL> {
        Ok(BOOL(handleable(wparam) as i32))
    }
    fn OnTestKeyUp(&self, _pic: Option<&ITfContext>, _w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        Ok(BOOL(0))
    }
    fn OnKeyUp(&self, _pic: Option<&ITfContext>, _w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        Ok(BOOL(0))
    }
    fn OnPreservedKey(&self, _pic: Option<&ITfContext>, _g: *const GUID) -> Result<BOOL> {
        Ok(BOOL(0))
    }

    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, _lp: LPARAM) -> Result<BOOL> {
        let ch = wparam.0 as u8 as char;
        log(&format!("key wparam={} ch={ch:?}", wparam.0));
        if ch.is_ascii_alphabetic() {
            self.buf.borrow_mut().push(ch.to_ascii_lowercase());
            return Ok(BOOL(1));
        }
        if wparam.0 == 0x20 {
            let key = self.buf.borrow().clone();
            if key.is_empty() {
                return Ok(BOOL(0));
            }
            self.buf.borrow_mut().clear();
            let hits = dict::lookup(&key);
            log(&format!("上屏 key={key} 候选={}", hits.len()));
            if let (Some(h), Some(ctx)) = (hits.first(), pic) {
                unsafe { let _ = commit(ctx, &h.word, key.len()); }
            }
            return Ok(BOOL(1));
        }
        if wparam.0 == 0x1B {
            self.buf.borrow_mut().clear();
            return Ok(BOOL(1));
        }
        Ok(BOOL(0))
    }
}

fn handleable(wparam: WPARAM) -> bool {
    let ch = wparam.0 as u8 as char;
    ch.is_ascii_alphabetic() || wparam.0 == 0x20 || wparam.0 == 0x1B
}

/// 把光标前 `back` 个 ASCII 字符替换成上屏文本。
unsafe fn commit(ctx: &ITfContext, word: &str, back: usize) -> Result<()> {
    let mut sel = [TF_SELECTION::default()];
    let mut fetched = 0u32;
    ctx.GetSelection(0, 0, &mut sel, &mut fetched)?;
    if fetched == 0 {
        return Ok(());
    }
    let range = match (&*sel[0].range).as_ref() {
        Some(r) => r,
        None => return Ok(()),
    };
    let acp: ITfRangeACP = range.cast()?;
    let mut s = 0i32;
    let mut e = 0i32;
    acp.GetExtent(&mut s, &mut e)?;
    acp.SetExtent(e - back as i32, e)?;
    let utf16: Vec<u16> = word.encode_utf16().collect();
    range.SetText(0, 0, &utf16)?;
    Ok(())
}

// ============================ COM 服务器 ============================

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory {
    fn CreateInstance(
        &self,
        punkouter: Option<&windows::core::IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> Result<()> {
        if punkouter.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        if ppv.is_null() {
            return Err(E_POINTER.into());
        }
        let svc: ITfTextInputProcessorEx = TextService::new().into();
        let hr = unsafe { svc.query(riid, ppv) };
        if hr.is_ok() { Ok(()) } else { Err(hr.into()) }
    }
    fn LockServer(&self, _f: BOOL) -> Result<()> { Ok(()) }
}

#[no_mangle]
pub extern "system" fn DllMain(_h: *mut c_void, _reason: u32, _r: *mut c_void) -> BOOL {
    BOOL(1)
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if LIVE.load(Ordering::SeqCst) == 0 { S_OK } else { S_FALSE_HR }
}

#[no_mangle]
pub extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        if ppv.is_null() {
            return E_POINTER;
        }
        *ppv = std::ptr::null_mut();
        if rclsid.is_null() || *rclsid != CLSID_PANGYI {
            return CLASS_E_CLASSNOTAVAILABLE_HR;
        }
        let f: IClassFactory = Factory.into();
        f.query(riid, ppv)
    }
}

#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT { E_FAIL }

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT { E_FAIL }
