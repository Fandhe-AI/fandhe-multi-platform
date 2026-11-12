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

    /// ネイティブが録画を開始した瞬間に呼ぶ（任意スレッド）。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn mark_started(request_id: u64) {
        STARTED_REQUEST_ID.store(request_id, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn has_started(request_id: u64) -> bool {
        STARTED_REQUEST_ID.load(std::sync::atomic::Ordering::Relaxed) == request_id
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
/// 依頼自体が失敗したら `Err`（コールバックは来ない）。保留は 1 件だけで、進行中、または結果が届いて未取得の間に
/// 新しい依頼を出すと `Internal("busy")` で拒否する（置き換えない。画面経由の依頼と slot を共有する）。
/// 結果は [`poll_capture`] で取り出すか、不要なら [`abandon_capture`] で捨てる（どちらもしないと、再生成まで busy のまま）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub fn begin_capture(max_duration: std::time::Duration) -> Result<u64, ScreenCaptureError> {
    // 進行中の依頼、または未取得の結果（画面側・公開側）があれば置き換えず拒否する（失敗時の clear で既存依頼を壊さない）。
    let Some(request_id) =
        PUBLIC_OWNER.begin_excluding(&bridge::SLOT, &[&SCREEN_OWNER, &PUBLIC_OWNER])
    else {
        return Err(ScreenCaptureError::Internal("busy"));
    };
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
    PUBLIC_OWNER.release(id);
    Some(result)
}

/// [`begin_capture`] の結果が届いているのに取り出さない（受け手が不要になった）とき、その結果を捨てて保留を解放する。
/// 結果が届いている `request_id` なら `true`。未着（録画中）・別の id なら何もせず `false`
/// （録画中のものは `platform_screen_capture().stop(id)` で止め、届いた結果をこの関数で捨てる）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub fn abandon_capture(request_id: u64) -> bool {
    match bridge::SLOT.snapshot() {
        Some((id, Some(_))) if id == request_id => {
            bridge::SLOT.clear(id);
            PUBLIC_OWNER.release(id);
            true
        }
        _ => false,
    }
}

/// 公開入口 [`begin_capture`] が追う自分の分の要求。結果が届いて [`poll_capture`] が取り出すまで、
/// 画面側がその結果を置き換えない（`SCREEN_OWNER` と対称）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
static PUBLIC_OWNER: fandhe_runtime::request_slot::SlotOwner =
    fandhe_runtime::request_slot::SlotOwner::new();

/// 画面用 effect が追う自分の分の要求（公開入口 [`begin_capture`] と slot を共有するため）。
/// 停止・経過時間の対象 id もこちらを使い、公開入口の録画を巻き込まない。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
static SCREEN_OWNER: fandhe_runtime::request_slot::SlotOwner =
    fandhe_runtime::request_slot::SlotOwner::new();

/// 公開入口などの先行が進行中で、画面の依頼を受けられないときの表示。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
const BUSY_MESSAGE: &str = "他の画面録画が処理中です。完了後にもう一度お試しください";

/// 依頼が失われた（再生成・別の依頼への置き換え）ときの表示。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
const LOST_MESSAGE: &str = "画面録画が中断されました。もう一度お試しください";

/// 4 OS 経路の effect ドライバ本体。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
fn drive_callback(state: &ScreenCaptureState, sender: &CompletionSender<ScreenCaptureState>) {
    if let Some(request) = take_start_request(state) {
        let _ = sender.send(Box::new(mark_recording));
        let generation = request.generation;
        // 先行（公開入口の録画など）を置き換えず、画面には失敗として返す（黙って捨てない）。
        let Some(request_id) = SCREEN_OWNER.begin_excluding(&bridge::SLOT, &[&PUBLIC_OWNER]) else {
            let _ = sender.send(Box::new(move |s: &mut ScreenCaptureState| {
                finish_capture(s, generation, Err(BUSY_MESSAGE.to_string()));
            }));
            return;
        };
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
        let request_id = SCREEN_OWNER.current();
        if request_id != 0 {
            let _ = platform_screen_capture().stop(request_id);
        }
        return;
    }

    // 経過秒数は画面側の時刻で進める。同意ダイアログ中（未開始）は進めない。
    let mine = SCREEN_OWNER.current();
    let started = mine != 0 && bridge::has_started(mine);
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
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &SCREEN_OWNER,
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
        move |s: &mut ScreenCaptureState| {
            finish_capture(s, generation, Err(LOST_MESSAGE.to_string()));
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

    /// 静的 `bridge::SLOT` を触るテストを直列化する（並列実行で request id を上書きし合わない）。
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    ))]
    static SLOT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    ))]
    #[test]
    fn begin_capture_rejects_while_in_flight_and_keeps_existing() {
        let _g = SLOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let id = bridge::SLOT.try_begin().expect("空き");
        assert!(begin_capture(std::time::Duration::from_secs(1)).is_err());
        assert!(bridge::deliver(id, Err(ScreenCaptureError::Cancelled)));
        assert!(matches!(
            poll_capture(id),
            Some(Err(ScreenCaptureError::Cancelled))
        ));
        bridge::reset();
    }

    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    ))]
    #[test]
    fn screen_request_does_not_replace_public_capture_and_owner_is_separate() {
        let _g = SLOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        // 公開入口の録画が進行中のとき、画面の登録は拒否され、公開側の id はそのまま。
        let public = bridge::SLOT.try_begin().expect("空き");
        assert!(
            SCREEN_OWNER
                .begin_excluding(&bridge::SLOT, &[&PUBLIC_OWNER])
                .is_none()
        );
        assert!(bridge::SLOT.snapshot().is_some_and(|(id, _)| id == public));
        // 画面の停止・経過時間の対象は公開側の id を指さない。
        assert_ne!(SCREEN_OWNER.current(), public);
        bridge::reset();
    }

    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    ))]
    #[test]
    fn unconsumed_public_result_is_not_replaced_by_screen_or_next_public() {
        let _g = SLOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let id = PUBLIC_OWNER
            .begin_excluding(&bridge::SLOT, &[&SCREEN_OWNER, &PUBLIC_OWNER])
            .expect("空き");
        // OS の結果が届いて、公開側がまだ取り出していない。
        assert!(bridge::deliver(id, Err(ScreenCaptureError::Cancelled)));
        assert!(
            SCREEN_OWNER
                .begin_excluding(&bridge::SLOT, &[&PUBLIC_OWNER])
                .is_none()
        );
        assert!(
            PUBLIC_OWNER
                .begin_excluding(&bridge::SLOT, &[&SCREEN_OWNER, &PUBLIC_OWNER])
                .is_none()
        );
        // 取り出して clear した後は登録できる。
        bridge::SLOT.clear(id);
        assert!(
            PUBLIC_OWNER
                .begin_excluding(&bridge::SLOT, &[&SCREEN_OWNER, &PUBLIC_OWNER])
                .is_some()
        );
        bridge::reset();
    }

    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    ))]
    #[test]
    fn abandoned_public_result_is_recovered_and_poll_releases_owner() {
        let _g = SLOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let id = PUBLIC_OWNER
            .begin_excluding(&bridge::SLOT, &[&SCREEN_OWNER, &PUBLIC_OWNER])
            .expect("空き");
        // 録画中は捨てられない。
        assert!(!abandon_capture(id));
        assert!(bridge::deliver(id, Err(ScreenCaptureError::Cancelled)));
        // 放棄された結果は回収でき、画面側からも公開側からも再び依頼できる。
        assert!(abandon_capture(id));
        assert_eq!(PUBLIC_OWNER.current(), 0);
        let id2 = SCREEN_OWNER
            .begin_excluding(&bridge::SLOT, &[&PUBLIC_OWNER])
            .expect("回収後は空き");
        // 取り出した場合も印が外れる。
        bridge::reset();
        let id3 = PUBLIC_OWNER
            .begin_excluding(&bridge::SLOT, &[&SCREEN_OWNER, &PUBLIC_OWNER])
            .expect("空き");
        assert!(bridge::deliver(id3, Err(ScreenCaptureError::Cancelled)));
        assert!(poll_capture(id3).is_some());
        assert_eq!(PUBLIC_OWNER.current(), 0);
        let _ = id2;
        bridge::reset();
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
        let _g = SLOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let id = bridge::SLOT.begin();
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
