use super::*;
use std::sync::Arc;
use windows::core::{IUnknown, PCWSTR, PWSTR};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::UI::TextServices::{
    IEnumTfContextViews, IEnumTfProperties, ITfComposition_Impl, ITfContext_Impl,
    ITfContextView, ITfProperty, ITfRange, ITfRangeBackup, ITfRange_Impl,
    ITfReadOnlyProperty, TfAnchor, TfGravity, TfShiftDir, TF_HALTCOND, TS_STATUS,
};

// Production state is shared, so tests touching it must run one at a time.
static TEST_LOCK: Mutex<()> = Mutex::new(());

#[implement(ITfComposition)]
struct TestComposition {
    unlocked: Arc<AtomicBool>,
    sink: Option<ITfCompositionSink>,
    range: Option<ITfRange>,
    ended: Arc<AtomicU32>,
}

impl ITfComposition_Impl for TestComposition {
    fn GetRange(&self) -> Result<ITfRange> {
        self.unlocked.fetch_and(COMPOSITION.0.try_lock().is_ok(), Ordering::SeqCst);
        self.range.clone().ok_or_else(|| E_FAIL.into())
    }

    fn ShiftStart(&self, _ec: u32, _range: Option<&ITfRange>) -> Result<()> { Ok(()) }
    fn ShiftEnd(&self, _ec: u32, _range: Option<&ITfRange>) -> Result<()> { Ok(()) }

    fn EndComposition(&self, ec: u32) -> Result<()> {
        self.ended.fetch_add(1, Ordering::SeqCst);
        let unlocked = COMPOSITION.0.try_lock().is_ok();
        self.unlocked.fetch_and(unlocked, Ordering::SeqCst);
        // Avoid hanging the regression runner on the old implementation.
        if unlocked {
            if let Some(sink) = &self.sink {
                let this: ITfComposition = unsafe { self.cast()? };
                unsafe { sink.OnCompositionTerminated(ec, Some(&this))?; }
            }
        }
        Ok(())
    }
}

fn composition(sink: Option<ITfCompositionSink>) -> (ITfComposition, Arc<AtomicBool>) {
    let unlocked = Arc::new(AtomicBool::new(true));
    (TestComposition {
        unlocked: unlocked.clone(), sink, range: None, ended: Arc::new(AtomicU32::new(0)),
    }.into(), unlocked)
}

/// Runs real edit sessions against an in-memory document, without activating TSF or a window.
#[implement(ITfContext)]
struct TestContext;

impl ITfContext_Impl for TestContext {
    fn RequestEditSession(&self, _tid: u32, session: Option<&ITfEditSession>, flags: TF_CONTEXT_EDIT_CONTEXT_FLAGS) -> Result<HRESULT> {
        assert_eq!(flags.0, TF_ES_SYNC.0 | TF_ES_READWRITE.0);
        unsafe { session.ok_or_else(|| windows::core::Error::from(E_POINTER))?.DoEditSession(7)?; }
        Ok(S_OK)
    }
    fn InWriteSession(&self, _tid: u32) -> Result<BOOL> { Err(E_FAIL.into()) }
    fn GetSelection(&self, _ec: u32, _index: u32, _count: u32, _selection: *mut TF_SELECTION, _fetched: *mut u32) -> Result<()> { Err(E_FAIL.into()) }
    fn SetSelection(&self, _ec: u32, _count: u32, _selection: *const TF_SELECTION) -> Result<()> { Err(E_FAIL.into()) }
    fn GetStart(&self, _ec: u32) -> Result<ITfRange> { Err(E_FAIL.into()) }
    fn GetEnd(&self, _ec: u32) -> Result<ITfRange> { Err(E_FAIL.into()) }
    fn GetActiveView(&self) -> Result<ITfContextView> { Err(E_FAIL.into()) }
    fn EnumViews(&self) -> Result<IEnumTfContextViews> { Err(E_FAIL.into()) }
    fn GetStatus(&self) -> Result<TS_STATUS> { Err(E_FAIL.into()) }
    fn GetProperty(&self, _guid: *const GUID) -> Result<ITfProperty> { Err(E_FAIL.into()) }
    fn GetAppProperty(&self, _guid: *const GUID) -> Result<ITfReadOnlyProperty> { Err(E_FAIL.into()) }
    fn TrackProperties(&self, _props: *const *const GUID, _count: u32, _app_props: *const *const GUID, _app_count: u32) -> Result<ITfReadOnlyProperty> { Err(E_FAIL.into()) }
    fn EnumProperties(&self) -> Result<IEnumTfProperties> { Err(E_FAIL.into()) }
    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> { Err(E_FAIL.into()) }
    fn CreateRangeBackup(&self, _ec: u32, _range: Option<&ITfRange>) -> Result<ITfRangeBackup> { Err(E_FAIL.into()) }
}

#[implement(ITfRange)]
struct TestRange {
    text: Arc<Mutex<String>>,
}

impl ITfRange_Impl for TestRange {
    fn SetText(&self, _ec: u32, _flags: u32, text: &PCWSTR, len: i32) -> Result<()> {
        assert!(COMPOSITION.0.try_lock().is_ok(), "SetText called while composition mutex was locked");
        assert!(len >= 0);
        let utf16 = if len == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(text.0, len as usize) } };
        *self.text.lock().unwrap() = String::from_utf16(utf16).unwrap();
        Ok(())
    }
    fn GetText(&self, _ec: u32, _flags: u32, _text: PWSTR, _max: u32, _len: *mut u32) -> Result<()> { Err(E_FAIL.into()) }
    fn GetFormattedText(&self, _ec: u32) -> Result<IDataObject> { Err(E_FAIL.into()) }
    fn GetEmbedded(&self, _ec: u32, _service: *const GUID, _iid: *const GUID) -> Result<IUnknown> { Err(E_FAIL.into()) }
    fn InsertEmbedded(&self, _ec: u32, _flags: u32, _object: Option<&IDataObject>) -> Result<()> { Err(E_FAIL.into()) }
    fn ShiftStart(&self, _ec: u32, _count: i32, _shifted: *mut i32, _halt: *const TF_HALTCOND) -> Result<()> { Err(E_FAIL.into()) }
    fn ShiftEnd(&self, _ec: u32, _count: i32, _shifted: *mut i32, _halt: *const TF_HALTCOND) -> Result<()> { Err(E_FAIL.into()) }
    fn ShiftStartToRange(&self, _ec: u32, _range: Option<&ITfRange>, _anchor: TfAnchor) -> Result<()> { Err(E_FAIL.into()) }
    fn ShiftEndToRange(&self, _ec: u32, _range: Option<&ITfRange>, _anchor: TfAnchor) -> Result<()> { Err(E_FAIL.into()) }
    fn ShiftStartRegion(&self, _ec: u32, _dir: TfShiftDir) -> Result<BOOL> { Err(E_FAIL.into()) }
    fn ShiftEndRegion(&self, _ec: u32, _dir: TfShiftDir) -> Result<BOOL> { Err(E_FAIL.into()) }
    fn IsEmpty(&self, _ec: u32) -> Result<BOOL> { Err(E_FAIL.into()) }
    fn Collapse(&self, _ec: u32, _anchor: TfAnchor) -> Result<()> { Err(E_FAIL.into()) }
    fn IsEqualStart(&self, _ec: u32, _range: Option<&ITfRange>, _anchor: TfAnchor) -> Result<BOOL> { Err(E_FAIL.into()) }
    fn IsEqualEnd(&self, _ec: u32, _range: Option<&ITfRange>, _anchor: TfAnchor) -> Result<BOOL> { Err(E_FAIL.into()) }
    fn CompareStart(&self, _ec: u32, _range: Option<&ITfRange>, _anchor: TfAnchor) -> Result<i32> { Err(E_FAIL.into()) }
    fn CompareEnd(&self, _ec: u32, _range: Option<&ITfRange>, _anchor: TfAnchor) -> Result<i32> { Err(E_FAIL.into()) }
    fn AdjustForInsert(&self, _ec: u32, _count: u32) -> Result<BOOL> { Err(E_FAIL.into()) }
    fn GetGravity(&self, _start: *mut TfGravity, _end: *mut TfGravity) -> Result<()> { Err(E_FAIL.into()) }
    fn SetGravity(&self, _ec: u32, _start: TfGravity, _end: TfGravity) -> Result<()> { Err(E_FAIL.into()) }
    fn Clone(&self) -> Result<ITfRange> { Err(E_FAIL.into()) }
    fn GetContext(&self) -> Result<ITfContext> { Err(E_FAIL.into()) }
}

fn reset() {
    let previous = COMPOSITION.lock().unwrap().take();
    drop(previous);
    UI_CANDS.lock().unwrap().clear();
    ALL_CANDS.lock().unwrap().clear();
    SENT.lock().unwrap().clear();
    PAGE.store(0, Ordering::Relaxed);
    *PENDING_CAND.lock().unwrap() = None;
    unsafe { cand::set_items(Vec::new()); }
}

#[test]
fn function_and_numpad_keys_pass_through() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    for has_buf in [false, true] {
        for vk in (0x60..=0x6F).chain(0x70..=0x87).chain([0x141]) {
            assert!(!claim(WPARAM(vk), has_buf, false), "VK {vk:#x}, buffer={has_buf}");
        }
    }
}

#[test]
fn letter_keys_still_enter_pinyin() {
    let _serial = TEST_LOCK.lock().unwrap();
    for vk in 0x41..=0x5A {
        assert!(claim(WPARAM(vk), false, false));
        assert!(claim(WPARAM(vk), true, false));
        assert!(!claim(WPARAM(vk), false, true));
    }
}

#[test]
fn non_letter_keydown_does_not_change_pinyin() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    let service = TextService::new();
    *service.buf.borrow_mut() = "ni".to_string();
    for vk in [0x61, 0x69, 0x70, 0x74, 0x7A, 0x141] {
        let eaten = service.OnKeyDown(None, WPARAM(vk), LPARAM(0)).unwrap();
        assert!(!eaten.as_bool(), "VK {vk:#x}");
        assert_eq!(&*service.buf.borrow(), "ni");
    }
    reset();
}

#[test]
fn host_termination_clears_preedit_and_candidates() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    let service = TextService::new();
    *service.buf.borrow_mut() = "ni".to_string();
    let (comp, _) = composition(None);
    *COMPOSITION.lock().unwrap() = Some(comp.clone());
    UI_CANDS.lock().unwrap().push(("你".into(), "ты".into()));
    ALL_CANDS.lock().unwrap().push(("你".into(), "nǐ".into(), "ты".into()));
    SENT.lock().unwrap().push("你好".into());
    PAGE.store(3, Ordering::Relaxed);
    *PENDING_CAND.lock().unwrap() = Some((42, 100));
    unsafe {
        cand::set_items(vec![cand::Item {
            word: "你".into(), ru: "ты".into(), fresh: false,
        }]);
    }
    service.OnCompositionTerminated(0, Some(&comp)).unwrap();
    assert!(service.buf.borrow().is_empty());
    assert!(COMPOSITION.lock().unwrap().is_none());
    assert!(UI_CANDS.lock().unwrap().is_empty());
    assert!(ALL_CANDS.lock().unwrap().is_empty());
    assert!(SENT.lock().unwrap().is_empty());
    assert_eq!(PAGE.load(Ordering::Relaxed), 0);
    assert_eq!(*PENDING_CAND.lock().unwrap(), None);
    assert_eq!(unsafe { cand::count() }, 0);
    reset();
}

#[test]
fn old_termination_does_not_cancel_new_composition() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    let service = TextService::new();
    *service.buf.borrow_mut() = "hao".to_string();
    let (current, _) = composition(None);
    let (old, _) = composition(None);
    *COMPOSITION.lock().unwrap() = Some(current.clone());
    service.OnCompositionTerminated(0, Some(&old)).unwrap();
    let remaining = COMPOSITION.lock().unwrap().clone();
    assert_eq!(remaining.as_ref(), Some(&current));
    assert_eq!(&*service.buf.borrow(), "hao");
    reset();
}

#[test]
fn cancel_composition_allows_synchronous_callback() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    let sink: ITfCompositionSink = TextService::new().into();
    let (comp, unlocked) = composition(Some(sink));
    *COMPOSITION.lock().unwrap() = Some(comp);
    SENT.lock().unwrap().push("你好".into());
    EndSession.DoEditSession(0).unwrap();
    assert!(unlocked.load(Ordering::SeqCst), "COM was called while composition mutex was locked");
    assert!(COMPOSITION.lock().unwrap().is_none());
    assert_eq!(&*SENT.lock().unwrap(), &["你好"]);
    reset();
}

#[test]
fn updating_composition_calls_com_without_lock() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    let (comp, unlocked) = composition(None);
    *COMPOSITION.lock().unwrap() = Some(comp);
    UpdateSession { text: vec![b'n' as u16] }.DoEditSession(0).unwrap();
    assert!(unlocked.load(Ordering::SeqCst));
    reset();
}

#[test]
fn russian_output_keeps_the_selected_gloss() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    UI_CANDS.lock().unwrap().push(("重".into(), "тяжёлый | тя́жкий".into()));
    unsafe { cand::set_ru_sel(true); }
    let (word, ru) = selected_choice().unwrap();
    assert_eq!(TextService::pick_text(&word, &ru), "тяжёлый");
    reset();
}

#[test]
fn blank_candidate_gloss_does_not_borrow_another_reading() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    UI_CANDS.lock().unwrap().push(("重".into(), String::new()));
    unsafe { cand::set_ru_sel(true); }
    let (word, ru) = selected_choice().unwrap();
    assert_eq!(TextService::pick_text(&word, &ru), "重");
    reset();
}

#[test]
fn candidate_text_preserves_first_sense_and_translation_mode() {
    assert_eq!(candidate_text("重", " повторя́ть | повтори́ть ", true, true), "повторя́ть");
    assert_eq!(candidate_text("重", " повторя́ть | повтори́ть ", false, true), "重 повторя́ть");
    assert_eq!(candidate_text("重", "тяжёлый", false, false), "重");
    assert_eq!(candidate_text("重", "  ", true, true), "重");
}

#[test]
fn selected_gloss_survives_candidate_pool_cleanup() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    UI_CANDS.lock().unwrap().extend([
        ("重".into(), "тяжёлый".into()), ("重".into(), "повторя́ть".into()),
    ]);
    unsafe {
        cand::set_items(vec![
            cand::Item { word: "重".into(), ru: "тяжёлый".into(), fresh: false },
            cand::Item { word: "重".into(), ru: "повторя́ть".into(), fresh: false },
        ]);
        cand::select(1);
    }
    let (word, ru) = selected_choice().unwrap();
    UI_CANDS.lock().unwrap().clear();
    assert_eq!(candidate_text(&word, &ru, true, false), "повторя́ть");
    assert_eq!(candidate_text(&word, &ru, false, true), "重 повторя́ть");
    reset();
}

#[test]
fn tab_only_selects_when_candidates_exist() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    let service = TextService::new();
    *service.buf.borrow_mut() = "zzzzzz".to_string();
    assert!(!claim(WPARAM(0x09), true, false));
    assert!(!service.OnKeyDown(None, WPARAM(0x09), LPARAM(0)).unwrap().as_bool());
    assert_eq!(&*service.buf.borrow(), "zzzzzz");

    UI_CANDS.lock().unwrap().extend([("你".into(), "ты".into()), ("尼".into(), String::new())]);
    unsafe {
        cand::set_items(vec![
            cand::Item { word: "你".into(), ru: "ты".into(), fresh: false },
            cand::Item { word: "尼".into(), ru: String::new(), fresh: false },
        ]);
    }
    assert!(claim(WPARAM(0x09), true, false));
    assert!(service.OnKeyDown(None, WPARAM(0x09), LPARAM(0)).unwrap().as_bool());
    assert_eq!(unsafe { cand::current_index() }, 1);
    assert_eq!(*PENDING_CAND.lock().unwrap(), None);
    assert!(!unsafe { cand::alive() });
    reset();
}

#[test]
fn confirming_without_candidates_preserves_letters_and_ends_composition() {
    let _serial = TEST_LOCK.lock().unwrap();
    let learned_path = std::path::PathBuf::from(settings::cfg_file()).with_file_name("learned.txt");
    let learned_before = std::fs::read(&learned_path).ok();
    for key in [0x0D, 0x20] {
        reset();
        let service = TextService::new();
        let raw = "zzzzzz";
        *service.buf.borrow_mut() = raw.to_string();
        SENT.lock().unwrap().push("已有句子".into());
        let text = Arc::new(Mutex::new(raw.to_string()));
        let range: ITfRange = TestRange { text: text.clone() }.into();
        let ended = Arc::new(AtomicU32::new(0));
        let unlocked = Arc::new(AtomicBool::new(true));
        let comp: ITfComposition = TestComposition {
            unlocked: unlocked.clone(), sink: None, range: Some(range), ended: ended.clone(),
        }.into();
        *COMPOSITION.lock().unwrap() = Some(comp);
        let ctx: ITfContext = TestContext.into();

        assert!(claim(WPARAM(key), true, false));
        assert!(service.OnKeyDown(Some(&ctx), WPARAM(key), LPARAM(0)).unwrap().as_bool());
        assert_eq!(&*text.lock().unwrap(), raw, "VK {key:#x} changed the raw input");
        assert_eq!(ended.load(Ordering::SeqCst), 1, "VK {key:#x} left a composition alive");
        assert!(unlocked.load(Ordering::SeqCst));
        assert!(COMPOSITION.lock().unwrap().is_none());
        assert!(service.buf.borrow().is_empty());
        assert!(UI_CANDS.lock().unwrap().is_empty());
        assert_eq!(&*SENT.lock().unwrap(), &["已有句子"]);
        assert_eq!(std::fs::read(&learned_path).ok(), learned_before);
        assert_eq!(*PENDING_CAND.lock().unwrap(), None);
        assert!(!unsafe { cand::alive() });
    }
    reset();
}

#[test]
fn page_keys_and_arrows_keep_page_selection_in_sync() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    let size = settings::max_cands();
    set_all_cands((0..(size * 2 + 2)).map(|i| dict::Hit {
        word: format!("候选{i}"), pinyin: "ni".into(), ru: String::new(), freq: 1, tier: 0,
    }).collect());
    assert_eq!(unsafe { cand::page_info() }, (0, 3));
    assert_eq!(unsafe { cand::count() }, size);
    assert!(!flip_page(-1));
    assert!(flip_page(1));
    assert_eq!(unsafe { cand::page_info() }, (1, 3));
    assert_eq!(selected_choice().unwrap().0, format!("候选{size}"));

    // Moving up from a page's first item lands on the preceding page's last item.
    move_sel(-1);
    assert_eq!(unsafe { cand::page_info() }, (0, 3));
    assert_eq!(unsafe { cand::current_index() }, size - 1);
    assert_eq!(selected_choice().unwrap().0, format!("候选{}", size - 1));
    move_sel(1);
    assert_eq!(unsafe { cand::page_info() }, (1, 3));
    assert_eq!(unsafe { cand::current_index() }, 0);

    assert!(flip_page(1));
    assert_eq!(unsafe { cand::page_info() }, (2, 3));
    assert_eq!(unsafe { cand::count() }, 2);
    move_sel(1);
    assert_eq!(unsafe { cand::current_index() }, 1);
    assert_eq!(selected_choice().unwrap().0, format!("候选{}", size * 2 + 1));
    move_sel(1);
    assert!(!flip_page(1));
    assert_eq!(unsafe { cand::current_index() }, 1);
    assert_eq!(*PENDING_CAND.lock().unwrap(), None);
    assert!(!unsafe { cand::alive() });
    reset();
}
