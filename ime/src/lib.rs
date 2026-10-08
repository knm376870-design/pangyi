//! 旁译 Pangyi —— Windows TSF 输入法（骨架）
//!
//! 里程碑 1：被 TSF 加载、收到按键、按空格把中文上屏。
//! 待办：编辑会话（ITfEditSession）、候选窗口、拼音下划线。

#![allow(non_snake_case)]

pub mod cand;
pub mod dict;
pub mod settings;
pub mod status;

use std::cell::{Cell, RefCell};
use std::os::raw::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use std::sync::Mutex;
use windows::core::{implement, ComInterface, Interface, PCWSTR, Result, GUID, HRESULT};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Foundation::{BOOL, CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER, COLORREF, HWND, LPARAM, LRESULT, POINT, S_OK, TRUE, WPARAM};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::TextServices::{
    ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfTextInputProcessor_Impl,
    ITfClientId, ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContextComposition,
    ITfDocumentMgr, ITfEditSession, ITfEditSession_Impl, ITfTextInputProcessorEx,
    ITfTextInputProcessorEx_Impl, ITfThreadMgrEventSink, ITfThreadMgrEventSink_Impl,
    ITfSource, ITfThreadMgr, TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION,
};

static PENDING_CAND: Mutex<Option<(i32, i32)>> = Mutex::new(None);

pub const CLSID_PANGYI: GUID = GUID::from_u128(0xB4E8F2A1_7C3D_4E5F_9A21_8D6C4B3E1F07);
pub const GUID_PROFILE: GUID = GUID::from_u128(0x5A7C9E12_3D4B_4F8A_B6E1_2C9D7A5E8F34);

static LIVE: AtomicU32 = AtomicU32::new(0);
static mut COMPOSITION: Option<ITfComposition> = None;
static mut CAND_HWND: HWND = HWND(0);
static UI_CANDS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
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

#[implement(ITfTextInputProcessorEx, ITfKeyEventSink, ITfThreadMgrEventSink, ITfCompositionSink)]
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
    /// 把某个词上屏（换掉当前组合）
    fn commit_word(&self, pic: Option<&ITfContext>, word: &str) {
        if let Some(ctx) = pic {
            let session: ITfEditSession = DoneSession {
                ctx: ctx.clone(),
                text: word.encode_utf16().collect(),
            }
            .into();
            let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
            let _ = unsafe { ctx.RequestEditSession(self.client_id.get(), &session, flags) };
        }
    }

    /// 更新组合内容为当前的拼音串（用于退格）
    fn refresh_composition(&self, pic: Option<&ITfContext>) {
        let key = self.buf.borrow().clone();
        if key.is_empty() {
            unsafe { COMPOSITION = None; }
            unsafe { cand_hide(); }
            return;
        }
        unsafe { cand::set_preedit(&key); }
        unsafe { cand::set_sentence(dict::complete(&key).map(|(w, _)| w)); }
        let hits = dict::lookup(&key);
        let items: Vec<cand::Item> = hits
            .iter()
            .map(|h| cand::Item { word: h.word.clone(), pinyin: h.pinyin.clone(), ru: h.ru.clone(), fresh: false })
            .collect();
        unsafe { cand::set_items(items); }
        {
            let mut g = UI_CANDS.lock().unwrap();
            g.clear();
            g.extend(hits.iter().map(|h| (h.word.clone(), h.ru.clone())));
        }
        if let Some(ctx) = pic {
            let s: ITfEditSession = UpdateSession { text: key.encode_utf16().collect() }.into();
            let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
            let _ = unsafe { ctx.RequestEditSession(self.client_id.get(), &s, flags) };
        }
    }
}

impl TextService {
    fn activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        log("Activate");
        unsafe { settings::load(); status::init(); status::show(); }
        // 后台把词典索引建好，避免第一次打字时卡一下
        std::thread::spawn(|| {
            let _ = dict::lookup("kaifa");
        });
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
                if CAND_HWND.0 == 0 { cand_init(); }
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
        unsafe { status::hide(); }
        // 关键：切走再切回来会重新 Activate，必须允许重新挂按键接收器。
        // 之前这里没复位，导致"切换回来后按键全部漏给应用"。
        self.advised.set(false);
        Ok(())
    }
}

impl ITfCompositionSink_Impl for TextService {
    fn OnCompositionTerminated(&self, _ec: u32, _c: Option<&ITfComposition>) -> Result<()> {
        unsafe { COMPOSITION = None; }
        Ok(())
    }
}

impl ITfKeyEventSink_Impl for TextService {
    fn OnSetFocus(&self, fg: BOOL) -> Result<()> {
        if !fg.as_bool() {
            // 只收候选窗；不要动组合——实测清组合会让上屏丢字
            unsafe { cand_hide(); }
        }
        Ok(())
    }

    fn OnTestKeyDown(&self, _pic: Option<&ITfContext>, wparam: WPARAM, _lp: LPARAM) -> Result<BOOL> {
        let ch0 = wparam.0 as u8 as char;
        if ch0.is_ascii_alphabetic() && status::english() {
            return Ok(BOOL(0));
        }
        let has_buf = !self.buf.borrow().is_empty();
        let want = claim(wparam, has_buf, status::english());
        log(&format!("KeySink::OnTestKeyDown wparam={} 我要={want}", wparam.0));
        Ok(BOOL(want as i32))
    }
    fn OnTestKeyUp(&self, _pic: Option<&ITfContext>, w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        log(&format!("KeySink::OnTestKeyUp wparam={}", w.0));
        Ok(BOOL(0))
    }
    fn OnKeyUp(&self, _pic: Option<&ITfContext>, w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        log(&format!("KeySink::OnKeyUp wparam={}", w.0));
        Ok(BOOL(0))
    }
    fn OnPreservedKey(&self, _pic: Option<&ITfContext>, g: *const GUID) -> Result<BOOL> {
        log(&format!("KeySink::OnPreservedKey guid={g:?}"));
        Ok(BOOL(0))
    }

    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, _lp: LPARAM) -> Result<BOOL> {
        let ch = wparam.0 as u8 as char;
        log(&format!("KeySink::OnKeyDown wparam={} ch={ch:?}", wparam.0));

        // 字母：进组合
        if ch.is_ascii_alphabetic() {
            if status::english() { return Ok(BOOL(0)); }
            self.buf.borrow_mut().push(ch.to_ascii_lowercase());
            let key = self.buf.borrow().clone();
            unsafe { cand::set_preedit(&key); }
            unsafe { cand::set_sentence(dict::complete(&key).map(|(w, _)| w)); }
            let hits = dict::lookup(&key);
            let items: Vec<cand::Item> = hits.iter().map(|h| cand::Item { word: h.word.clone(), pinyin: h.pinyin.clone(), ru: h.ru.clone(), fresh: false }).collect();
            unsafe { cand::set_items(items); }
            {
                let mut g = UI_CANDS.lock().unwrap();
                g.clear();
                g.extend(hits.iter().map(|h| (h.word.clone(), h.ru.clone())));
            }
            if let Some(ctx) = pic {
                let text: Vec<u16> = key.encode_utf16().collect();
                let sink: ITfCompositionSink = match unsafe { self.cast() } { Ok(s) => s, Err(_) => return Ok(BOOL(1)) };
                let s: ITfEditSession = CompSession { ctx: ctx.clone(), text, sink }.into();
                let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
                let _ = unsafe { ctx.RequestEditSession(self.client_id.get(), &s, flags) };
            }
            if let Some((x, y)) = *PENDING_CAND.lock().unwrap() {
                unsafe { cand_update(x, y); }
            }
            return Ok(BOOL(1));
        }

        let has_buf = !self.buf.borrow().is_empty();

        // 退格：删拼音
        if wparam.0 == 0x08 {
            if !has_buf { return Ok(BOOL(0)); }
            self.buf.borrow_mut().pop();
            self.refresh_composition(pic);
            return Ok(BOOL(1));
        }

        // 数字 1-9：直接选第几个候选
        if (0x31..=0x39).contains(&(wparam.0 as u32)) {
            if !has_buf { return Ok(BOOL(0)); }
            let idx = (wparam.0 as usize) - 0x31;
            let word = UI_CANDS.lock().unwrap().get(idx).map(|(w, _)| w.clone());
            if let Some(w) = word {
                self.buf.borrow_mut().clear();
                UI_CANDS.lock().unwrap().clear();
                unsafe { cand_hide(); }
                self.commit_word(pic, &w);
            }
            return Ok(BOOL(1));
        }

        // 方向键 / Tab：在候选里移动
        if matches!(wparam.0 as u32, 0x26 | 0x28 | 0x09) {
            if !has_buf { return Ok(BOOL(0)); }
            let n = UI_CANDS.lock().unwrap().len().min(9);
            if n == 0 { return Ok(BOOL(1)); }
            let cur = unsafe { cand::current_index() };
            let next = if (wparam.0 as u32) == 0x26 { (cur + n - 1) % n } else { (cur + 1) % n };
            unsafe { cand::select(next); }
            return Ok(BOOL(1));
        }

        // 回车 / 空格：上屏当前选中的候选
        if wparam.0 == 0x0D || wparam.0 == 0x20 {
            if !has_buf { return Ok(BOOL(0)); }
            let w = unsafe { cand::current() }
                .or_else(|| UI_CANDS.lock().unwrap().first().map(|(w, _)| w.clone()));
            self.buf.borrow_mut().clear();
            UI_CANDS.lock().unwrap().clear();
            unsafe { cand_hide(); }
            log(&format!("上屏 word={:?}", w));
            if let Some(w) = w { self.commit_word(pic, &w); }
            return Ok(BOOL(1));
        }

        // ESC：取消这次输入
        if wparam.0 == 0x1B {
            if !has_buf { return Ok(BOOL(0)); }
            self.buf.borrow_mut().clear();
            UI_CANDS.lock().unwrap().clear();
            unsafe { cand_hide(); }
            unsafe { COMPOSITION = None; }
            return Ok(BOOL(1));
        }

        // PageUp / PageDown：先吃掉，别把拼音留在组合里
        if matches!(wparam.0 as u32, 0x21 | 0x22) {
            if !has_buf { return Ok(BOOL(0)); }
            return Ok(BOOL(1));
        }

        // 标点：先把当前候选上屏，再放行让标点正常输入
        if has_buf && is_punct_vk(wparam.0 as u32) {
            let w = unsafe { cand::current() }
                .or_else(|| UI_CANDS.lock().unwrap().first().map(|(w, _)| w.clone()));
            self.buf.borrow_mut().clear();
            UI_CANDS.lock().unwrap().clear();
            unsafe { cand_hide(); }
            if let Some(w) = w { self.commit_word(pic, &w); }
            return Ok(BOOL(0));
        }

        Ok(BOOL(0))
    }}

/// 标点键（TSF 传进来的是虚拟键码，不是字符）
fn is_punct_vk(vk: u32) -> bool {
    matches!(
        vk,
        0xBA | 0xBB | 0xBC | 0xBD | 0xBE | 0xBF | 0xC0 | 0xDB | 0xDC | 0xDD | 0xDE
    )
}

/// 这个键要不要拦下来（有拼音时还要拦退格、数字、标点、回车等）
fn claim(wparam: WPARAM, has_buf: bool, english: bool) -> bool {
    let ch = wparam.0 as u8 as char;
    if ch.is_ascii_alphabetic() {
        return !english;
    }
    if !has_buf {
        return false;
    }
    match wparam.0 as u32 {
        0x20 | 0x1B | 0x08 | 0x0D | 0x09 => true, // 空格 / ESC / 退格 / 回车 / Tab
        0x31..=0x39 => true,                      // 数字选词
        0x25..=0x28 => true,                      // 方向键选词
        0x21 | 0x22 => true,                      // PageUp / PageDown
        _ => is_punct_vk(wparam.0 as u32),
    }
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
        // 静音
        Ok(())
    }
    fn OnUninitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        // 静音
        Ok(())
    }
    fn OnSetFocus(&self, _focus: Option<&ITfDocumentMgr>, _prev: Option<&ITfDocumentMgr>) -> Result<()> {
        // 焦点变化非常频繁，不能每次都写日志（会把日志刷爆）
        self.advise_key_sink();
        Ok(())
    }
    fn OnPushContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        // 静音
        Ok(())
    }
    fn OnPopContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        // 静音
        Ok(())
    }
}
// ============================ 候选窗 ============================

unsafe extern "system" fn cand_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let bg = CreateSolidBrush(COLORREF(0x00342a1e));
            FillRect(dc, &rc, bg);
            let _ = DeleteObject(bg);
            SetBkMode(dc, TRANSPARENT);
            let font = CreateFontW(-20, 0, 0, 0, 500, 0, 0, 0, 134, 0, 0, 0, 0,
                windows::core::w!("Microsoft YaHei UI"));
            let old = SelectObject(dc, font);
            let list = UI_CANDS.lock().unwrap();
            for (i, (word, ru)) in list.iter().take(9).enumerate() {
                let y = 5 + (i as i32) * 32;
                SetTextColor(dc, COLORREF(0x0066a2f7));
                let a: Vec<u16> = format!("{}", i + 1).encode_utf16().collect();
                TextOutW(dc, 12, y, &a);
                SetTextColor(dc, COLORREF(0x00f0e9e9));
                let bw: Vec<u16> = word.encode_utf16().collect();
                TextOutW(dc, 40, y, &bw);
                SetTextColor(dc, COLORREF(0x00ffa6c3));
                let br: Vec<u16> = ru.replace('|', "·").encode_utf16().collect();
                TextOutW(dc, 160, y, &br);
            }
            let _ = SelectObject(dc, old);
            let _ = DeleteObject(font);
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

unsafe fn cand_init() {
    cand::init();
    return;
    #[allow(unreachable_code)]
    let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
    let cls: Vec<u16> = "PangyiCandW\0".encode_utf16().collect();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(cand_proc),
        lpszClassName: PCWSTR(cls.as_ptr()),
        hInstance: hinst.into(),
        hbrBackground: HBRUSH(6isize),
        ..Default::default()
    };
    RegisterClassW(&wc);
    let title: Vec<u16> = "旁译\0".encode_utf16().collect();
    CAND_HWND = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
        PCWSTR(cls.as_ptr()), PCWSTR(title.as_ptr()), WS_POPUP,
        0, 0, 620, 320, None, None, hinst, None);
}

unsafe fn cand_update(x: i32, y: i32) {
    // 注意：不要再用旧的 CAND_HWND 判断——候选窗已经搬到 cand 模块里了，
    // 这里留着旧判断会导致候选窗永远不显示。
    cand::show_at(x, y);
}

unsafe fn cand_hide() { cand::hide(); }

// ============================ 组合（拼音下划线）============================

#[implement(ITfEditSession)]
struct CompSession {
    ctx: ITfContext,
    text: Vec<u16>,
    sink: ITfCompositionSink,
}

impl ITfEditSession_Impl for CompSession {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        unsafe {
            let mut sel = [TF_SELECTION::default()];
            let mut n = 0u32;
            self.ctx.GetSelection(ec, 0, &mut sel, &mut n)?;
            if n == 0 { return Ok(()); }

            let have = COMPOSITION.as_ref().is_some();
            let range = if have {
                match COMPOSITION.as_ref().unwrap().GetRange() {
                    Ok(r) => r,
                    Err(_) => return Ok(()),
                }
            } else {
                match (&*sel[0].range).as_ref() { Some(r) => r.clone(), None => return Ok(()) }
            };

            if !have {
                if let Ok(cc) = self.ctx.cast::<ITfContextComposition>() {
                    if let Ok(comp) = cc.StartComposition(ec, &range, &self.sink) {
                        COMPOSITION = Some(comp);
                    }
                }
            }
            if let Some(comp) = COMPOSITION.as_ref() {
                if let Ok(r) = comp.GetRange() {
                    let _ = r.SetText(ec, 0, &self.text);
                }
            }

            // 把候选窗放到光标下方；取不到文本范围就退回系统光标位置（否则候选窗根本不出现）
            let mut placed = false;
            if let Ok(view) = self.ctx.GetActiveView() {
                let mut rc = RECT::default();
                let mut clipped = BOOL(0);
                let r2 = COMPOSITION.as_ref().and_then(|c| c.GetRange().ok()).unwrap_or(range);
                if view.GetTextExt(ec, &r2, &mut rc, &mut clipped).is_ok() {
                    log(&format!("候选窗定位: 文本范围 {} ,{}", rc.left, rc.bottom));
                    *PENDING_CAND.lock().unwrap() = Some((rc.left, rc.bottom + 6));
                    placed = true;
                } else {
                    log("候选窗定位: GetTextExt 失败，改用光标位置");
                }
            }
            if !placed {
                let mut gi = GUITHREADINFO::default();
                gi.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
                let (mut x, mut y) = (400, 400);
                if GetGUIThreadInfo(0, &mut gi).is_ok() {
                    let mut pt = POINT::default();
                    if GetCaretPos(&mut pt).is_ok() {
                        ClientToScreen(gi.hwndCaret, &mut pt);
                        x = pt.x;
                        y = pt.y + 26;
                    }
                }
                log(&format!("候选窗定位: 光标 {x},{y}"));
                *PENDING_CAND.lock().unwrap() = Some((x, y));
            }
        }
        Ok(())
    }
}

/// 上屏：把 composition 的文字换成最终的中文，然后结束组合
#[implement(ITfEditSession)]
struct DoneSession {
    ctx: ITfContext,
    text: Vec<u16>,
}

impl ITfEditSession_Impl for DoneSession {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        unsafe {
            // 换字 + 结束组合放在同一次会话里（拆成两次会让某些宿主把文字丢掉）
            let mut wrote = false;
            if let Some(comp) = COMPOSITION.take() {
                if let Ok(r) = comp.GetRange() {
                    wrote = r.SetText(ec, 0, &self.text).is_ok();
                }
                let hr = comp.EndComposition(ec);
                log(&format!("上屏: SetText={wrote} EndComposition -> {hr:?}"));
            } else {
                log("上屏: 没有活动组合");
            }

        }
        Ok(())
    }
}

/// 只更新组合里的文字（退格时用），不结束组合
#[implement(ITfEditSession)]
struct UpdateSession {
    text: Vec<u16>,
}

impl ITfEditSession_Impl for UpdateSession {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        unsafe {
            if let Some(comp) = COMPOSITION.as_ref() {
                if let Ok(r) = comp.GetRange() {
                    let _ = r.SetText(ec, 0, &self.text);
                }
            }
        }
        Ok(())
    }
}
/// 结束组合：单独一次编辑会话。以前和“换字”挤在同一次会话里，会出现上屏成拼音或干脆不上屏。
#[implement(ITfEditSession)]
struct EndSession {
    ctx: ITfContext,
}

impl ITfEditSession_Impl for EndSession {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        unsafe {
            log(&format!("EndSession: 有组合={}", COMPOSITION.as_ref().is_some()));
            if let Some(comp) = COMPOSITION.take() {
                let hr = comp.EndComposition(ec);
                log(&format!("EndSession: EndComposition -> {hr:?}"));
            }
        }
        Ok(())
    }
}

unsafe fn end_composition() {
    if COMPOSITION.is_some() {
        // 用一个空内容的上屏会话把它收掉
        COMPOSITION = None;
    }
}