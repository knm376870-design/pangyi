//! 旁译 Pangyi —— Windows TSF 输入法（骨架）
//!
//! 里程碑 1：被 TSF 加载、收到按键、按空格把中文上屏。
//! 待办：编辑会话（ITfEditSession）、候选窗口、拼音下划线。

#![allow(non_snake_case)]

pub mod cand;
pub mod dict;
pub mod settings;

use std::cell::{Cell, RefCell};
use std::os::raw::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use std::sync::Mutex;
use windows::core::{implement, ComInterface, Result, GUID, HRESULT};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Foundation::{BOOL, CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER, LPARAM, POINT, S_OK, WPARAM};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::TextServices::{
    ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfTextInputProcessor_Impl,
    ITfClientId, ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContextComposition,
    ITfDocumentMgr, ITfEditSession, ITfEditSession_Impl, ITfTextInputProcessorEx,
    ITfTextInputProcessorEx_Impl, ITfThreadMgrEventSink, ITfThreadMgrEventSink_Impl,
    ITfSource, ITfThreadMgr, TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION,
};

static SHIFT_DOWN_AT: AtomicU32 = AtomicU32::new(0);
static SHIFT_DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static PENDING_CAND: Mutex<Option<(i32, i32)>> = Mutex::new(None);

pub const CLSID_PANGYI: GUID = GUID::from_u128(0xB4E8F2A1_7C3D_4E5F_9A21_8D6C4B3E1F07);
pub const GUID_PROFILE: GUID = GUID::from_u128(0x5A7C9E12_3D4B_4F8A_B6E1_2C9D7A5E8F34);

static LIVE: AtomicU32 = AtomicU32::new(0);
/// 当前活动组合。一个进程里可能有好几个线程在用输入法，这块是进程级状态，
/// 原来写成 static mut 属于数据竞争 —— 改成互斥量。
///
/// ITfComposition 是 Apartment 模型的 COM 对象，windows-rs 没给它实现 Send，
/// 但它只会在创建它的那个线程（TSF 的编辑会话回调线程）上被使用，
/// 真正的并发安全由里面的 Mutex 保证，所以这里包一层放行编译器。
struct CompSlot(Mutex<Option<ITfComposition>>);
unsafe impl Send for CompSlot {}
unsafe impl Sync for CompSlot {}
impl CompSlot {
    fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, Option<ITfComposition>>> {
        self.0.lock()
    }
}
static COMPOSITION: CompSlot = CompSlot(Mutex::new(None));
static UI_CANDS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// 全部候选（翻页用）以及当前页号
static ALL_CANDS: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());
static PAGE: AtomicUsize = AtomicUsize::new(0);
const PAGE_SIZE: usize = 9;
const S_FALSE_HR: HRESULT = HRESULT(1);
const CLASS_E_CLASSNOTAVAILABLE_HR: HRESULT = HRESULT(0x80040111u32 as i32);

/// 写诊断日志。默认关闭 —— 每敲一个键都写文件会明显拖慢输入。
/// 需要排查时在 %LOCALAPPDATA%\Pangyi\settings.txt 里加一行 debug=1。
fn log(msg: &str) {
    if !crate::settings::debug() {
        return;
    }
    use std::io::Write;
    let path = r"C:\ProgramData\Pangyi\ime.log";
    let _ = std::fs::create_dir_all(r"C:\ProgramData\Pangyi");
    // 别让它无限长：超过 4MB 就重新开一个
    if let Ok(m) = std::fs::metadata(path) {
        if m.len() > 4 * 1024 * 1024 {
            let _ = std::fs::remove_file(path);
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "[pid {}] {msg}", std::process::id());
    }
}

/// 热路径（每次按键）上用的日志：关掉时连字符串都不拼，零开销。
macro_rules! dlog {
    ($($arg:tt)*) => {
        if crate::settings::debug() {
            log(&format!($($arg)*));
        }
    };
}

// ============================ 文本服务 ============================

#[implement(ITfTextInputProcessorEx, ITfKeyEventSink, ITfThreadMgrEventSink, ITfCompositionSink)]
struct TextService {
    client_id: Cell<u32>,
    tid: Cell<u32>,
    /// 保持线程管理器 / 按键管理器的引用计数，别存裸指针
    tm: RefCell<Option<ITfThreadMgr>>,
    keystroke: RefCell<Option<ITfKeystrokeMgr>>,
    /// ITfThreadMgrEventSink 的订阅 cookie，Deactivate 时必须撤掉
    tmsink_cookie: Cell<u32>,
    advised: Cell<bool>,
    /// 是否处于 Activate..Deactivate 之间。只有为真才允许挂按键接收器。
    active: Cell<bool>,
    /// 上一次尝试挂接收器的时间（毫秒）。用时间节流代替"重试 N 次就放弃"：
    /// 连接点被占满时我们要一直有机会重试，但不能每来一次焦点事件就猛打一次。
    last_advise_ms: Cell<u32>,
    buf: RefCell<String>,
}

impl TextService {
    fn new() -> Self {
        Self {
            client_id: Cell::new(0),
            tid: Cell::new(0),
            tm: RefCell::new(None),
            keystroke: RefCell::new(None),
            tmsink_cookie: Cell::new(0),
            advised: Cell::new(false),
            active: Cell::new(false),
            last_advise_ms: Cell::new(0),
            buf: RefCell::new(String::new()),
        }
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


/// 记下这一轮的全部候选，回到第一页。
///
/// 注意：这里必须先把 ALL_CANDS 的锁放掉再调 refresh_page()。
/// 之前是「持着锁直接调 refresh_page()」，而 refresh_page 也要锁 ALL_CANDS ——
/// std::sync::Mutex 不可重入，于是敲第一个字母就把输入线程锁死，
/// 表现就是"切到旁译之后打不出字，得重新开一个程序才行"。
fn set_all_cands(hits: &[dict::Hit]) {
    {
        let mut g = ALL_CANDS.lock().unwrap();
        g.clear();
        g.extend(hits.iter().map(|h| (h.word.clone(), h.pinyin.clone(), h.ru.clone())));
    }
    PAGE.store(0, std::sync::atomic::Ordering::Relaxed);
    refresh_page();
}

/// 把当前页的候选交给候选窗。
/// 先在锁里把这一页拷出来，出了锁再去动 UI / 窗口，避免任何嵌套加锁。
fn refresh_page() {
    let (items, ui) = {
        let all = ALL_CANDS.lock().unwrap();
        let page = PAGE.load(std::sync::atomic::Ordering::Relaxed);
        let start = (page * PAGE_SIZE).min(all.len());
        let end = (start + PAGE_SIZE).min(all.len());
        let slice = &all[start..end];
        let items: Vec<cand::Item> = slice
            .iter()
            .map(|(w, p, r)| cand::Item { word: w.clone(), pinyin: p.clone(), ru: r.clone(), fresh: false })
            .collect();
        let ui: Vec<(String, String)> = slice.iter().map(|(w, _p, r)| (w.clone(), r.clone())).collect();
        (items, ui)
    };
    unsafe { cand::set_items(items); }
    let mut u = UI_CANDS.lock().unwrap();
    u.clear();
    u.extend(ui);
}

impl TextService {
    /// 把某个词上屏（换掉当前组合）
    fn commit_word(&self, pic: Option<&ITfContext>, word: &str) {
        dict::note_choice(word); // 自学习：下次把这个词排前面
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
            *COMPOSITION.lock().unwrap() = None;
            unsafe { cand_hide(); }
            return;
        }
        unsafe { cand::set_preedit(&key); }
        unsafe { cand::set_sentence(dict::complete(&key).map(|(w, _)| w)); }
        let hits = dict::lookup(&key);
        set_all_cands(&hits);
        if let Some(ctx) = pic {
            let s: ITfEditSession = UpdateSession { text: key.encode_utf16().collect() }.into();
            let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
            let _ = unsafe { ctx.RequestEditSession(self.client_id.get(), &s, flags) };
        }
    }
}

impl TextService {
    fn activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        // TSF 有时会连着发两次 Activate；重复挂 sink 必定失败，第二次直接忽略
        if self.active.get() {
            return Ok(());
        }
        // 先读设置：log() 要靠 settings::debug() 决定要不要写，顺序不能反
        unsafe { settings::load(); }
        log("Activate");

        // 后台预热词典索引，避免第一次打字时卡一下（只跑一次）
        static WARMED: std::sync::Once = std::sync::Once::new();
        WARMED.call_once(|| {
            std::thread::spawn(|| {
                // 词典索引 + 语言模型都在后台建好。
                // 不预热的话，用户打到第 3 个字母时才开始建 20 万条 bigram，
                // 那一下会明显卡顿，而且每换一个程序都要重来一次。
                dict::warmup();
            });
        });

        // 新会话开始：把上一次留下的状态清干净，否则上一个窗口的拼音串会漏进来
        self.buf.borrow_mut().clear();
        UI_CANDS.lock().unwrap().clear();
        ALL_CANDS.lock().unwrap().clear();
        PAGE.store(0, Ordering::Relaxed);
        *COMPOSITION.lock().unwrap() = None;
        unsafe { cand_hide(); }

        let Some(tm) = ptim else {
            log("Activate: 没拿到 ITfThreadMgr，按键接收器挂不上");
            return Ok(());
        };
        self.tid.set(tid);

        // TSF 要的是 TfClientId（用 CLSID 换），不是线程 ID。
        // 传错会导致 AdviseKeyEventSink 返回 0x80040201（事件无法调用任何订户）。
        match tm.cast::<ITfClientId>() {
            Ok(ci) => match unsafe { ci.GetClientId(&CLSID_PANGYI) } {
                Ok(cid) => {
                    self.client_id.set(cid);
                    dlog!("TfClientId={cid}");
                }
                Err(e) => dlog!("GetClientId 失败 {e:?}"),
            },
            Err(e) => dlog!("拿不到 ITfClientId: {e:?}"),
        }

        // 订阅线程管理器事件（焦点变化）。cookie 要留着，Deactivate 必须撤掉，
        // 否则停用之后焦点事件还会打到我们身上。
        if let Ok(tmsink) = unsafe { self.cast::<ITfThreadMgrEventSink>() } {
            if let Ok(src) = tm.cast::<ITfSource>() {
                match unsafe { src.AdviseSink(&ITfThreadMgrEventSink::IID, &tmsink) } {
                    Ok(cookie) => {
                        self.tmsink_cookie.set(cookie);
                        dlog!("AdviseSink(ThreadMgrEventSink) ok cookie={cookie}");
                    }
                    Err(e) => dlog!("AdviseSink 调用失败 {e:?}"),
                }
            }
        }

        *self.tm.borrow_mut() = Some(tm.clone());
        if let Ok(ks) = tm.cast::<ITfKeystrokeMgr>() {
            *self.keystroke.borrow_mut() = Some(ks);
        }

        self.active.set(true);
        self.advised.set(false);
        self.last_advise_ms.set(0);
        // 按键接收器只在这里挂 —— 这是 TSF 规定唯一正确的时机
        self.advise_key_sink();
        unsafe { cand_init(); }
        Ok(())
    }

    /// 挂按键接收器。
    ///
    /// 这里必须严格区分「已激活」和「只是焦点变了」：
    /// 老代码在 Deactivate 之后仍然会走到这里（`advised` 被重置成 false 却没真的
    /// Unadvise），于是每次焦点变化都去挂一次、每次都被 TSF 拒绝
    /// （0x80040201 = 事件无法调用任何订户），日志被刷了 1700 多条；
    /// 而且一旦某次挂载失败，这个进程里的按键就再也进不来 —— 表现就是
    /// 「输入法切过去了但打不出字，得重新开一个程序才行」。
    fn advise_key_sink(&self) {
        if !self.active.get() || self.advised.get() { return; }
        if self.client_id.get() == 0 { return; }
        // 节流 300ms：焦点事件可能来得很密，别每个都真去挂一遍
        let now = now_ms32();
        if self.last_advise_ms.get() != 0 && now.wrapping_sub(self.last_advise_ms.get()) < 300 {
            return;
        }
        self.last_advise_ms.set(now);

        let ks = self.keystroke.borrow();
        let Some(ks) = ks.as_ref() else { return };
        let Ok(sink) = (unsafe { self.cast::<ITfKeyEventSink>() }) else { return };
        let cid = self.client_id.get();

        let mut attempt = unsafe { ks.AdviseKeyEventSink(cid, &sink, BOOL(1)) };
        if let Err(e) = &attempt {
            // 0x80040201 = CONNECT_E_ADVISELIMIT：连接点已经被占满了。
            // 实测第一次挂载经常就是这个 —— 说明有上一次没撤干净的订阅还占着槽位
            // （老版本 Deactivate 从来不清订阅，所以这里会一直挂不上，
            //   该进程的按键就再也进不来 —— 也就是"切过去了但打不出字"）。
            // 对策：先用同一个 clientId 撤一次，再挂。
            dlog!("AdviseKeyEventSink 被占满，先撤再挂 cid={cid} {e:?}");
            let _ = unsafe { ks.UnadviseKeyEventSink(cid) };
            attempt = unsafe { ks.AdviseKeyEventSink(cid, &sink, BOOL(1)) };
        }
        match attempt {
            Ok(()) => {
                dlog!("AdviseKeyEventSink ok (TfClientId={cid})");
                self.advised.set(true);
            }
            Err(e) => dlog!("AdviseKeyEventSink 失败 cid={cid} {e:?}"),
        }
    }

    fn deactivate(&self) -> Result<()> {
        log("Deactivate");
        // 先落闸：之后任何焦点事件都不允许再挂接收器
        self.active.set(false);

        // 1) 撤按键接收器。老版本只把 advised 置回 false 却不撤，
        //    TSF 那边的订阅还留着，切回来时行为就不确定了。
        if self.advised.get() {
            if let Some(ks) = self.keystroke.borrow().as_ref() {
                let _ = unsafe { ks.UnadviseKeyEventSink(self.client_id.get()) };
            }
            self.advised.set(false);
        }

        // 2) 撤线程管理器事件订阅，否则停用后焦点变化还会打进来
        let cookie = self.tmsink_cookie.replace(0);
        if cookie != 0 {
            if let Some(tm) = self.tm.borrow().as_ref() {
                if let Ok(src) = tm.cast::<ITfSource>() {
                    let _ = unsafe { src.UnadviseSink(cookie) };
                }
            }
        }

        // 3) 释放引用、清会话状态
        self.keystroke.borrow_mut().take();
        self.tm.borrow_mut().take();
        self.buf.borrow_mut().clear();
        UI_CANDS.lock().unwrap().clear();
        ALL_CANDS.lock().unwrap().clear();
        PAGE.store(0, Ordering::Relaxed);
        *COMPOSITION.lock().unwrap() = None;
        unsafe { cand_hide(); }
        Ok(())
    }
}

impl ITfCompositionSink_Impl for TextService {
    fn OnCompositionTerminated(&self, _ec: u32, _c: Option<&ITfComposition>) -> Result<()> {
        *COMPOSITION.lock().unwrap() = None;
        Ok(())
    }
}

impl ITfKeyEventSink_Impl for TextService {
    fn OnSetFocus(&self, fg: BOOL) -> Result<()> {
        if fg.as_bool() {
            // 焦点回来了：只在「已激活但接收器没挂上」时补挂一次（自愈）。
            // 已经挂好就是空操作 —— 老代码在这里无条件重挂，是失败刷屏的来源。
            self.advise_key_sink();
        } else {
            // 只收候选窗；不要动组合——实测清组合会让上屏丢字
            unsafe { cand_hide(); }
        }
        Ok(())
    }

    fn OnTestKeyDown(&self, _pic: Option<&ITfContext>, wparam: WPARAM, _lp: LPARAM) -> Result<BOOL> {
        // 带 Ctrl / Alt / Win 的组合键一律不碰，原样交给应用和系统
        if command_modifier_down() {
            dlog!("KeySink::OnTestKeyDown wparam={} 有修饰键，放行", wparam.0);
            return Ok(BOOL(0));
        }
        let ch0 = wparam.0 as u8 as char;
        if ch0.is_ascii_alphabetic() && crate::settings::english() {
            return Ok(BOOL(0));
        }
        let has_buf = !self.buf.borrow().is_empty();
        let want = claim(wparam, has_buf, crate::settings::english());
        dlog!("KeySink::OnTestKeyDown wparam={} 我要={want}", wparam.0);
        Ok(BOOL(want as i32))
    }
    fn OnTestKeyUp(&self, _pic: Option<&ITfContext>, w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        dlog!("KeySink::OnTestKeyUp wparam={}", w.0);
        Ok(BOOL(0))
    }
    fn OnKeyUp(&self, _pic: Option<&ITfContext>, w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        // 单独按一下 Shift（没夹着别的键）= 切中/英
        if w.0 == 0x10 {
            let t0 = SHIFT_DOWN_AT.load(Ordering::Relaxed);
            let dt = now_ms32().wrapping_sub(t0);
            if !SHIFT_DIRTY.load(Ordering::Relaxed) && t0 != 0 && dt < 400 {
                status_toggle_english();
            }
        }
        Ok(BOOL(0))
    }
    fn OnPreservedKey(&self, _pic: Option<&ITfContext>, g: *const GUID) -> Result<BOOL> {
        dlog!("KeySink::OnPreservedKey guid={g:?}");
        Ok(BOOL(0))
    }

    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, _lp: LPARAM) -> Result<BOOL> {
        // 双保险：带 Ctrl / Alt / Win 的键直接放行，绝不进组合
        if command_modifier_down() {
            return Ok(BOOL(0));
        }
        let ch = wparam.0 as u8 as char;
        dlog!("KeySink::OnKeyDown wparam={} ch={ch:?}", wparam.0);

        // 单独按一下 Shift = 切换中/英（没有状态条了，用这个切）
        if wparam.0 == 0x10 {
            SHIFT_DOWN_AT.store(now_ms32(), Ordering::Relaxed);
            SHIFT_DIRTY.store(false, Ordering::Relaxed);
            return Ok(BOOL(0));
        }
        SHIFT_DIRTY.store(true, Ordering::Relaxed);

        // 字母：进组合
        if ch.is_ascii_alphabetic() {
            if crate::settings::english() { return Ok(BOOL(0)); }
            self.buf.borrow_mut().push(ch.to_ascii_lowercase());
            let key = self.buf.borrow().clone();
            unsafe { cand::set_preedit(&key); }
            unsafe { cand::set_sentence(dict::complete(&key).map(|(w, _)| w)); }
            let hits = dict::lookup(&key);
            set_all_cands(&hits);
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
            // Shift+数字 让应用去输入 ! @ # 等符号
            if shift_down() { return Ok(BOOL(0)); }
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
            dlog!("上屏 word={:?}", w);
            if let Some(w) = w { self.commit_word(pic, &w); }
            return Ok(BOOL(1));
        }

        // ESC：取消这次输入
        if wparam.0 == 0x1B {
            if !has_buf { return Ok(BOOL(0)); }
            self.buf.borrow_mut().clear();
            UI_CANDS.lock().unwrap().clear();
            unsafe { cand_hide(); }
            *COMPOSITION.lock().unwrap() = None;
            return Ok(BOOL(1));
        }

        // PageUp / PageDown：翻页
        if matches!(wparam.0 as u32, 0x21 | 0x22) {
            if !has_buf { return Ok(BOOL(0)); }
            let total = ALL_CANDS.lock().unwrap().len();
            let pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;
            let cur = PAGE.load(std::sync::atomic::Ordering::Relaxed);
            let next = if (wparam.0 as u32) == 0x21 {
                cur.saturating_sub(1)
            } else {
                (cur + 1).min(pages.saturating_sub(1))
            };
            if next != cur {
                PAGE.store(next, std::sync::atomic::Ordering::Relaxed);
                refresh_page();
            }
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
fn now_ms32() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_millis() & 0xFFFF_FFFF) as u32)
        .unwrap_or(0)
}

/// 切换中/英（原来由状态条上的按钮做）
fn status_toggle_english() {
    crate::settings::set_english(!crate::settings::english());
}

/// 某个虚拟键现在是不是按下的
fn key_down(vk: i32) -> bool {
    unsafe { (windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(vk) as u16 & 0x8000) != 0 }
}

/// Ctrl / Alt / Win 按住时，输入法必须【完全放行】。
///
/// 这是之前一个很严重的问题：claim() 只看键码不看修饰键，
/// 而 Ctrl+C 的 C、Ctrl+V 的 V、Alt+F4 的 F4(0x73='s')、Win+D 的 D 都是"字母"，
/// 于是这些系统快捷键全被输入法当成"要打拼音"吞掉了 ——
/// 复制粘贴、全选、撤销、保存、关闭窗口、Win 组合键统统失效。
fn command_modifier_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN};
    key_down(VK_CONTROL.0 as i32)
        || key_down(VK_MENU.0 as i32)
        || key_down(VK_LWIN.0 as i32)
        || key_down(VK_RWIN.0 as i32)
}

fn shift_down() -> bool {
    key_down(windows::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT.0 as i32)
}

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
        // 数字选词。Shift+数字 是打 ! @ # $ % 这类符号，不能抢
        0x31..=0x39 => !shift_down(),
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
                Ok(()) => dlog!("DoEditSession 上屏成功 {} 字", self.text.len()),
                Err(e) => dlog!("DoEditSession SetText 失败 {e:?}"),
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
        // 焦点变化非常频繁，不能写日志（会把日志刷爆）。
        // 注意：这里的重挂只在 active && !advised 时才会真的发生，见 advise_key_sink。
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

/// 候选窗实现已经全部搬进 cand 模块；这里只留一个入口。
unsafe fn cand_init() {
    cand::init();
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

            let have = COMPOSITION.lock().unwrap().is_some();
            let range = if have {
                let g = COMPOSITION.lock().unwrap();
                match g.as_ref().unwrap().GetRange() {
                    Ok(r) => r,
                    Err(_) => return Ok(()),
                }
            } else {
                match (&*sel[0].range).as_ref() { Some(r) => r.clone(), None => return Ok(()) }
            };

            if !have {
                if let Ok(cc) = self.ctx.cast::<ITfContextComposition>() {
                    if let Ok(comp) = cc.StartComposition(ec, &range, &self.sink) {
                        *COMPOSITION.lock().unwrap() = Some(comp);
                    }
                }
            }
            {
                let g = COMPOSITION.lock().unwrap();
                if let Some(comp) = g.as_ref() {
                    if let Ok(r) = comp.GetRange() {
                        let _ = r.SetText(ec, 0, &self.text);
                    }
                }
            }

            // 把候选窗放到光标下方；取不到文本范围就退回系统光标位置（否则候选窗根本不出现）
            let mut placed = false;
            if let Ok(view) = self.ctx.GetActiveView() {
                let mut rc = RECT::default();
                let mut clipped = BOOL(0);
                let r2 = COMPOSITION.lock().unwrap().as_ref().and_then(|c| c.GetRange().ok()).unwrap_or(range);
                if view.GetTextExt(ec, &r2, &mut rc, &mut clipped).is_ok() {
                    dlog!("候选窗定位: 文本范围 {} ,{}", rc.left, rc.bottom);
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
                dlog!("候选窗定位: 光标 {x},{y}");
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
            if let Some(comp) = COMPOSITION.lock().unwrap().take() {
                if let Ok(r) = comp.GetRange() {
                    wrote = r.SetText(ec, 0, &self.text).is_ok();
                }
                let hr = comp.EndComposition(ec);
                dlog!("上屏: SetText={wrote} EndComposition -> {hr:?}");
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
            let g = COMPOSITION.lock().unwrap();
            if let Some(comp) = g.as_ref() {
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
            let mut g = COMPOSITION.lock().unwrap();
            dlog!("EndSession: 有组合={}", g.is_some());
            if let Some(comp) = g.take() {
                let hr = comp.EndComposition(ec);
                dlog!("EndSession: EndComposition -> {hr:?}");
            }
        }
        Ok(())
    }
}

unsafe fn end_composition() {
    *COMPOSITION.lock().unwrap() = None;
}