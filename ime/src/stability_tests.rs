use super::*;
use std::sync::Arc;
use windows::Win32::UI::TextServices::{ITfComposition_Impl, ITfRange};

// Production state is shared, so tests touching it must run one at a time.
static TEST_LOCK: Mutex<()> = Mutex::new(());

#[implement(ITfComposition)]
struct TestComposition {
    unlocked: Arc<AtomicBool>,
    sink: Option<ITfCompositionSink>,
}

impl ITfComposition_Impl for TestComposition {
    fn GetRange(&self) -> Result<ITfRange> {
        self.unlocked.fetch_and(COMPOSITION.0.try_lock().is_ok(), Ordering::SeqCst);
        Err(E_FAIL.into())
    }

    fn ShiftStart(&self, _ec: u32, _range: Option<&ITfRange>) -> Result<()> { Ok(()) }
    fn ShiftEnd(&self, _ec: u32, _range: Option<&ITfRange>) -> Result<()> { Ok(()) }

    fn EndComposition(&self, ec: u32) -> Result<()> {
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
    (TestComposition { unlocked: unlocked.clone(), sink }.into(), unlocked)
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
        cand::set_preedit("ni");
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
