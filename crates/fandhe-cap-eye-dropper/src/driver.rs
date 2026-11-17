//! Phase 11 の拡充ループ型サンプル「画面のスポイト」（親 #148、Issue #762 macOS）の effect ドライバと、
//! 画面の状態型に依存しない非同期の入口（Issue #1359）。
//! `fandhe_core::screens::eye_dropper_pick`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は
//! `fandhe_device::eye_dropper` のモジュール doc。本 crate が `fandhe-egui-proxy` から移した（#1191）。
//!
//! # macOS・Web: `bridge` の受け口
//!
//! `NSColorSampler` のハンドラ・ブラウザの `EyeDropper.open()` は非同期に届くため、`EyeDropper` トレイトを通さず
//! `fandhe_runtime::request_slot::RequestSlot` で受ける。画面（macOS の [`eye_dropper_pick_effect`]・Web の `web.rs`）と
//! 公開入口（[`begin_eye_dropper`] / [`poll_eye_dropper`] / [`discard_eye_dropper`]）が同じ slot を共有し、
//! 持ち主の印（`SlotOwner`）で互いの要求を置き換えない。片方が進行中・結果未取得のあいだ、後から来た方は
//! `Internal("eye_dropper busy")` で断る（macOS はサンプラーが 2 つ同時に開かないようにするためでもある）。
//!
//! # 他の OS
//!
//! `UnsupportedEyeDropper` 経由で `Unsupported` を画面へ出す（成功にしない）。公開入口も `Unsupported`。

use fandhe_core::screens::eye_dropper_pick::EyeDropperOutcome;
#[cfg(not(target_arch = "wasm32"))]
use fandhe_core::screens::eye_dropper_pick::EyeDropperState;
use fandhe_device::eye_dropper::PickedColor;

#[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
use fandhe_core::screens::eye_dropper_pick::{finish_pick, mark_picking, take_request};
use fandhe_device::eye_dropper::{EyeDropper, UnsupportedEyeDropper};

use crate::error::EyeDropperError;
#[cfg(not(target_arch = "wasm32"))]
use fandhe_runtime::CompletionSender;

/// `Result<PickedColor, EyeDropperError>` を画面用の結果へ畳む。権限拒否などの想定外は `from_device` の段で
/// `Internal` になり、成功にならない。表示文字列は内部識別子のみで色は入らない。
pub(crate) fn to_outcome(result: Result<PickedColor, EyeDropperError>) -> EyeDropperOutcome {
    match result {
        Ok(c) => EyeDropperOutcome::Picked(c.hex()),
        Err(EyeDropperError::Cancelled) => EyeDropperOutcome::Cancelled,
        Err(EyeDropperError::Unsupported) => EyeDropperOutcome::Unsupported,
        Err(e) => EyeDropperOutcome::Failed(e.to_string()),
    }
}

/// macOS・Web: OS コールバックとの橋渡し（pull 型、`RequestSlot` の facade）。
#[cfg(any(target_os = "macos", target_arch = "wasm32"))]
pub(crate) mod bridge {
    use fandhe_device::eye_dropper::PickedColor;
    use fandhe_runtime::request_slot::{RequestSlot, SlotOwner};

    use crate::error::EyeDropperError;

    /// OS からの生の結果。画面へ渡すときは `super::to_outcome` で変換し、公開入口の poll はそのまま返す。
    pub(crate) type Outcome = Result<PickedColor, EyeDropperError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();
    /// 画面 effect が追う自分の分の要求。
    pub(crate) static SCREEN_OWNER: SlotOwner = SlotOwner::new();
    /// 公開入口 [`super::begin_eye_dropper`] が追う自分の分の要求。
    pub(crate) static PUBLIC_OWNER: SlotOwner = SlotOwner::new();
    /// 公開側の poll・discard 本体（持ち主の照合・取得・解放）を直列にする。並行 poll で結果が二重に返らないため。
    pub(crate) static PUBLIC_POLL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// OS からの結果を置く（macOS の main queue から。Web は公開入口の結果だけ `web.rs` が `SLOT` へ直接置き、画面 effect は slot を進行中の印にしか使わない）。`request_id` 不一致・
    /// 未登録・既に結果ありは捨てて `false`（古い世代・二重通知が成功へ化けない）。
    #[cfg(target_os = "macos")]
    pub(crate) fn deliver(request_id: u64, result: Outcome) -> bool {
        SLOT.deliver(request_id, result)
    }

    /// 公開入口の自分の分の要求を捨てる（`super::discard_eye_dropper` の本体）。持ち主が `request_id` と一致するときだけ
    /// 持ち主を外し、「捨てた時点で進行中（結果未着）だったか」を返す。他人の id・取り出し済み・0 は `None`。
    ///
    /// 結果が届いていれば slot も空ける。進行中なら slot は残す: `NSColorSampler` には閉じる API が無く、
    /// slot を消すと次の開始で 2 つ目のサンプラーが開いてしまう。OS 側の確定（結果の到着）までは次の開始を busy で断り、
    /// 届いた持ち主のいない結果は次の開始が置き換える（device-bridge-contracts §9「完了の通知が来るまで未解放の記録を消さない」）。
    pub(crate) fn discard_public(request_id: u64) -> Option<bool> {
        let _guard = PUBLIC_POLL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if request_id == 0 || PUBLIC_OWNER.current() != request_id {
            return None;
        }
        let in_flight = SLOT
            .snapshot()
            .is_some_and(|(id, ready)| id == request_id && ready.is_none());
        if !in_flight {
            SLOT.clear(request_id);
        }
        PUBLIC_OWNER.release(request_id);
        Some(in_flight)
    }
}

/// macOS の effect ドライバ本体（`contact_pick::drive_callback` と同型）。
#[cfg(target_os = "macos")]
fn drive_callback(state: &EyeDropperState, sender: &CompletionSender<EyeDropperState>) {
    use fandhe_core::screens::eye_dropper_pick::{
        PickPhase, finish_pick, mark_picking, take_request,
    };

    if let Some(generation) = take_request(state) {
        let _ = sender.send(Box::new(mark_picking));
        // 公開入口の進行中・未取得の要求は置き換えず、画面側を処理中の失敗で終える。
        let Some(request_id) =
            bridge::SCREEN_OWNER.begin_excluding(&bridge::SLOT, &[&bridge::PUBLIC_OWNER])
        else {
            let outcome = to_outcome(Err(EyeDropperError::Internal("eye_dropper busy")));
            let _ = sender.send(Box::new(move |s: &mut EyeDropperState| {
                finish_pick(s, generation, outcome);
            }));
            return;
        };
        if !crate::macos::present(request_id) {
            // 起動依頼自体が同期的に失敗した（コールバックは二度と来ない）。
            bridge::deliver(
                request_id,
                Err(EyeDropperError::Internal("eye_dropper present")),
            );
        }
        return;
    }

    if state.phase != PickPhase::Picking {
        return;
    }
    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &bridge::SCREEN_OWNER,
        sender,
        |_request_id| {},
        move |s: &mut EyeDropperState, result| {
            // `Idle` へ既に戻っていて反映できない完了は、slot を残さず印も外す
            // （残すと公開入口が busy から抜けられない）。
            if s.phase != PickPhase::Idle {
                finish_pick(s, generation, to_outcome(result));
                true
            } else {
                let id = bridge::SCREEN_OWNER.current();
                bridge::SLOT.clear(id);
                bridge::SCREEN_OWNER.release(id);
                false
            }
        },
        // 自分の要求が置き換わった・失われた。待ち続けず失敗で終える（Idle なら何もしない）。
        move |s: &mut EyeDropperState| {
            if s.phase != PickPhase::Idle {
                finish_pick(
                    s,
                    generation,
                    to_outcome(Err(EyeDropperError::Internal("eye_dropper lost"))),
                );
            }
        },
    );
}

/// macOS・Web 以外: `EyeDropper` トレイト経由の同期呼び出し。
#[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
fn drive(
    state: &EyeDropperState,
    sender: &CompletionSender<EyeDropperState>,
    dropper: &dyn EyeDropper,
) {
    let Some(generation) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_picking));
    let outcome = to_outcome(dropper.pick().map_err(EyeDropperError::from_device));
    let _ = sender.send(Box::new(move |s: &mut EyeDropperState| {
        finish_pick(s, generation, outcome);
    }));
}

/// 同期の trait 経由で選べる OS 実装を返す。どの OS にも同期実装は無い（macOS の `NSColorSampler`・Web の
/// `EyeDropper.open()` は非同期のため受け口 `bridge` でしか受けられず、trait を通らない）ので、常に
/// `UnsupportedEyeDropper`（`Unsupported` を返し、成功へは倒さない）。
///
/// 画面の状態型に依存しない非同期の入口は [`begin_eye_dropper`] / [`poll_eye_dropper`] / [`discard_eye_dropper`]
/// （macOS・Web。フレームワークの外のアプリも使える）。画面 effect は [`eye_dropper_pick_effect`]。
pub fn platform_eye_dropper() -> Box<dyn EyeDropper> {
    Box::new(UnsupportedEyeDropper)
}

/// `registry.rs` の `effect` 区画から `eye_dropper` 画面用に配線される effect ドライバ。
/// wasm32 では同名の関数を `web.rs`（`Applier` 版）が公開する。
#[cfg(not(target_arch = "wasm32"))]
pub fn eye_dropper_pick_effect(
    state: &EyeDropperState,
    sender: &CompletionSender<EyeDropperState>,
) {
    #[cfg(target_os = "macos")]
    drive_callback(state, sender);
    #[cfg(not(target_os = "macos"))]
    drive(state, sender, &UnsupportedEyeDropper);
}

/// 画面の状態型に依存しない入口: 画面上の 1 点の色の取得を始め、要求 id を返す（macOS・Web）。結果は
/// [`poll_eye_dropper`] で 1 度だけ取り出す。使い終えない・結果を使わない要求は [`discard_eye_dropper`] で捨てる。
///
/// - 画面 effect の要求が進行中・結果未取得のとき、または自分の前の要求が進行中・結果未取得のときは
///   `Internal("eye_dropper busy")` で断る（画面 effect 側も、この入口の要求があれば処理中の失敗で終える）。
/// - 依頼自体の失敗は `Err`（結果は届かない）。他の OS・`EyeDropper` を持たないブラウザは `Unsupported`。
/// - Web はブラウザが利用者の操作（transient activation）の中の呼び出しだけを許すため、クリックなどの
///   ハンドラの中から同期で呼ぶこと。そうでなければ `NotAllowedError` で拒否され、失敗として poll に届く。
///   secure context に限られ、Chromium 系だけが対応する。
/// - 拾った色はログ・`Debug` に出さない。
#[cfg(any(target_os = "macos", target_arch = "wasm32"))]
pub fn begin_eye_dropper() -> Result<u64, EyeDropperError> {
    #[cfg(target_arch = "wasm32")]
    {
        crate::web::begin_public()
    }
    #[cfg(target_os = "macos")]
    {
        let request_id = bridge::PUBLIC_OWNER
            .begin_excluding(
                &bridge::SLOT,
                &[&bridge::SCREEN_OWNER, &bridge::PUBLIC_OWNER],
            )
            .ok_or(EyeDropperError::Internal("eye_dropper busy"))?;
        if !crate::macos::present(request_id) {
            bridge::SLOT.clear(request_id);
            bridge::PUBLIC_OWNER.release(request_id);
            return Err(EyeDropperError::Internal("eye_dropper present"));
        }
        Ok(request_id)
    }
}

/// 非対応 OS では成功へ倒さず `Unsupported`。
#[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
pub fn begin_eye_dropper() -> Result<u64, EyeDropperError> {
    Err(EyeDropperError::Unsupported)
}

/// [`begin_eye_dropper`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・公開入口でない id・
/// 取り出し済み・0・非対応 OS は `None`。slot の要求が失われたとき（reset 等）は 1 度だけ `Err(Internal("eye_dropper lost"))`、以後は `None`。結果は 3 値: 色（`Ok`）、利用者の取り消し（`Err(Cancelled)`）、失敗。
/// 並行して同じ id を poll しても、結果が二重に返ることはない。
pub fn poll_eye_dropper(request_id: u64) -> Option<Result<PickedColor, EyeDropperError>> {
    #[cfg(any(target_os = "macos", target_arch = "wasm32"))]
    {
        // 照合から解放までを直列にする。後から来た poll は先の poll が持ち主を外した後に走り、照合で `None` になる。
        let _guard = bridge::PUBLIC_POLL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if request_id == 0 || bridge::PUBLIC_OWNER.current() != request_id {
            return None;
        }
        if let Some(result) = bridge::SLOT.take_ready(request_id) {
            bridge::PUBLIC_OWNER.release(request_id);
            return Some(result);
        }
        // slot が空・別の要求に替わっていれば喪失（reset 等）。未着と区別して 1 度だけ失敗で返し、以後は `None`。
        if bridge::SLOT
            .snapshot()
            .is_some_and(|(id, _)| id == request_id)
        {
            return None;
        }
        bridge::PUBLIC_OWNER.release(request_id);
        Some(Err(EyeDropperError::Internal("eye_dropper lost")))
    }
    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    {
        let _ = request_id;
        None
    }
}

/// [`begin_eye_dropper`] の自分の要求を捨てる。`request_id` が公開入口の現在の要求のときだけ `true`
/// （他人の id・取り出し済み・0・非対応 OS は `false` で何もしない）。以後 [`poll_eye_dropper`] は `None`。
///
/// 結果が届いていれば slot も空ける。進行中のときの扱いが OS で違う: Web は `AbortController` で中断し、
/// `AbortError`（取り消し）が確定するまで次の開始を busy で断る。macOS は `NSColorSampler` を閉じる API が無いため、
/// 利用者が選ぶか Esc を押してサンプラーが閉じるまで、次の開始（画面・公開とも）を busy で断る。
/// 閉じたあとの届いた結果は、持ち主がいないため次の開始が置き換える。
pub fn discard_eye_dropper(request_id: u64) -> bool {
    #[cfg(any(target_os = "macos", target_arch = "wasm32"))]
    {
        let Some(in_flight) = bridge::discard_public(request_id) else {
            return false;
        };
        // 中断はロックの外で行う（中断が同期的に slot へ結果を届けうるため、ロックを握ったまま呼ばない）。
        #[cfg(target_arch = "wasm32")]
        if in_flight {
            crate::web::abort_public(request_id);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = in_flight;
        true
    }
    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    {
        let _ = request_id;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(hex: &str) -> PickedColor {
        fandhe_device::eye_dropper::color_from_srgb_hex(hex).unwrap()
    }

    #[test]
    fn to_outcome_maps_all_branches() {
        assert_eq!(
            to_outcome(Ok(color("#AABBCC"))),
            EyeDropperOutcome::Picked("#aabbcc".into())
        );
        assert_eq!(
            to_outcome(Err(EyeDropperError::Cancelled)),
            EyeDropperOutcome::Cancelled
        );
        assert_eq!(
            to_outcome(Err(EyeDropperError::Unsupported)),
            EyeDropperOutcome::Unsupported
        );
        assert!(matches!(
            to_outcome(Err(EyeDropperError::Internal("x"))),
            EyeDropperOutcome::Failed(_)
        ));
        // 権限拒否が来ても成功にならない（`from_device` が Internal に畳む）。
        assert!(matches!(
            to_outcome(Err(EyeDropperError::from_device(
                fandhe_device::DeviceError::PermissionDenied("eye_dropper")
            ))),
            EyeDropperOutcome::Failed(_)
        ));
    }

    /// macOS: `present` は NSApp を要るので呼ばず、owner と slot を直接使う。slot は静的なので直列に走らせる。
    #[cfg(target_os = "macos")]
    mod macos_bridge {
        use super::*;
        use std::sync::Mutex;

        static SERIAL: Mutex<()> = Mutex::new(());

        fn reset() {
            bridge::SLOT.reset();
            bridge::SCREEN_OWNER.release(bridge::SCREEN_OWNER.current());
            bridge::PUBLIC_OWNER.release(bridge::PUBLIC_OWNER.current());
        }

        fn begin_public() -> Option<u64> {
            bridge::PUBLIC_OWNER.begin_excluding(
                &bridge::SLOT,
                &[&bridge::SCREEN_OWNER, &bridge::PUBLIC_OWNER],
            )
        }

        fn begin_screen() -> Option<u64> {
            bridge::SCREEN_OWNER.begin_excluding(&bridge::SLOT, &[&bridge::PUBLIC_OWNER])
        }

        #[test]
        fn stale_and_duplicate_delivery_never_becomes_success() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            reset();
            let id = begin_public().unwrap();
            assert!(!bridge::deliver(id.wrapping_add(1), Ok(color("#aabbcc"))));
            assert!(bridge::deliver(id, Err(EyeDropperError::Cancelled)));
            // 二重通知は first-wins で、成功へ化けない。
            assert!(!bridge::deliver(id, Ok(color("#aabbcc"))));
            assert_eq!(
                bridge::SLOT.snapshot(),
                Some((id, Some(Err(EyeDropperError::Cancelled))))
            );
            reset();
        }

        #[test]
        fn screen_and_public_exclude_each_other() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            reset();
            let public = begin_public().unwrap();
            assert!(begin_screen().is_none());
            assert!(begin_public().is_none());
            assert!(bridge::deliver(public, Ok(color("#010203"))));
            // 結果が未取得のうちも、相手は置き換えられない。
            assert!(begin_screen().is_none());
            assert_eq!(poll_eye_dropper(public), Some(Ok(color("#010203"))));
            let screen = begin_screen().unwrap();
            assert!(begin_public().is_none());
            assert_ne!(screen, public);
            reset();
        }

        #[test]
        fn poll_returns_once_and_rejects_foreign_ids() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            reset();
            let id = begin_public().unwrap();
            assert_eq!(poll_eye_dropper(id), None);
            assert_eq!(poll_eye_dropper(0), None);
            assert_eq!(poll_eye_dropper(id.wrapping_add(1)), None);
            assert!(bridge::deliver(id, Ok(color("#aabbcc"))));
            assert_eq!(poll_eye_dropper(id), Some(Ok(color("#aabbcc"))));
            assert_eq!(poll_eye_dropper(id), None);
            // 画面側の id は公開入口から取れない。
            let screen = begin_screen().unwrap();
            assert!(bridge::deliver(screen, Ok(color("#aabbcc"))));
            assert_eq!(poll_eye_dropper(screen), None);
            reset();
        }

        #[test]
        fn poll_reports_lost_once_when_slot_was_reset() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            reset();
            let id = begin_public().unwrap();
            bridge::SLOT.reset();
            assert_eq!(
                poll_eye_dropper(id),
                Some(Err(EyeDropperError::Internal("eye_dropper lost")))
            );
            assert_eq!(poll_eye_dropper(id), None);
            // 持ち主が外れたので、次の開始は busy にならない。
            assert!(begin_public().is_some());
            reset();
        }

        #[test]
        fn discard_in_flight_keeps_slot_busy_until_delivered() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            reset();
            let id = begin_public().unwrap();
            assert!(discard_eye_dropper(id));
            assert!(!discard_eye_dropper(id));
            assert_eq!(poll_eye_dropper(id), None);
            // サンプラーが開いたままなので、画面・公開とも開始できない。
            assert!(begin_public().is_none());
            assert!(begin_screen().is_none());
            // OS が閉じて結果が届くと（持ち主がいない結果は）置き換えられる。
            assert!(bridge::deliver(id, Err(EyeDropperError::Cancelled)));
            assert!(begin_public().is_some());
            reset();
        }

        #[test]
        fn discard_after_ready_frees_slot() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            reset();
            let id = begin_public().unwrap();
            assert!(bridge::deliver(id, Ok(color("#aabbcc"))));
            assert!(discard_eye_dropper(id));
            assert_eq!(bridge::SLOT.snapshot(), None);
            assert!(begin_screen().is_some());
            reset();
        }
    }

    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    #[test]
    fn public_entry_is_unsupported_not_success() {
        assert_eq!(begin_eye_dropper(), Err(EyeDropperError::Unsupported));
        assert_eq!(poll_eye_dropper(1), None);
        assert!(!discard_eye_dropper(1));
    }

    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    #[test]
    fn unsupported_is_reflected_not_success() {
        use fandhe_core::screens::eye_dropper_pick::{PickPhase, UNSUPPORTED_TEXT, request_pick};
        struct NoopWaker;
        impl fandhe_runtime::RepaintWaker for NoopWaker {
            fn wake(&self) {}
            fn wake_after(&self, _delay: std::time::Duration) {}
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        let mut state = EyeDropperState::default();
        request_pick(&mut state);
        eye_dropper_pick_effect(&state, &sender);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut state);
        }
        assert_eq!(state.phase, PickPhase::Idle);
        assert!(state.picked.is_none());
        assert_eq!(state.error, UNSUPPORTED_TEXT);
    }
}
