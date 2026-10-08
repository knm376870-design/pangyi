//! 旁译 Pangyi —— Windows TSF 输入法（骨架）
//!
//! 里程碑 1：被 TSF 加载、收到按键、按空格把中文上屏。
//! 待办：编辑会话（ITfEditSession）、候选窗口、拼音下划线。

#![allow(non_snake_case)]

mod dict;

use std::cell::{Cell, RefCell};
use std::os::raw::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::core::{implement, ComInterface, Interface, Result, GUID, HRESULT};
use windows::Win32::Foundation::{BOOL, CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER, LPARAM, S_OK, WPARAM};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::TextServices::{
    ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfTextInputProcessor_Impl,
    ITfClientId, ITfDocumentMgr, ITfEditSession, ITfEditSession_Impl, ITfTextInputProcessorEx,
    ITfTextInputProcessorEx_Impl, ITfThreadMgrEventSink, ITfThreadMgrEventSink_Impl,
    ITfSource, ITfThreadMgr, TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION,
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
        let _ = writeln!(f, "[pid {}] {msg}", std::process::id());
    }
}

// ============================ 文本服务 ============================

#[implement(ITfTextInputProcessorEx, ITfKeyEventSink, ITfThreadMgrEventSink)]
struct TextService {
    client_id: Cell<u32>,
    tid: Cell<u32>,
    tm: Cell<*mut c_void>,
    advised: Cell<bool>,
    buf: RefCell<String>,
}

impl TextService {
    fn new() -> Self {
        Self { client_id: Cell::new(0), tid: Cell::new(0), tm: Cell::new(std::ptr::null_mut()), advised: Cell::new(false), buf: RefCell::new(String::new()) }
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
                self.tid.set(tid);
                // TSF 要的是 TfClientId（用 CLSID 换），不是线程 ID。
                // 传错会导致 AdviseKeyEventSink 返回 0x80040201（事件无法调用任何订户）。
                match tm.cast::<ITfClientId>() {
                    Ok(ci) => match ci.GetClientId(&CLSID_PANGYI) {
                        Ok(cid) => { self.client_id.set(cid); log(&format!("TfClientId={cid}")); }
                        Err(e) => log(&format!("GetClientId 失败 {e:?}")),
                    },
                    Err(e) => log(&format!("拿不到 ITfClientId: {e:?}")),
                }
                self.tm.set(tm.as_raw());
                let tmsink: ITfThreadMgrEventSink = self.cast()?;
                match tm.cast::<ITfSource>() {
                    Ok(src) => match src.AdviseSink(&ITfThreadMgrEventSink::IID, &tmsink) {
                        Ok(cookie) => log(&format!("AdviseSink(ThreadMgrEventSink) ok cookie={cookie}")),
                        Err(e) => log(&format!("AdviseSink 调用失败 {e:?}")),
                    },
                    Err(e) => log(&format!("拿不到 ITfSource: {e:?}")),
                }
                self.advise_key_sink();
            }
        }
        Ok(())
    }

    /// 挂（或重挂）按键接收器。焦点变化时会再调一次。
    fn advise_key_sink(&self) {
        if self.advised.get() {
            return;   // 已经挂过就不重复挂：TSF 对同一 sink 二次 Advise 会返回 0x80040201
        }
        let raw = self.tm.get();
        if raw.is_null() { return; }
        unsafe {
            let tm = std::mem::ManuallyDrop::new(ITfThreadMgr::from_raw(raw));
            let Ok(ks) = tm.cast::<ITfKeystrokeMgr>() else { return };
            let Ok(sink) = self.cast::<ITfKeyEventSink>() else { return };
            let cid = self.client_id.get();
            match ks.AdviseKeyEventSink(cid, &sink, BOOL(1)) {
                Ok(()) => { log(&format!("AdviseKeyEventSink ok (TfClientId={cid})")); self.advised.set(true); }
                Err(e) => log(&format!("AdviseKeyEventSink 失败 cid={cid} {e:?}")),
            }
        }
    }

    fn deactivate(&self) -> Result<()> {
        log("Deactivate");
        Ok(())
    }
}

impl ITfKeyEventSink_Impl for TextService {
    fn OnSetFocus(&self, fg: BOOL) -> Result<()> {
        log(&format!("OnSetFocus fg={}", fg.as_bool()));
        Ok(())
    }

    fn OnTestKeyDown(&self, _pic: Option<&ITfContext>, wparam: WPARAM, _lp: LPARAM) -> Result<BOOL> {
        let want = handleable(wparam);
        if want {
            log(&format!("OnTestKeyDown wparam={} 我要={want}", wparam.0));
        }
        Ok(BOOL(want as i32))
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
                // TSF 规定：改文本必须包在编辑会话里，否则 SetText 会被拒
                let session: ITfEditSession = CommitSession {
                    ctx: ctx.clone(),
                    text: h.word.encode_utf16().collect(),
                }
                .into();
                let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
                match unsafe { ctx.RequestEditSession(self.tid.get(), &session, flags) } {
                    Ok(hr) => log(&format!("RequestEditSession -> {hr:?}")),
                    Err(e) => log(&format!("RequestEditSession 失败 {e:?}")),
                }
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

/// 编辑会话：在 TSF 允许的时机把文本写进文档。
#[implement(ITfEditSession)]
struct CommitSession {
    ctx: ITfContext,
    text: Vec<u16>,
}

impl ITfEditSession_Impl for CommitSession {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        unsafe {
            let mut sel = [TF_SELECTION::default()];
            let mut n = 0u32;
            self.ctx.GetSelection(ec, 0, &mut sel, &mut n)?;
            if n == 0 {
                return Ok(());
            }
            let range = match (&*sel[0].range).as_ref() {
                Some(r) => r,
                None => return Ok(()),
            };
            match range.SetText(ec, 0, &self.text) {
                Ok(()) => log(&format!("DoEditSession 上屏成功 {} 字", self.text.len())),
                Err(e) => log(&format!("DoEditSession SetText 失败 {e:?}")),
            }
        }
        Ok(())
    }
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
    log("DllGetClassObject 被调用");
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

impl ITfThreadMgrEventSink_Impl for TextService {
    fn OnInitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        log("ThreadMgr::OnInitDocumentMgr");
        Ok(())
    }
    fn OnUninitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        log("ThreadMgr::OnUninitDocumentMgr");
        Ok(())
    }
    fn OnSetFocus(&self, _focus: Option<&ITfDocumentMgr>, _prev: Option<&ITfDocumentMgr>) -> Result<()> {
        log("ThreadMgr::OnSetFocus -> 重挂按键接收器");
        self.advise_key_sink();
        Ok(())
    }
    fn OnPushContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        log("ThreadMgr::OnPushContext");
        Ok(())
    }
    fn OnPopContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        log("ThreadMgr::OnPopContext");
        Ok(())
    }
}