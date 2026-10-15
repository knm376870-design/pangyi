//! 旁译 Pangyi —— Windows TSF 输入法（骨架）
//!
//! 里程碑 1：被 TSF 加载、收到按键、按空格把中文上屏。
//! 待办：编辑会话（ITfEditSession）、候选窗口、拼音下划线。

#![allow(non_snake_case)]

pub mod cand;
pub mod dict;
pub mod settings;

#[cfg(test)]
mod stability_tests;

use std::cell::{Cell, RefCell};
use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

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

/// COM 对象存活计数。
///
/// 之前 LIVE 从来没被加过：DllCanUnloadNow 恒返回 S_OK，COM 运行时
/// （CoFreeUnusedLibraries）空闲时真可能把 DLL 卸掉，而 TextService 实例还活着，
/// 宿主进程就会崩。现在每建一个 TextService 加一，对象销毁时（LiveGuard 的 Drop）减一。
static LIVE: AtomicU32 = AtomicU32::new(0);

struct LiveGuard;
impl Drop for LiveGuard {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}
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
/// 整句翻译：当前句子缓冲（自上个句末标点/回车以来已上屏的中文词序列）。
/// 只在整句模式（trmode=2）里写；句末收尾后清空。
/// 存词不存串 —— 翻译按用户上屏的词边界逐词查，不做重切分。
static SENT: Mutex<Vec<String>> = Mutex::new(Vec::new());
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
    /// COM 存活标记：对象销毁时给 LIVE 减一（见 LIVE 的注释）
    _live: LiveGuard,
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
        LIVE.fetch_add(1, Ordering::SeqCst);
        Self {
            _live: LiveGuard,
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
fn set_all_cands(hits: Vec<dict::Hit>) {
    {
        let mut g = ALL_CANDS.lock().unwrap();
        g.clear();
        // 直接把 lookup 的结果移进来（原来是逐条 clone），
        // 候选池放大到几百条之后这一步不能再白拷一份
        g.extend(hits.into_iter().map(|h| (h.word, h.pinyin, h.ru)));
    }
    PAGE.store(0, std::sync::atomic::Ordering::Relaxed);
    refresh_page();
}

/// 把当前页的候选交给候选窗。
/// 先在锁里把这一页拷出来，出了锁再去动 UI / 窗口，避免任何嵌套加锁。
fn refresh_page() {
    // 页大小 = 「候选个数」设置：翻页、候选窗显示、数字选词都用同一份，
    // 否则设成 5/7 时翻页会跳过没显示的候选，数字还能选到窗口上没有的条目
    let size = crate::settings::max_cands();
    let (items, ui, page, pages) = {
        let all = ALL_CANDS.lock().unwrap();
        let pages = ((all.len() + size - 1) / size).max(1);
        let page = PAGE
            .load(std::sync::atomic::Ordering::Relaxed)
            .min(pages - 1);
        let start = (page * size).min(all.len());
        let end = (start + size).min(all.len());
        let slice = &all[start..end];
        let items: Vec<cand::Item> = slice
            .iter()
            .map(|(w, p, r)| cand::Item { word: w.clone(), pinyin: p.clone(), ru: r.clone(), fresh: false })
            .collect();
        let ui: Vec<(String, String)> = slice.iter().map(|(w, _p, r)| (w.clone(), r.clone())).collect();
        (items, ui, page, pages)
    };
    unsafe {
        cand::set_items(items);
        // 页码给候选窗画右下角那个「▲▼ 2/59」
        cand::set_page(page, pages);
    }
    let mut u = UI_CANDS.lock().unwrap();
    u.clear();
    u.extend(ui);
}

/// 翻页：delta = -1 上一页 / +1 下一页。返回是否真的翻动了
///（已经在头/尾页就返回 false，按键当没发生）。
/// 微软拼音同款按键：PageUp/PageDown、`-`/`=`，上下方向键撞到页边界也翻。
fn flip_page(delta: isize) -> bool {
    let total = ALL_CANDS.lock().unwrap().len();
    if total == 0 {
        return false;
    }
    let size = crate::settings::max_cands();
    let pages = (total + size - 1) / size;
    let cur = PAGE.load(std::sync::atomic::Ordering::Relaxed);
    let next = if delta < 0 {
        cur.saturating_sub(1)
    } else {
        (cur + 1).min(pages.saturating_sub(1))
    };
    if next == cur {
        return false;
    }
    PAGE.store(next, std::sync::atomic::Ordering::Relaxed);
    refresh_page();
    true
}

/// 在候选里移动一格：delta = -1 上一格 / +1 下一格；撞到页边界自动翻页
/// （第 1 个再往上 = 上一页的最后一条，最后一个再往下 = 下一页第 1 条，
///   和微软拼音一样看着一页一页连起来翻）。调用前要先确认当前页有候选。
fn move_sel(delta: isize) {
    let n = UI_CANDS.lock().unwrap().len();
    let cur = unsafe { cand::current_index() };
    if delta < 0 && cur == 0 {
        if flip_page(-1) {
            // 翻到上一页后选最后一条（连续往上按不会卡住）
            let last = unsafe { cand::count() }.saturating_sub(1);
            unsafe { cand::select(last); }
        }
        return;
    }
    if delta > 0 && cur + 1 >= n {
        if flip_page(1) {
            unsafe { cand::select(0); }
        }
        return;
    }
    let next = if delta < 0 { cur - 1 } else { cur + 1 };
    unsafe { cand::select(next); }
}

impl TextService {
    /// 把文本写进文档（换掉当前组合），不记录自学习。
    /// 用于「候选 + 标点 / 候选 + 大写字母」这类拼接上屏 —— 不能把拼接串
    /// 喂给 note_choice，否则 learned.txt 里会混进「你好,」「你H」这种脏词条。
    fn commit_text(&self, pic: Option<&ITfContext>, text: &str) {
        if let Some(ctx) = pic {
            let session: ITfEditSession = DoneSession {
                ctx: ctx.clone(),
                text: text.encode_utf16().collect(),
            }
            .into();
            let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
            let _ = unsafe { ctx.RequestEditSession(self.client_id.get(), &session, flags) };
        }
    }

    /// ←/→ 选中「俄语侧」时，上屏内容换成该词的俄语第一释义；
    /// 没译文的词照旧上中文（它的俄语列本来就是空的）。
    fn pick_text(word: &str) -> String {
        if unsafe { cand::ru_sel() } && !crate::settings::horizontal() {
            dict::ru_of(word).map(|r| r.to_string()).unwrap_or_else(|| word.to_string())
        } else {
            word.to_string()
        }
    }

    /// 把某个词上屏（换掉当前组合）
    fn commit_word(&self, pic: Option<&ITfContext>, word: &str) {
        dict::note_choice(word); // 自学习：下次把这个词排前面
        // v4.5.0 选中俄语侧：只上屏俄语，中文不输入（逐字翻译不再重复拼）
        // v4.4.0 逐字翻译：俄语第一释义跟在词后面；整句模式：记进句子缓冲
        dlog!("commit_word word={word} ru_sel={} horizontal={}", unsafe { cand::ru_sel() }, crate::settings::horizontal());
        let text = if unsafe { cand::ru_sel() } && !crate::settings::horizontal() {
            Self::pick_text(word)
        } else if crate::settings::trmode() == 1 {
            match dict::ru_of(word) {
                Some(ru) => format!("{word} {ru}"),
                None => word.to_string(),
            }
        } else {
            word.to_string()
        };
        self.commit_text(pic, &text);
        sent_push(word);
    }

    /// 更新组合内容为当前的拼音串（用于退格）
    fn refresh_composition(&self, pic: Option<&ITfContext>) {
        let key = self.buf.borrow().clone();
        if key.is_empty() {
            // 删到空了：把组合真正结束掉，否则文档里会残留最后一个拼音字母
            // （直接置 None 只是丢掉引用，组合文字还在文档里，见 EndSession 的注释）
            unsafe { cand_hide(); }
            if let Some(ctx) = pic {
                let s: ITfEditSession = EndSession.into();
                let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
                let _ = unsafe { ctx.RequestEditSession(self.client_id.get(), &s, flags) };
            } else {
                *COMPOSITION.lock().unwrap() = None;
            }
            return;
        }
        unsafe { cand::set_preedit(&key); }
        set_all_cands(dict::lookup(&key));
        sentence_hint(&key);
        // 候选窗重画：set_items 只改内存，不触发 WM_PAINT。
        // 缺这一句的话，退格后候选窗还停留在旧拼音的候选上（v4.2.0 的 bug：
        // 删一个字母候选不变，删光才一起消失 —— 只有 cand_hide 生效）。
        if let Some((x, y)) = *PENDING_CAND.lock().unwrap() {
            unsafe { cand_update(x, y); }
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
        // TSF 有时会连着发两次 Activate；重复挂 sink 必定失败，第二次直接忽略
        if self.active.get() {
            return Ok(());
        }
        // 先读设置：log() 要靠 settings::debug() 决定要不要写，顺序不能反
        unsafe { settings::load(); }
        log(concat!("Activate v", env!("CARGO_PKG_VERSION")));
        log(&format!(
            "cnpunct={} english={} trmode={}",
            crate::settings::cnpunct(),
            crate::settings::english(),
            crate::settings::trmode()
        ));

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
        SENT.lock().unwrap().clear();
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
        SENT.lock().unwrap().clear();
        unsafe { cand_hide(); }
        Ok(())
    }
}

impl ITfCompositionSink_Impl for TextService {
    fn OnCompositionTerminated(&self, _ec: u32, c: Option<&ITfComposition>) -> Result<()> {
        // 宿主取消组合后，拼音缓冲与候选必须一起失效。主动上屏/取消时组合已被
        // take()，回调不应再清句子缓冲；旧组合的延迟通知也不能取消新组合。
        // 接口相等比较会 QueryInterface，必须在组合锁之外做。
        let current = COMPOSITION.lock().unwrap().clone();
        let Some(current) = current else { return Ok(()); };
        if c != Some(&current) { return Ok(()); }
        let ended = {
            let mut slot = COMPOSITION.lock().unwrap();
            // 比较 COM 身份期间若发生重入，也不能取走后来替换的新组合。
            if slot.as_ref().map(windows::core::Interface::as_raw)
                != Some(windows::core::Interface::as_raw(&current))
            {
                return Ok(());
            }
            slot.take()
        };
        drop(ended);
        self.buf.borrow_mut().clear();
        UI_CANDS.lock().unwrap().clear();
        ALL_CANDS.lock().unwrap().clear();
        PAGE.store(0, Ordering::Relaxed);
        *PENDING_CAND.lock().unwrap() = None;
        SENT.lock().unwrap().clear();
        unsafe {
            cand::set_preedit("");
            cand::set_sentence(None);
            cand::set_items(Vec::new());
            cand::set_page(0, 1);
            cand_hide();
        }
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
            SENT.lock().unwrap().clear(); // 换焦点：没说完的句子作废
        }
        Ok(())
    }

    fn OnTestKeyDown(&self, _pic: Option<&ITfContext>, wparam: WPARAM, _lp: LPARAM) -> Result<BOOL> {
        // Shift 的按下/抬起在测试回调里直接记，不靠 OnKeyDown/OnKeyUp：
        // 按 TSF 契约，OnKeyDown/OnKeyUp 只在测试回调返回 TRUE 时才会被回调，
        // 而 Shift 不能被 claim（会吞掉 Shift+字母、Shift+方向键）。
        // 测试回调对每个按键都会问一遍，所以放这里最稳。
        if wparam.0 == 0x10 {
            SHIFT_DOWN_AT.store(now_ms32(), Ordering::Relaxed);
            // 只有「单独」按 Shift 才算候选切换：有别的修饰键按着（Ctrl+Shift 等）就记脏
            SHIFT_DIRTY.store(command_modifier_down(), Ordering::Relaxed);
            dlog!("KeySink::OnTestKeyDown wparam=16 Shift键 async_shift={} async_caps={}", shift_down(), caps_on());
            return Ok(BOOL(0));
        }
        SHIFT_DIRTY.store(true, Ordering::Relaxed);
        // Ctrl+. 切换中文标点（微软同款快捷键）。必须放在「带修饰键放行」之前，
        // 否则永远到不了 OnKeyDown。
        if wparam.0 == 0xBE && ctrl_only() {
            dlog!("KeySink::OnTestKeyDown Ctrl+. 切中文标点");
            return Ok(BOOL(1));
        }
        // Ctrl+, 循环切换翻译模式（关 -> 逐字 -> 整句 -> 关），和 Ctrl+. 同款思路
        if wparam.0 == 0xBC && ctrl_only() {
            dlog!("KeySink::OnTestKeyDown Ctrl+, 切翻译模式");
            return Ok(BOOL(1));
        }
        // 带 Ctrl / Alt / Win 的组合键一律不碰，原样交给应用和系统
        if command_modifier_down() {
            dlog!("KeySink::OnTestKeyDown wparam={} 有修饰键，放行", wparam.0);
            return Ok(BOOL(0));
        }
        let letter = letter_key(wparam);
        if letter.is_some() && crate::settings::english() {
            return Ok(BOOL(0));
        }
        // Shift / Caps + 字母：大写英文直通（微软一致 —— 用户原话「不能输入
        // 大写英文字母」）。没有拼音时直接放行，应用按键盘状态输出大写字母；
        // 有拼音时先 claim，OnKeyDown 里提交首选候选再放行（微软：拼音中间
        // 按 Shift+字母 = 首选上屏 + 大写字母）。
        if letter.is_some() && (shift_down() || caps_on()) {
            dlog!("KeySink::OnTestKeyDown wparam={} shift={} caps={} 大写分支", wparam.0, shift_down(), caps_on());
            let has_buf = !self.buf.borrow().is_empty();
            return Ok(BOOL(has_buf as i32));
        }
        let has_buf = !self.buf.borrow().is_empty();
        let want = claim(wparam, has_buf, crate::settings::english());
        dlog!("KeySink::OnTestKeyDown wparam={} 我要={want} shift={} caps={}", wparam.0, shift_down(), caps_on());
        Ok(BOOL(want as i32))
    }
    fn OnTestKeyUp(&self, _pic: Option<&ITfContext>, w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        // 单独按一下 Shift（没夹着别的键、400ms 内松开）= 切中/英。
        // 放在测试回调而不是 OnKeyUp，原因见 OnTestKeyDown 的注释。
        if w.0 == 0x10 {
            let t0 = SHIFT_DOWN_AT.load(Ordering::Relaxed);
            let dt = now_ms32().wrapping_sub(t0);
            if !SHIFT_DIRTY.load(Ordering::Relaxed) && t0 != 0 && dt < 400 {
                status_toggle_english();
            }
        }
        Ok(BOOL(0))
    }
    fn OnKeyUp(&self, _pic: Option<&ITfContext>, _w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        // Shift 切换已经挪到 OnTestKeyUp（见上面的注释），这里不再处理
        Ok(BOOL(0))
    }
    fn OnPreservedKey(&self, _pic: Option<&ITfContext>, g: *const GUID) -> Result<BOOL> {
        dlog!("KeySink::OnPreservedKey guid={g:?}");
        Ok(BOOL(0))
    }

    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, _lp: LPARAM) -> Result<BOOL> {
        // Ctrl+. 切换中文标点（微软同款快捷键）。OnTestKeyDown 已经 claim 过，
        // 这里直接执行；组合保持原样（微软也不会因它上屏）。
        if wparam.0 == 0xBE && ctrl_only() {
            crate::settings::set_cnpunct(!crate::settings::cnpunct());
            dlog!("KeySink::OnKeyDown Ctrl+. 中文标点 -> {}", crate::settings::cnpunct());
            return Ok(BOOL(1));
        }
        // Ctrl+, 循环切换翻译模式（关 -> 逐字 -> 整句 -> 关）。
        // 离开整句模式时句子缓冲作废 —— 不然攒着的半句话会带到下一个模式。
        if wparam.0 == 0xBC && ctrl_only() {
            let next = (crate::settings::trmode() + 1) % 3;
            crate::settings::set_trmode(next);
            if next != 2 {
                SENT.lock().unwrap().clear();
            }
            dlog!("KeySink::OnKeyDown Ctrl+, 翻译模式 -> {next}");
            return Ok(BOOL(1));
        }
        // 双保险：带 Ctrl / Alt / Win 的键直接放行，绝不进组合
        if command_modifier_down() {
            return Ok(BOOL(0));
        }
        let letter = letter_key(wparam);
        dlog!("KeySink::OnKeyDown wparam={} letter={letter:?}", wparam.0);

        // Shift/Caps 大写字母：首选候选上屏 + 大写字母自己写进文档，然后吃掉
        // 按键（BOOL(1)）。不能走「提交后 return BOOL(0) 让应用输出」——实测
        // （fix_e2e 探针）TSF 下 claim 过的键 OnKeyDown 返回 FALSE 也到不了
        // 应用，标点分支原来就吃这个亏。没有拼音时 OnTestKeyDown 不 claim，
        // 键直接放行由应用输出大写，走不到这里。
        if letter.is_some() && (shift_down() || caps_on()) {
            dlog!("KeySink::OnKeyDown wparam={} shift={} caps={} buf={:?} 大写分支", wparam.0, shift_down(), caps_on(), self.buf.borrow());
            if crate::settings::english() {
                return Ok(BOOL(0));
            }
            if !self.buf.borrow().is_empty() {
                let w = unsafe { cand::current() }
                    .or_else(|| UI_CANDS.lock().unwrap().first().map(|(w, _)| w.clone()));
                self.buf.borrow_mut().clear();
                UI_CANDS.lock().unwrap().clear();
                unsafe { cand_hide(); }
                let letter = letter.unwrap();
                match w {
                    Some(w) => {
                        dict::note_choice(&w);
                        sent_push(&w);
                        // v4.5.0：选中俄语侧时大写字母也跟在俄语后面
                        self.commit_text(pic, &format!("{}{letter}", Self::pick_text(&w)));
                    }
                    // 没有候选：组合里只剩拼音，直接换成大写字母上屏
                    None => self.commit_text(pic, &letter.to_string()),
                }
            }
            return Ok(BOOL(1));
        }

        // 字母：进组合
        if let Some(ch) = letter {
            if crate::settings::english() { return Ok(BOOL(0)); }
            self.buf.borrow_mut().push(ch.to_ascii_lowercase());
            let key = self.buf.borrow().clone();
            unsafe { cand::set_preedit(&key); }
            set_all_cands(dict::lookup(&key));
            sentence_hint(&key);
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
            // 理论上 claim() 只会放行当前页里真有的候选，这里再兜一次底
            let Some(w) = word else { return Ok(BOOL(0)); };
            self.buf.borrow_mut().clear();
            UI_CANDS.lock().unwrap().clear();
            unsafe { cand_hide(); }
            self.commit_word(pic, &w);
            return Ok(BOOL(1));
        }

        // 方向键 / Tab：在候选里移动；撞到页边界自动翻页
        // （第 1 个再往上 = 上一页的最后一条，最后一个再往下 = 下一页第 1 条，
        //   和微软拼音一样看着一页一页连起来翻）
        if matches!(wparam.0 as u32, 0x26 | 0x28 | 0x09) {
            if !has_buf { return Ok(BOOL(0)); }
            // 没有候选可动就让应用正常处理方向键/Tab（移动光标、切焦点）
            if UI_CANDS.lock().unwrap().len() == 0 { return Ok(BOOL(0)); }
            let up = (wparam.0 as u32) == 0x26;
            move_sel(if up { -1 } else { 1 });
            return Ok(BOOL(1));
        }

        // ← / →：竖排时在「中文 / 俄语」两边之间切换 —— 蓝条挪到俄语列，
        // 上屏就只出俄语（中文不用输入）；再按 ← 挪回中文侧。
        // 横排没有左右两栏，←→ 就当作 ↑↓ 移动用。
        if matches!(wparam.0 as u32, 0x25 | 0x27) {
            if !has_buf { return Ok(BOOL(0)); }
            if UI_CANDS.lock().unwrap().len() == 0 { return Ok(BOOL(0)); }
            if crate::settings::horizontal() {
                let right = (wparam.0 as u32) == 0x27;
                move_sel(if right { 1 } else { -1 });
            } else {
                unsafe { cand::set_ru_sel((wparam.0 as u32) == 0x27); }
            }
            dlog!("←/→ wparam={} 现在 ru_sel={}", wparam.0, unsafe { cand::ru_sel() });
            return Ok(BOOL(1));
        }

        // 整句翻译：没拼音时按回车 = 句子收尾，整句俄语跟上，回车自己吞掉
        // （用户再按一次回车才是发送/换行）。claim() 保证缓冲非空才进得来。
        if wparam.0 == 0x0D && !has_buf {
            if let Some(t) = sent_close("", None) {
                self.commit_text(pic, &t);
                return Ok(BOOL(1));
            }
            return Ok(BOOL(0));
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

        // ESC：取消这次输入（组合要真正结束掉，见 EndSession 的注释）
        if wparam.0 == 0x1B {
            if !has_buf { return Ok(BOOL(0)); }
            self.buf.borrow_mut().clear();
            UI_CANDS.lock().unwrap().clear();
            unsafe { cand_hide(); }
            if let Some(ctx) = pic {
                let s: ITfEditSession = EndSession.into();
                let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
                let _ = unsafe { ctx.RequestEditSession(self.client_id.get(), &s, flags) };
            } else {
                *COMPOSITION.lock().unwrap() = None;
            }
            return Ok(BOOL(1));
        }

        // PageUp / PageDown：翻页
        if matches!(wparam.0 as u32, 0x21 | 0x22) {
            if !has_buf { return Ok(BOOL(0)); }
            flip_page(if (wparam.0 as u32) == 0x21 { -1 } else { 1 });
            return Ok(BOOL(1));
        }

        // - / = ：翻页（微软拼音同款）。注意它们在 is_punct_vk 名单里，
        // 这一段必须放在下面「标点上屏」分支之前，否则会被当成标点提交上屏
        if matches!(wparam.0 as u32, 0xBD | 0xBB) {
            if !has_buf { return Ok(BOOL(0)); }
            flip_page(if (wparam.0 as u32) == 0xBD { -1 } else { 1 });
            return Ok(BOOL(1));
        }

        // 标点：先把当前候选上屏，标点字符自己写进文档，然后吃掉按键。
        // 理由同大写分支：claim 过的键 TSF 不会再交给应用，原来「return
        // BOOL(0) 放行」实测到不了应用（候选上屏了、标点被吞）。
        if has_buf && is_punct_vk(wparam.0 as u32) {
            let w = unsafe { cand::current() }
                .or_else(|| UI_CANDS.lock().unwrap().first().map(|(w, _)| w.clone()));
            // v4.3.0：中文标点开且没按 Shift 时，标点转全角；Shift+标点 保持半角
            let c = vk_to_char(wparam.0 as u32).map(|c| {
                if crate::settings::cnpunct() && !crate::settings::english() && !shift_down() {
                    punct_full(c).unwrap_or(c)
                } else {
                    c
                }
            });
            self.buf.borrow_mut().clear();
            UI_CANDS.lock().unwrap().clear();
            unsafe { cand_hide(); }
            match (w, c) {
                (Some(w), Some(c)) => {
                    dict::note_choice(&w);
                    // v4.5.0：选中俄语侧时只上屏俄语+标点，中文词照旧进句子缓冲
                    if unsafe { cand::ru_sel() } && !crate::settings::horizontal() {
                        self.commit_text(pic, &format!("{}{c}", Self::pick_text(&w)));
                        sent_push(&w);
                    // 逐字翻译：俄语第一释义夹在词和标点之间
                    } else if crate::settings::trmode() == 1 {
                        let ru = dict::ru_of(&w).map(|r| format!(" {r}")).unwrap_or_default();
                        self.commit_text(pic, &format!("{w}{ru}{c}"));
                    } else {
                        // 整句模式：词进句子缓冲；句末标点触发整句翻译收尾
                        sent_push(&w);
                        if let Some(t) = sent_close(&w, Some(c)) {
                            self.commit_text(pic, &t);
                        } else {
                            self.commit_text(pic, &format!("{w}{c}"));
                        }
                    }
                }
                (Some(w), None) => self.commit_word(pic, &w),
                (None, Some(c)) => {
                    if let Some(t) = sent_close("", Some(c)) {
                        self.commit_text(pic, &t);
                    } else {
                        self.commit_text(pic, &c.to_string());
                    }
                }
                (None, None) => {
                    // 什么都没得写：像 ESC 一样把组合真正结束掉
                    if let Some(ctx) = pic {
                        let s: ITfEditSession = EndSession.into();
                        let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
                        let _ = unsafe { ctx.RequestEditSession(self.client_id.get(), &s, flags) };
                    } else {
                        *COMPOSITION.lock().unwrap() = None;
                    }
                }
            }
            return Ok(BOOL(1));
        }

        // 中文标点（智能全角）：无拼音时标点键直接转全角上屏，不进组合。
        // claim() 只在 中文模式 + 开关开 + 无 Shift 时拦标点（- = ` 不拦），
        // 整句模式还会额外拦句末标点（收尾用）—— 这里兜底照做一遍：
        // 拿不到字符就放行给应用。
        if !has_buf && is_punct_vk(wparam.0 as u32) {
            if let Some(c) = vk_to_char(wparam.0 as u32) {
                let c = if crate::settings::cnpunct() && !shift_down() {
                    punct_full(c).unwrap_or(c)
                } else {
                    c
                };
                // 整句模式：句末标点 = 收尾，整句俄语跟在标点后上屏
                if let Some(t) = sent_close("", Some(c)) {
                    self.commit_text(pic, &t);
                } else {
                    self.commit_text(pic, &c.to_string());
                }
                return Ok(BOOL(1));
            }
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

/// 引号配对：' 第一次出左引号，第二次出右引号（和微软一致，简单交替）。
static QUOTE_OPEN: AtomicBool = AtomicBool::new(false);

/// 半角标点 → 全角中文标点。没有全角对应的（- = ` 等）返回 None，原样写。
fn punct_full(c: char) -> Option<char> {
    Some(match c {
        ',' => '，',
        '.' => '。',
        ';' => '；',
        '/' => '、',
        '\\' => '、',
        '[' => '【',
        ']' => '】',
        '\'' => {
            if QUOTE_OPEN.fetch_xor(true, Ordering::Relaxed) { '’' } else { '‘' }
        }
        _ => return None,
    })
}

/// 只有 Ctrl 按着（Alt / Win 没按）—— Ctrl+. 切中文标点用。
fn ctrl_only() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN};
    key_down(VK_CONTROL.0 as i32)
        && !key_down(VK_MENU.0 as i32)
        && !key_down(VK_LWIN.0 as i32)
        && !key_down(VK_RWIN.0 as i32)
}

/// 虚拟键码 → 当前键盘布局下（含 Shift/Caps 状态）实际会打出的字符。
/// 自己往文档里写标点时用：布局必须是前台线程正在用的那个，字符才和
/// 应用自己按键打出来的一致（中文布局下 Shift+逗号 是《不是 <）。
fn vk_to_char(vk: u32) -> Option<char> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardLayout, MapVirtualKeyW, ToUnicodeEx, MAPVK_VK_TO_VSC, VK_CAPITAL, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let scan = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC);
        // 返回值才是线程 id（出参写的是进程 id），GetKeyboardLayout 要线程 id
        let hkl = GetKeyboardLayout(GetWindowThreadProcessId(GetForegroundWindow(), None));
        let mut state = [0u8; 256];
        if shift_down() {
            state[VK_SHIFT.0 as usize] = 0x80;
        }
        if caps_on() {
            state[VK_CAPITAL.0 as usize] = 0x01;
        }
        let mut buf = [0u16; 4];
        let n = ToUnicodeEx(vk, scan, &state, &mut buf, 0, hkl);
        if n == 1 {
            char::from_u32(buf[0] as u32)
        } else {
            None
        }
    }
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

/// 某个虚拟键现在是不是按下的。
/// 必须用 GetAsyncKeyState：GetKeyState 只反映「调用线程消息队列最后一次
/// 取消息时」的键盘快照，而 TSF 的按键回调跑在 TSF 自己的键盘管理器线程上，
/// 那个线程从不取应用消息 —— 结果 Shift/Ctrl/Caps 永远读不到（大写字母
/// 直通失效就是栽在这里）。GetAsyncKeyState 读全局输入状态表，任何线程、
/// 任何来源（真实键盘 / SendInput 注入）都即时正确。
fn key_down(vk: i32) -> bool {
    unsafe { (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
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

/// Caps Lock 灯是不是亮着（亮着时字母应直通英文大写，和微软一样）。
/// 注意：GetAsyncKeyState 的低 1 位是「上次查询后按过没有」，不是开关状态
/// （实测 Caps 按下瞬间为 true、下一键查询就变 false），不能用；
/// GetKeyState 的低 1 位才是真正的开关状态（队列快照，TSF 回调线程上
/// 对 Shift 这类瞬时键是过期的，但 Caps 开关位试试是不是也过期）。
fn caps_on() -> bool {
    unsafe {
        (windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(
            windows::Win32::UI::Input::KeyboardAndMouse::VK_CAPITAL.0 as i32,
        ) as u16
            & 0x0001)
            != 0
    }
}

/// 整句模式：把刚上屏的词记进句子缓冲（其他模式空操作）
fn sent_push(word: &str) {
    if crate::settings::trmode() == 2 {
        SENT.lock().unwrap().push(word.to_string());
    }
}

fn is_sent_end(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '…' | '.' | '!' | '?')
}

/// 俄语句尾标点：中文全角标点映射成俄语半角（。-> . ？-> ?）
fn rus_end(c: char) -> char {
    match c {
        '。' => '.',
        '！' => '!',
        '？' => '?',
        _ => c,
    }
}

/// 整句收尾：缓冲里有没收尾的中文句子时，把它整个译成俄语接在上屏文本后面。
/// c = 触发收尾的句末标点（回车收尾传 None，俄语不加标点）。
/// 返回拼接好的完整上屏文本；没开整句模式 / 缓冲空 / 译不出时返回 None。
fn sent_close(text: &str, c: Option<char>) -> Option<String> {
    if crate::settings::trmode() != 2 {
        return None;
    }
    if SENT.lock().unwrap().is_empty() {
        return None;
    }
    if let Some(c) = c {
        if !is_sent_end(c) {
            return None;
        }
    }
    let ru = dict::translate_words(SENT.lock().unwrap().iter().map(|s| s.as_str()))?;
    SENT.lock().unwrap().clear();
    match c {
        Some(c) => Some(format!("{text}{c} {ru}{}", rus_end(c))),
        None => Some(format!("{text} {ru}")),
    }
}

/// 候选窗顶行 ☁ 提示：默认是拼音补全；整句翻译模式显示「已上屏句子 +
/// 当前首选候选」的实时整句翻译（谷歌翻译式：每打一个字更新一次）。
/// 要放在 set_all_cands 之后调 —— 首选候选得先刷新。
fn sentence_hint(key: &str) {
    let hint = if crate::settings::trmode() == 2 {
        let cur = unsafe { cand::current() }
            .or_else(|| UI_CANDS.lock().unwrap().first().map(|(w, _)| w.clone()))
            .unwrap_or_default();
        let mut words = SENT.lock().unwrap().clone();
        if !cur.is_empty() {
            words.push(cur);
        }
        dict::translate_words(words.iter().map(|s| s.as_str())).map(|ru| format!("译 {ru}"))
    } else {
        dict::complete(key).map(|(w, _)| w)
    };
    unsafe { cand::set_sentence(hint); }
}

/// Windows 字母键只在 VK_A..VK_Z（0x41..0x5A）。不能把虚拟键码当 ASCII：
/// 小键盘 1（0x61）和 F5（0x74）碰巧对应小写 a/t，先截成 u8 还会误认扩展键码。
fn letter_key(wparam: WPARAM) -> Option<char> {
    if (0x41..=0x5A).contains(&wparam.0) {
        Some(wparam.0 as u8 as char)
    } else {
        None
    }
}

fn claim(wparam: WPARAM, has_buf: bool, english: bool) -> bool {
    if letter_key(wparam).is_some() {
        return !english;
    }
    if !has_buf {
        // 整句翻译：缓冲里有没收尾的句子时，句末标点（. / 及 Shift 变体）和
        // 回车拦下来收尾 —— 这样聊天软件里不敲标点直接按回车也能带上整句俄语
        if crate::settings::trmode() == 2
            && !SENT.lock().unwrap().is_empty()
            && matches!(wparam.0 as u32, 0x0D | 0xBE | 0xBF)
        {
            return true;
        }
        // 中文标点：无拼音时标点键也拦下来转全角（不进组合）。
        // Shift+标点 保持半角放行（方便打代码）；- = ` 没有全角对应，不拦。
        return !english
            && crate::settings::cnpunct()
            && !shift_down()
            && matches!(wparam.0 as u32, 0xBA | 0xBC | 0xBE..=0xBF | 0xDB..=0xDE);
    }
    match wparam.0 as u32 {
        0x20 | 0x1B | 0x08 | 0x0D | 0x09 => true, // 空格 / ESC / 退格 / 回车 / Tab
        // 数字选词：只认当前页里真有的候选，没有就放行给应用（可能只是打字）。
        // Shift+数字 是打 ! @ # $ % 这类符号，不能抢
        0x31..=0x39 => {
            let idx = (wparam.0 as usize) - 0x31;
            !shift_down() && idx < UI_CANDS.lock().unwrap().len()
        }
        // 方向键选词：当前页真有候选才拦，否则放行给应用移动光标。
        // （v4.5.0 前这里无条件 true，空候选时 ←→ 会被 TSF 吞掉）
        0x25..=0x28 => !UI_CANDS.lock().unwrap().is_empty(),
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
    fn LockServer(&self, f: BOOL) -> Result<()> {
        if f.as_bool() {
            LIVE.fetch_add(1, Ordering::SeqCst);
        } else {
            LIVE.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
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

            // COM 调用可能同步回到 OnCompositionTerminated。只在锁内拷贝引用，
            // GetRange/SetText/EndComposition 都在锁外执行，避免输入线程自锁。
            let current = COMPOSITION.lock().unwrap().clone();
            let range = if let Some(comp) = &current {
                match comp.GetRange() {
                    Ok(r) => r,
                    Err(_) => return Ok(()),
                }
            } else {
                match (&*sel[0].range).as_ref() { Some(r) => r.clone(), None => return Ok(()) }
            };

            if current.is_none() {
                if let Ok(cc) = self.ctx.cast::<ITfContextComposition>() {
                    if let Ok(comp) = cc.StartComposition(ec, &range, &self.sink) {
                        *COMPOSITION.lock().unwrap() = Some(comp);
                    }
                }
            }
            let current = COMPOSITION.lock().unwrap().clone();
            if let Some(comp) = current {
                if let Ok(r) = comp.GetRange() {
                    let _ = r.SetText(ec, 0, &self.text);
                }
            }

            // 把候选窗放到光标下方；取不到文本范围就退回系统光标位置（否则候选窗根本不出现）
            let mut placed = false;
            if let Ok(view) = self.ctx.GetActiveView() {
                let mut rc = RECT::default();
                let mut clipped = BOOL(0);
                let current = COMPOSITION.lock().unwrap().clone();
                let r2 = current.as_ref().and_then(|c| c.GetRange().ok()).unwrap_or(range);
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

/// 上屏：把 composition 的文字换成最终的中文，然后结束组合。
/// 没有活动组合时（无拼音直接上屏，比如中文标点）写到当前光标处。
#[implement(ITfEditSession)]
struct DoneSession {
    ctx: ITfContext,
    text: Vec<u16>,
}

impl ITfEditSession_Impl for DoneSession {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        unsafe {
            // 换字 + 结束组合放在同一次会话里（拆成两次会让某些宿主把文字丢掉）
            let current = COMPOSITION.lock().unwrap().take();
            if let Some(comp) = current {
                let mut wrote = false;
                if let Ok(r) = comp.GetRange() {
                    wrote = r.SetText(ec, 0, &self.text).is_ok();
                }
                let hr = comp.EndComposition(ec);
                dlog!("上屏: SetText={wrote} EndComposition -> {hr:?}");
            } else {
                // 无组合：插到当前 selection（CommitSession 同款）
                let mut sel = [TF_SELECTION::default()];
                let mut n = 0u32;
                self.ctx.GetSelection(ec, 0, &mut sel, &mut n)?;
                if n == 0 {
                    return Ok(());
                }
                if let Some(range) = (&*sel[0].range).as_ref() {
                    let ok = range.SetText(ec, 0, &self.text).is_ok();
                    dlog!("上屏: 无组合 SetText={ok}");
                }
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
            let current = COMPOSITION.lock().unwrap().clone();
            if let Some(comp) = current {
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
struct EndSession;

impl ITfEditSession_Impl for EndSession {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        unsafe {
            let current = COMPOSITION.lock().unwrap().take();
            dlog!("EndSession: 有组合={}", current.is_some());
            if let Some(comp) = current {
                // 先清空组合文字再结束：不结束的话文字会留在文档里被宿主当正文提交
                if let Ok(r) = comp.GetRange() {
                    let empty: Vec<u16> = Vec::new();
                    let _ = r.SetText(ec, 0, &empty);
                }
                let hr = comp.EndComposition(ec);
                dlog!("EndSession: EndComposition -> {hr:?}");
            }
        }
        Ok(())
    }
}
