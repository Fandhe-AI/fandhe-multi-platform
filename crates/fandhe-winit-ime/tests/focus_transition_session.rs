//! `ImeSessionTracker` + `ImeFocusCoordinator` の結合テスト（Issue #587 受入基準 2）。
//!
//! `ime_focus.rs` のユニットテストは `ImeFocusCoordinator` 単体を `ImeSessionId` を
//! 直接組み立てて駆動するが、本ファイルは実配線（`ime_event_from_winit` →
//! `ImeSessionTracker::stamp` → `ImeFocusCoordinator::dispatch_ime`、
//! `ImeSessionTracker::observe_request` ← `ImeFocusCoordinator::focus` の `requests`）を
//! 通して、Windows 形の到着列（`SetEnabled(false)` 後に `Preedit("")` → `Commit` →
//! `Disabled` → `Enabled` が旧フィールドのセッション由来で遅延到着する形、
//! `crates/fandhe-winit-ime/src/lib.rs` `ime_event_from_winit` doc の Windows 固有挙動・
//! `ime-architecture.md` §4.8 参照）を実配線で再現し、旧フィールドの文字列が新フィールドへ
//! 入らず、新フィールドの正当な入力は届くことを確認する。

use fandhe_core::ui::{ImeFocusCoordinator, ImeRequest, TextFieldWidget, WidgetId};
use fandhe_winit_ime::{ime_event_from_winit, ImeSessionTracker};
use winit::event::Ime;

/// coordinator の `focus()` が返した `requests` を tracker へ配線する（実アプリの
/// `ImeTextFieldSample::apply_requests` と同じ配線、`examples/ime_text_field_sample.rs` 参照）。
fn observe_all(tracker: &mut ImeSessionTracker, requests: &[ImeRequest]) {
    for request in requests {
        tracker.observe_request(request);
    }
}

#[test]
fn windows_style_batched_stale_events_after_focus_transition_do_not_leak_into_new_field() {
    let mut a = TextFieldWidget::<()>::new_static(WidgetId::from_raw(1), "field_a", "");
    let mut b = TextFieldWidget::<()>::new_static(WidgetId::from_raw(2), "field_b", "");
    let mut coordinator = ImeFocusCoordinator::new();
    let mut tracker = ImeSessionTracker::new();

    // field_a へ初回フォーカス（BeginSession(session_a) を含む requests を tracker へ配線）。
    let mut fields: Vec<&mut TextFieldWidget<()>> = vec![&mut a, &mut b];
    let t1 = coordinator.focus(&mut fields, WidgetId::from_raw(1));
    observe_all(&mut tracker, &t1.requests);

    // field_a が変換中に Tab で field_b へフォーカス移動（BeginSession(session_b) を含む
    // requests を tracker へ配線。tracker はまだ session_b への昇格信号を観測していないため
    // pending に留め、旧セッション（session_a）を current のまま保持する）。
    let t2 = coordinator.focus(&mut fields, WidgetId::from_raw(2));
    observe_all(&mut tracker, &t2.requests);

    // field_a の IME セッション由来の遅延イベントがバッチで到着する想定
    // （Windows: WM_IME_ENDCOMPOSITION が「空 Preedit → Commit」を送出してから Disabled、
    // `ime_event_from_winit` doc・`windows_japanese_input.rs` 参照）。
    let stale_preedit = ime_event_from_winit(Ime::Preedit(String::new(), None));
    let session = tracker.stamp(&stale_preedit);
    coordinator.dispatch_ime(&mut fields, session, stale_preedit);

    let stale_commit = ime_event_from_winit(Ime::Commit("stale-from-field-a".to_string()));
    let session = tracker.stamp(&stale_commit);
    let events = coordinator.dispatch_ime(&mut fields, session, stale_commit);
    assert!(events.is_empty());
    // field_b（新フィールド）は旧セッションの確定文字列を受け取っていない。
    assert_eq!(fields[1].buffer(), "");

    let stale_disabled = ime_event_from_winit(Ime::Disabled);
    let session = tracker.stamp(&stale_disabled);
    coordinator.dispatch_ime(&mut fields, session, stale_disabled);
    // Disabled で tracker が pending（session_b）へ昇格する。
    assert_eq!(tracker.current(), coordinator.session());

    // field_b 自身の正当な入力シーケンス（Enabled → 空 Preedit → Commit）は届く。
    let enabled = ime_event_from_winit(Ime::Enabled);
    let session = tracker.stamp(&enabled);
    coordinator.dispatch_ime(&mut fields, session, enabled);

    let preedit = ime_event_from_winit(Ime::Preedit(String::new(), None));
    let session = tracker.stamp(&preedit);
    coordinator.dispatch_ime(&mut fields, session, preedit);

    let commit = ime_event_from_winit(Ime::Commit("hi".to_string()));
    let session = tracker.stamp(&commit);
    coordinator.dispatch_ime(&mut fields, session, commit);

    assert_eq!(fields[1].buffer(), "hi");
    // field_a（旧フィールド）は blur 済みで、遅延イベントの影響を一切受けていない。
    assert_eq!(fields[0].buffer(), "");
}

#[test]
fn macos_style_non_empty_preedit_promotes_session_without_disabled() {
    // field へのフォーカス遷移は `ImeFocusCoordinator::focus` が必ず旧フィールドの
    // `SetEnabled(false)` を発行する（§4.7 順序契約）ため、フィールド間の遷移では
    // `current_blur_requested` が立ち、「Enabled ゲート」（`session` モジュール doc、
    // Issue #680 レビュー指摘 P0: PRRT_kwDOTl73E86degYY）により非空 Preedit だけでは
    // 昇格しない。field_b 自身の Enabled 到着で昇格することを確認する。
    let mut a = TextFieldWidget::<()>::new_static(WidgetId::from_raw(3), "field_a", "");
    let mut b = TextFieldWidget::<()>::new_static(WidgetId::from_raw(4), "field_b", "");
    let mut coordinator = ImeFocusCoordinator::new();
    let mut tracker = ImeSessionTracker::new();

    let mut fields: Vec<&mut TextFieldWidget<()>> = vec![&mut a, &mut b];
    let t1 = coordinator.focus(&mut fields, WidgetId::from_raw(3));
    observe_all(&mut tracker, &t1.requests);
    let t2 = coordinator.focus(&mut fields, WidgetId::from_raw(4));
    observe_all(&mut tracker, &t2.requests);

    // Enabled ゲートが立っている間の非空 Preedit は field_a へ帰属し、昇格しない。
    let preedit_without_enabled = ime_event_from_winit(Ime::Preedit("あ".to_string(), None));
    let session = tracker.stamp(&preedit_without_enabled);
    assert_ne!(session, coordinator.session());
    coordinator.dispatch_ime(&mut fields, session, preedit_without_enabled);
    assert!(!fields[1].is_composing());

    // field_b 自身の開始信号（Enabled）が届いてようやく昇格する。
    let enabled = ime_event_from_winit(Ime::Enabled);
    let session = tracker.stamp(&enabled);
    assert_eq!(session, coordinator.session());
    coordinator.dispatch_ime(&mut fields, session, enabled);

    let preedit = ime_event_from_winit(Ime::Preedit("あ".to_string(), None));
    let session = tracker.stamp(&preedit);
    assert_eq!(session, coordinator.session());
    coordinator.dispatch_ime(&mut fields, session, preedit);
    assert!(fields[1].is_composing());
}

#[test]
fn triple_focus_transition_does_not_leak_intermediate_field_commit_into_latest_field() {
    // A→B→C とイベント到着前に連続でフォーカス遷移した後、A の Disabled → 中間
    // フィールド B 由来の遅延「空 Preedit → Commit」が届くケース（PR #680 レビュー
    // 指摘、Issue #587 の主対象を 3 フィールドへ拡張）。B の確定文字列が最新の
    // フィールド C へ誤って混入しないことを確認する。
    let mut a = TextFieldWidget::<()>::new_static(WidgetId::from_raw(5), "field_a", "");
    let mut b = TextFieldWidget::<()>::new_static(WidgetId::from_raw(6), "field_b", "");
    let mut c = TextFieldWidget::<()>::new_static(WidgetId::from_raw(7), "field_c", "");
    let mut coordinator = ImeFocusCoordinator::new();
    let mut tracker = ImeSessionTracker::new();

    let mut fields: Vec<&mut TextFieldWidget<()>> = vec![&mut a, &mut b, &mut c];
    let t1 = coordinator.focus(&mut fields, WidgetId::from_raw(5));
    observe_all(&mut tracker, &t1.requests);
    // イベント到着前に B → C へ連続遷移する（B の開始/終了信号は届かないまま）。
    let t2 = coordinator.focus(&mut fields, WidgetId::from_raw(6));
    observe_all(&mut tracker, &t2.requests);
    let t3 = coordinator.focus(&mut fields, WidgetId::from_raw(7));
    observe_all(&mut tracker, &t3.requests);

    // field_a の終了信号。tracker は中間の field_b へ昇格し、field_c はまだ pending。
    let disabled_a = ime_event_from_winit(Ime::Disabled);
    let session = tracker.stamp(&disabled_a);
    coordinator.dispatch_ime(&mut fields, session, disabled_a);

    // field_b 由来の遅延「空 Preedit → Commit」は field_b（機微情報を含み得る）へ帰属し、
    // 最新の field_c へ漏れない。
    let stale_preedit_b = ime_event_from_winit(Ime::Preedit(String::new(), None));
    let session = tracker.stamp(&stale_preedit_b);
    coordinator.dispatch_ime(&mut fields, session, stale_preedit_b);

    let stale_commit_b = ime_event_from_winit(Ime::Commit("stale-from-field-b".to_string()));
    let session = tracker.stamp(&stale_commit_b);
    coordinator.dispatch_ime(&mut fields, session, stale_commit_b);

    assert_eq!(fields[2].buffer(), "");

    // field_b の終了信号でようやく field_c へ昇格し、field_c 自身の入力が届く。
    let disabled_b = ime_event_from_winit(Ime::Disabled);
    let session = tracker.stamp(&disabled_b);
    coordinator.dispatch_ime(&mut fields, session, disabled_b);
    assert_eq!(tracker.current(), coordinator.session());

    let enabled = ime_event_from_winit(Ime::Enabled);
    let session = tracker.stamp(&enabled);
    coordinator.dispatch_ime(&mut fields, session, enabled);
    // Commit ゲート（`ImeFocusCoordinator` doc「フォーカス遷移直後の遅延 Commit に
    // 対するガード」参照）を開くため、実配線どおり Preedit を先に転送する。
    let preedit_c = ime_event_from_winit(Ime::Preedit(String::new(), None));
    let session = tracker.stamp(&preedit_c);
    coordinator.dispatch_ime(&mut fields, session, preedit_c);
    let commit = ime_event_from_winit(Ime::Commit("hi".to_string()));
    let session = tracker.stamp(&commit);
    coordinator.dispatch_ime(&mut fields, session, commit);

    assert_eq!(fields[2].buffer(), "hi");
    assert_eq!(fields[0].buffer(), "");
    assert_eq!(fields[1].buffer(), "");
}

#[test]
fn blurred_field_delayed_non_empty_preedit_does_not_leak_commit_into_next_field() {
    // Issue #680 P0（codex 指摘、PRRT_kwDOTl73E86degYY）の再現列を実配線で確認する:
    // field_a: 非空 Preedit → focus(field_b)/SetEnabled(false) → 遅延した field_a
    // 由来の非空 Preedit → 空 Preedit → Commit、という順序で届いても、field_a の
    // 確定文字列が field_b の buffer へ混入しないことを検証する（「Enabled ゲート」
    // 設計、`session` モジュール doc 参照）。
    let mut a = TextFieldWidget::<()>::new_static(WidgetId::from_raw(8), "field_a", "");
    let mut b = TextFieldWidget::<()>::new_static(WidgetId::from_raw(9), "field_b", "");
    let mut coordinator = ImeFocusCoordinator::new();
    let mut tracker = ImeSessionTracker::new();

    let mut fields: Vec<&mut TextFieldWidget<()>> = vec![&mut a, &mut b];
    let t1 = coordinator.focus(&mut fields, WidgetId::from_raw(8));
    observe_all(&mut tracker, &t1.requests);

    // field_a がコンポジションを開始する。
    let preedit_a = ime_event_from_winit(Ime::Preedit("あ".to_string(), None));
    let session = tracker.stamp(&preedit_a);
    coordinator.dispatch_ime(&mut fields, session, preedit_a);

    // Tab で field_b へフォーカス移動（SetEnabled(false) → BeginSession の順序契約）。
    let t2 = coordinator.focus(&mut fields, WidgetId::from_raw(9));
    observe_all(&mut tracker, &t2.requests);

    // field_a に対して native 側にキューイング済みだった遅延イベント列。
    // 非空 Preedit も B の開始信号として昇格させてはならない（Enabled ゲート）。
    let stale_preedit = ime_event_from_winit(Ime::Preedit("あい".to_string(), None));
    let session = tracker.stamp(&stale_preedit);
    coordinator.dispatch_ime(&mut fields, session, stale_preedit);

    let stale_empty_preedit = ime_event_from_winit(Ime::Preedit(String::new(), None));
    let session = tracker.stamp(&stale_empty_preedit);
    coordinator.dispatch_ime(&mut fields, session, stale_empty_preedit);

    let stale_commit = ime_event_from_winit(Ime::Commit("stale-from-field-a".to_string()));
    let session = tracker.stamp(&stale_commit);
    coordinator.dispatch_ime(&mut fields, session, stale_commit);

    // field_b（新フィールド）は旧セッションの確定文字列を受け取っていない。
    assert_eq!(fields[1].buffer(), "");

    // field_b 自身の開始信号（Enabled）が届いてようやく昇格し、以降の入力が届く。
    let enabled = ime_event_from_winit(Ime::Enabled);
    let session = tracker.stamp(&enabled);
    coordinator.dispatch_ime(&mut fields, session, enabled);
    assert_eq!(tracker.current(), coordinator.session());

    let preedit_b = ime_event_from_winit(Ime::Preedit(String::new(), None));
    let session = tracker.stamp(&preedit_b);
    coordinator.dispatch_ime(&mut fields, session, preedit_b);

    let commit_b = ime_event_from_winit(Ime::Commit("hi".to_string()));
    let session = tracker.stamp(&commit_b);
    coordinator.dispatch_ime(&mut fields, session, commit_b);

    assert_eq!(fields[1].buffer(), "hi");
    assert_eq!(fields[0].buffer(), "");
}
