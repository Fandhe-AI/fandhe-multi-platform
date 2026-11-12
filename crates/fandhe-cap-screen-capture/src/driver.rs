//! Phase 11 の拡充ループ（親 #1135、Issue #1335）: 画面の録画・キャプチャの effect ドライバ。
//! `fandhe_core::screens::screen_capture`（端末依存ゼロ）の要求を観測し、OS の録画エンジン
//! （`crate::contract::ScreenCapturer`）へ依頼して、結果を状態機械へ書き戻す
//! （`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から呼ばれる、`app::EffectFn` 参照）。
//! 契約（三値・守る範囲）の SSOT は `fandhe_cap_screen_capture` のモジュール doc。
//!
//! # 4 OS は pull 型、それ以外は同期
//!
//! 同意 UI と録画完了が非同期に届くため、macOS / iOS / Android / Windows は `mic_record` と
//! 同じ pull 型（[`bridge`] = `RequestSlot` の facade、`poll_and_finish` で反映）。
//! それ以外（Linux 等）は `UnsupportedScreenCapturer` を同期で呼ぶ [`drive`] だけ。
//!
//! # OS 別の置き場（後続の子はここの dispatch を触らない）
//!
//! - macOS: `apple_screen_capture.rs` + `apple/fandhe_apple_screen_capture.m`（#1335、実装済み）
//! - iOS: `apple_screen_capture.rs` + `apple/fandhe_apple_screen_capture.m` の ReplayKit 側（#1336、実装済み）
//! - Android: `android_screen_capture.rs` + `ScreenCaptureHelper.java` / `ScreenCaptureService.java`（#1337、実装済み）
//! - Windows: `windows_screen_capture.rs`（Issue #26、`Windows.Graphics.Capture` + `MediaTranscoder`、実装済み）
//!
//! 結果コード表（ネイティブ → [`error_from_code`]）は `.m` ファイル冒頭と同じ。

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
use std::time::Instant;

use crate::contract::ScreenCapturer;
use crate::error::ScreenCaptureError;
use fandhe_core::screens::screen_capture::{CaptureOutcome, CaptureSummary};
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
use fandhe_core::screens::screen_capture::{
    CapturePhase, MAX_CAPTURE_MS, ScreenCaptureState, finish_capture, mark_recording,
    mark_stopping, take_start_request, take_stop_request, tick,
};
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
)))]
use fandhe_core::screens::screen_capture::{
    ScreenCaptureState, finish_capture, mark_recording, take_start_request,
};

use fandhe_runtime::CompletionSender;

/// ネイティブ → [`ScreenCaptureError`] の写像（SSOT。`.m` / 将来の Java / Web も同じ表）。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 1 | 権限拒否（TCC） | `PermissionDenied` |
/// | 3 | 二重開始 | `Internal("screen_capture busy")` |
/// | 4 | 同意中に停止（キャンセル） | `Cancelled` |
/// | 5 | ライフサイクル破棄 | `Internal("screen_capture interrupted")` |
/// | 6 | API が無い（macOS 15 未満・iOS の `isAvailable == false`） | `Unsupported` |
/// | 他 | 内部エラー | `Internal("screen_capture")` |
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn error_from_code(code: i32) -> ScreenCaptureError {
    match code {
        1 => ScreenCaptureError::PermissionDenied,
        3 => ScreenCaptureError::Internal("screen_capture"),
        4 => ScreenCaptureError::Cancelled,
        5 => ScreenCaptureError::Internal("screen_capture"),
        6 => ScreenCaptureError::Unsupported,
        _ => ScreenCaptureError::Internal("screen_capture"),
    }
}

/// Windows の `HRESULT` → [`ScreenCaptureError`] の写像。`E_ACCESSDENIED` だけ権限拒否、
/// それ以外は `stage` の固定ラベルで `Internal` に畳む（生の HRESULT は出さない、
/// security.md A09）。Windows 限定モジュールから呼ばれるが、ホストの `cargo test` で
/// 固定するため `cfg` は付けない（`mic_record::error_from_hresult` と同じ方針）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn error_from_hresult(code: i32, stage: &'static str) -> ScreenCaptureError {
    const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
    if code == E_ACCESSDENIED {
        ScreenCaptureError::PermissionDenied
    } else {
        ScreenCaptureError::Internal(stage)
    }
}

/// 録画対象のサイズ → H.264 エンコーダへ渡す偶数サイズ。0 以下は `Internal`、
/// 奇数は 1 つ切り下げる（4:2:0 は幅・高さが偶数でないと受け付けない）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn even_capture_size(width: i32, height: i32) -> Result<(u32, u32), ScreenCaptureError> {
    let (w, h) = (width & !1, height & !1);
    if w <= 0 || h <= 0 {
        return Err(ScreenCaptureError::Internal("screen_capture"));
    }
    Ok((w as u32, h as u32))
}

/// OS からの結果 → 状態機械へ渡す形。キャンセルと非対応は失敗文言に畳まない
/// （`ScreenCaptureError` の内部構造は画面へ出さない、security.md A09）。
fn to_outcome(
    result: Result<crate::contract::CaptureResult, ScreenCaptureError>,
) -> Result<CaptureOutcome, String> {
    match result {
        Ok(r) => Ok(CaptureOutcome::Captured(CaptureSummary {
            size_bytes: r.size_bytes,
            mime: r.mime,
            duration_ms: r.duration_ms,
        })),
        Err(ScreenCaptureError::Cancelled) => Ok(CaptureOutcome::Cancelled),
        Err(ScreenCaptureError::Unsupported) => Ok(CaptureOutcome::Unsupported),
        Err(other) => Err(other.to_string()),
    }
}

/// OS 側コールバックとの橋渡し（`fandhe-runtime/src/request_slot.rs::RequestSlot` の facade）。
/// 1 回の録画につき `deliver` は 1 回だけ有効（first-wins）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub(crate) mod bridge {
    use fandhe_runtime::request_slot::RequestSlot;

    /// OS の生の結果。画面用への写像は取り出す側が行う
    /// （画面経由でも公開入口 [`super::poll_capture`] でも同じ slot を使う）。
    pub(crate) type Outcome =
        Result<crate::contract::CaptureResult, crate::error::ScreenCaptureError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 実際に録画が始まった request_id（`0` は未開始）。同意ダイアログ中は経過時間を
    /// 進めないための合図。
    static STARTED_REQUEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// ネイティブが録画を開始した瞬間に呼ぶ（任意スレッド）。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn mark_started(request_id: u64) {
        STARTED_REQUEST_ID.store(request_id, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn has_started(request_id: u64) -> bool {
        STARTED_REQUEST_ID.load(std::sync::atomic::Ordering::Relaxed) == request_id
    }

    pub(crate) fn current_request_id() -> Option<u64> {
        SLOT.snapshot().map(|(request_id, _)| request_id)
    }

    /// 結果を置く。`request_id` 不一致・未登録・既に結果ありは捨てて `false`。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<crate::contract::CaptureResult, crate::error::ScreenCaptureError>,
    ) -> bool {
        SLOT.deliver(request_id, result)
    }

    /// 開始失敗を即確定する（スタブの `Unsupported` もこの経路）。
    pub(crate) fn deliver_error(request_id: u64, e: crate::error::ScreenCaptureError) -> bool {
        deliver(request_id, Err(e))
    }

    /// Android の Activity 再生成後に呼ぶ（`mic_record::bridge::reset` と同じ位置づけ）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset();
        STARTED_REQUEST_ID.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// OS 別エンジンの dispatch（最終形。後続 OS の子 issue は各 OS のモジュール内だけを直す）。
pub fn platform_screen_capture() -> &'static dyn ScreenCapturer {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        crate::apple::capturer()
    }
    #[cfg(target_os = "android")]
    {
        crate::android::capturer()
    }
    #[cfg(target_os = "windows")]
    {
        crate::windows::capturer()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        &crate::contract::UnsupportedScreenCapturer
    }
}

/// 直近呼び出し時刻（経過時間の実測用。`speech_recognition` と同様に自前で持つ）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
static LAST_TICK: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

/// 画面に依存しない公開入口（4 OS）。録画を依頼して要求 id を返す。
/// 結果は [`poll_capture`] で受け取り、止めるときは `platform_screen_capture().stop(id)`。
/// 依頼自体が失敗したら `Err`（コールバックは来ない）。保留は 1 件だけで、進行中に新しい依頼を出すと `Internal("busy")`（未取得の結果は捨てる）
/// （画面経由の依頼と slot を共有する）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub fn begin_capture(max_duration: std::time::Duration) -> Result<u64, ScreenCaptureError> {
    // 進行中の依頼があれば置き換えず拒否する（失敗時の clear で既存依頼を壊さない）。
    if matches!(bridge::SLOT.snapshot(), Some((_, None))) {
        return Err(ScreenCaptureError::Internal("busy"));
    }
    let request_id = bridge::begin();
    match platform_screen_capture().start(request_id, max_duration) {
        Ok(()) => Ok(request_id),
        Err(e) => {
            bridge::SLOT.clear(request_id);
            Err(e)
        }
    }
}

/// [`begin_capture`] の結果を取り出す。未着なら `None`（次フレーム以降に再度呼ぶ）。
/// 取り出すと保留は解放される。`request_id` が現在の依頼でなければ `None`。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub fn poll_capture(
    request_id: u64,
) -> Option<Result<crate::contract::CaptureResult, ScreenCaptureError>> {
    let (id, ready) = bridge::SLOT.snapshot()?;
    if id != request_id {
        return None;
    }
    let result = ready?;
    bridge::SLOT.clear(id);
    Some(result)
}

/// 4 OS 経路の effect ドライバ本体。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
fn drive_callback(state: &ScreenCaptureState, sender: &CompletionSender<ScreenCaptureState>) {
    if take_start_request(state).is_some() {
        let _ = sender.send(Box::new(mark_recording));
        let request_id = bridge::begin();
        let max = std::time::Duration::from_millis(MAX_CAPTURE_MS);
        if let Err(e) = platform_screen_capture().start(request_id, max) {
            // 依頼自体が失敗した（コールバックは二度と来ない）。
            bridge::deliver_error(request_id, e);
        }
        return;
    }

    if take_stop_request(state).is_some() {
        let _ = sender.send(Box::new(mark_stopping));
        // 停止依頼が届かなくても（既に終了済み等）自動停止か既存の結果配送が追随する。
        if let Some(request_id) = bridge::current_request_id() {
            let _ = platform_screen_capture().stop(request_id);
        }
        return;
    }

    // 経過秒数は画面側の時刻で進める。同意ダイアログ中（未開始）は進めない。
    let started = bridge::current_request_id().is_some_and(bridge::has_started);
    let dt_ms = fandhe_runtime::tick::measure_dt_ms(
        &LAST_TICK,
        state.phase == CapturePhase::Recording && started,
    );
    if dt_ms > 0 {
        let _ = sender.send(Box::new(move |s: &mut ScreenCaptureState| tick(s, dt_ms)));
    }

    if state.phase != CapturePhase::Recording && state.phase != CapturePhase::Stopping {
        return;
    }
    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut ScreenCaptureState, raw| {
            // 反映できる状態か確かめてから確定し、反映できたときだけ `true`（clear させる）。
            if s.phase != CapturePhase::Idle {
                finish_capture(s, generation, to_outcome(raw));
                true
            } else {
                false
            }
        },
    );
}

/// 4 OS 以外向け: 同期呼び出し。常に `Unsupported`。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
)))]
fn drive(
    state: &ScreenCaptureState,
    sender: &CompletionSender<ScreenCaptureState>,
    capturer: &dyn crate::contract::ScreenCapturer,
) {
    let Some(request) = take_start_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_recording));
    let max = std::time::Duration::from_millis(request.max_duration_ms);
    let err = capturer
        .start(0, max)
        .and_then(|()| capturer.stop(0))
        .err()
        .unwrap_or(ScreenCaptureError::Internal("screen_capture"));
    let result = to_outcome(Err(err));
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut ScreenCaptureState| {
        finish_capture(s, generation, result);
    }));
}

/// `registry.rs` の `effect` 区画から配線される effect ドライバ（4 OS 版）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub fn screen_capture_effect(
    state: &ScreenCaptureState,
    sender: &CompletionSender<ScreenCaptureState>,
) {
    drive_callback(state, sender);
}

/// `registry.rs` の `effect` 区画から配線される effect ドライバ（4 OS 以外。常に非対応）。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
)))]
pub fn screen_capture_effect(
    state: &ScreenCaptureState,
    sender: &CompletionSender<ScreenCaptureState>,
) {
    drive(state, sender, &crate::contract::UnsupportedScreenCapturer);
}

/// `android_main` から呼ぶ（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::build_capture_result;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    #[allow(dead_code)]
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    #[test]
    fn begin_capture_rejects_while_in_flight_and_keeps_existing() {
        let id = bridge::SLOT.begin();
        assert!(begin_capture(std::time::Duration::from_secs(1)).is_err());
        assert!(bridge::deliver(id, Err(ScreenCaptureError::Cancelled)));
        assert!(matches!(
            poll_capture(id),
            Some(Err(ScreenCaptureError::Cancelled))
        ));
    }

    #[test]
    fn hresult_access_denied_is_permission_denied() {
        assert!(matches!(
            error_from_hresult(0x8007_0005_u32 as i32, "x"),
            ScreenCaptureError::PermissionDenied
        ));
        assert!(matches!(
            error_from_hresult(0x8000_4005_u32 as i32, "stage"),
            ScreenCaptureError::Internal("stage")
        ));
    }

    #[test]
    fn even_size_rounds_down_and_rejects_empty() {
        assert_eq!(even_capture_size(1921, 1081).unwrap(), (1920, 1080));
        assert!(even_capture_size(1, 1080).is_err());
        assert!(even_capture_size(0, 0).is_err());
        assert!(even_capture_size(-2, 100).is_err());
    }

    #[test]
    fn error_from_code_maps_table() {
        assert!(matches!(
            error_from_code(1),
            ScreenCaptureError::PermissionDenied
        ));
        assert!(matches!(
            error_from_code(3),
            ScreenCaptureError::Internal(_)
        ));
        assert!(matches!(error_from_code(4), ScreenCaptureError::Cancelled));
        assert!(matches!(
            error_from_code(5),
            ScreenCaptureError::Internal(_)
        ));
        assert!(matches!(
            error_from_code(6),
            ScreenCaptureError::Unsupported
        ));
        assert!(matches!(
            error_from_code(2),
            ScreenCaptureError::Internal(_)
        ));
        assert!(matches!(
            error_from_code(999),
            ScreenCaptureError::Internal(_)
        ));
    }

    #[test]
    fn to_outcome_separates_three_values() {
        assert_eq!(
            to_outcome(Err(ScreenCaptureError::Cancelled)),
            Ok(CaptureOutcome::Cancelled)
        );
        assert_eq!(
            to_outcome(Err(ScreenCaptureError::Unsupported)),
            Ok(CaptureOutcome::Unsupported)
        );
        assert!(to_outcome(Err(ScreenCaptureError::PermissionDenied)).is_err());
        // 0 バイトは成功にならない。
        assert!(to_outcome(build_capture_result(0, "video/mp4".into(), 1_000)).is_err());
        assert!(matches!(
            to_outcome(build_capture_result(10, "video/mp4".into(), 1_000)),
            Ok(CaptureOutcome::Captured(_))
        ));
    }

    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    ))]
    #[test]
    fn stale_request_id_deliver_is_dropped() {
        let id = bridge::begin();
        assert!(!bridge::deliver(
            id + 1,
            build_capture_result(10, "video/mp4".into(), 1_000)
        ));
        assert!(bridge::deliver(
            id,
            build_capture_result(10, "video/mp4".into(), 1_000)
        ));
        // first-wins: 2 回目は捨てる。
        assert!(!bridge::deliver(
            id,
            build_capture_result(10, "video/mp4".into(), 1_000)
        ));
        bridge::reset();
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    #[test]
    fn drive_reports_unsupported() {
        use fandhe_core::screens::screen_capture::{CapturePhase, start_capture};

        let mut state = ScreenCaptureState::default();
        start_capture(&mut state);
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        drive(&state, &sender, &crate::contract::UnsupportedScreenCapturer);
        rx.try_recv().expect("mark_recording")(&mut state);
        assert_eq!(state.phase, CapturePhase::Recording);
        rx.try_recv().expect("finish_capture")(&mut state);
        assert_eq!(state.phase, CapturePhase::Idle);
        assert_eq!(
            state.status,
            fandhe_core::screens::screen_capture::UNSUPPORTED_TEXT
        );
    }
}
