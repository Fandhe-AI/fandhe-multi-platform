//! Phase 11 の拡充ループ（親 #953、Issue #954・#956）: 型サンプル「マイク録音」の
//! effect ドライバ。`fandhe_core::screens::mic_record`（端末依存ゼロ）の要求を観測し、
//! 結果を `CompletionSender` 経由で返す（`crate::screens::registry` の `effect`
//! 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # macOS/iOS/Android/Windows: pull 型 `bridge`（`biometric_auth.rs`/`share.rs` と同型）
//!
//! macOS/iOS の権限確認（`AVCaptureDevice.requestAccessForMediaType:`）・Android の
//! `RECORD_AUDIO` ランタイム許可ダイアログ、および録音完了（利用者の「停止」操作、
//! `MediaRecorder`/`AVAudioRecorder` の自動停止、`onPause`/`onDestroy` による中断）は
//! いずれも非同期に届く。Windows（Issue #955）は権限プロンプト自体は出ないが、
//! `MediaCapture` の初期化・録音開始・停止がいずれも `IAsyncAction`/
//! `IAsyncOperation` の非同期 API で、同一フレーム内で `start`→`stop` を
//! 呼び切る同期 `drive` では `duration_ms == 0` になり必ず失敗するため
//! （`device/windows_mic_record.rs` モジュール doc 参照）、macOS/iOS/Android と同じ
//! pull 型に乗せる。4 OS とも `MicRecorder` トレイトを **通らず**コアの状態機械へ
//! 直接書き戻す（`fandhe_device::mic_record` モジュール doc 参照。Windows の
//! `WindowsMicRecorder` はエンジンとしてのみ `MicRecorder` を実装し、駆動は
//! `device/windows_mic_record.rs::present`/`request_stop` が担う）。[`bridge`] は
//! `device/request_slot.rs::RequestSlot` の facade で、`CompletionSender` も
//! 画面の `generation` も保持しない pull 型。1 回の録音セッション（開始 →
//! 自動停止・明示停止・ライフサイクル中断のいずれか片方が届く）につき
//! `deliver` は 1 回だけ呼ばれる契約（`RequestSlot` の first-wins）。
//! [`present`]/[`request_stop`] が macOS/iOS/Android の実装（macOS/iOS:
//! `apple_mic_record`、Android: `android::request_mic_record_start`/
//! `android::request_mic_record_stop`）へ振り分け、Windows は
//! `windows_mic_record::present`/`request_stop` を `drive_callback` から直接呼ぶ
//! （エラー型が `Result<(), DeviceError>` で macOS/iOS/Android の `bool` 戻り値と
//! 異なるため、共通の [`present`]/[`request_stop`] には含めない）。
//!
//! # macOS/iOS/Android/Windows 以外: `MicRecorder` トレイト経由（`UnsupportedMicRecorder` 固定）
//!
//! Web（#958）/Linux は `fandhe_device::mic_record::UnsupportedMicRecorder` を
//! 経由し、固定文言（[`UNAVAILABLE`]）を表示する（将来横展開する Issue がこの
//! 分岐を実装へ差し替える）。

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
use std::time::Instant;

use fandhe_core::screens::mic_record::{
    finish_record, mark_recording, take_start_request, MicRecordState, RecordOutcome, RecordSummary,
};
// RecordPhase は macOS/iOS/Android/Windows の `drive_callback`（本番コード）と、
// それ以外の `drive_reports_unsupported_via_finish_record`（テストのみ）から使う。
// macOS/iOS/Android/Windows 以外の本番コード（`drive`）は中間状態を持たないため
// 参照しない（`RecordPhase` を常時インポートするとその通常ビルドで unused import
// になる、CI clippy 指摘参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android",
    test
))]
use fandhe_core::screens::mic_record::RecordPhase;
// mark_stopping/take_stop_request/tick/MAX_RECORD_MS は macOS/iOS/Android/Windows の
// pull 型 `drive_callback` でのみ使う（それ以外は同期 `drive` が start/stop を
// まとめて呼び切るため中間状態を持たない、モジュール doc 参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
use fandhe_core::screens::mic_record::{mark_stopping, take_stop_request, tick, MAX_RECORD_MS};
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
)))]
use fandhe_device::mic_record::MicRecorder;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `MicRecorder`/`bridge::deliver` 未対応環境（Unsupported）で画面へ出す固定案内
/// （`DeviceError` の内部構造は含めない、security.md A09。`device/biometric_auth.rs::
/// UNAVAILABLE` と同型）。
const UNAVAILABLE: &str = "この環境ではマイク録音を使えません";

/// `DeviceError` を画面へ渡す文言へ変換する。`Unsupported` は固定文言へ畳み、
/// それ以外は `Display` の文言をそのまま転送する（`device/biometric_auth.rs::
/// to_message` と同じ方針）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => UNAVAILABLE.to_string(),
        other => other.to_string(),
    }
}

/// `fandhe_device::mic_record::Recording`（生バイト列を保持）→ 画面表示用の
/// [`RecordSummary`]（サイズのみ、生バイト列は保持しない）への変換
/// （`fandhe-core` は `fandhe-device` に依存しないため、写像は本クレート側の責務）。
fn to_summary(recording: fandhe_device::mic_record::Recording) -> RecordSummary {
    RecordSummary {
        size_bytes: recording.bytes.len(),
        mime: recording.mime,
        duration_ms: recording.duration_ms,
    }
}

/// OS/`MicRecorder` からの生の結果 → [`finish_record`] へ渡す `Result<RecordOutcome,
/// String>` への変換（SSOT）。`DeviceError::Cancelled` は正常な利用者操作
/// （権限確認プロンプト中の停止）であり失敗ではないため、`to_message` を経由させず
/// `Ok(RecordOutcome::Cancelled)` へ写す（PR #959 レビュー指摘「Cancel shown as
/// recording failure」対応。`to_message` を通すと `FAILED_PREFIX` 付きの `error` に
/// 畳まれ、内部識別子 `"mic_record"` まで画面へ漏れていた）。
fn to_outcome(
    result: Result<fandhe_device::mic_record::Recording, DeviceError>,
) -> Result<RecordOutcome, String> {
    match result {
        Ok(recording) => Ok(RecordOutcome::Recorded(to_summary(recording))),
        Err(DeviceError::Cancelled(_)) => Ok(RecordOutcome::Cancelled),
        Err(other) => Err(to_message(other)),
    }
}

/// OS 側から届く結果コード（`code`）→ `DeviceError` の写像（macOS/iOS `.m`/Android
/// `MainActivity` 共通の SSOT。3 実装とも二重に写像しない、`apple_mic_record.rs`
/// から移動、Issue #957）。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 1 | 権限拒否 | `PermissionDenied("mic_record")` |
/// | 3 | 二重開始（busy） | `Internal("mic_record busy")` |
/// | 4 | 許可確認中に利用者が「停止」でキャンセル（macOS/iOS: `.m` 側「許可確認中の
///   キャンセル」節、Android: `MainActivity.requestMicRecordStop` の権限確認中
///   分岐参照） | `Cancelled("mic_record")` |
/// | 5 | ライフサイクル中断（iOS: バックグラウンド遷移により破棄、`.m` 側
///   「バックグラウンド」節参照、Issue #956。Android: `onPause`/`onDestroy`/
///   Activity 再生成で破棄、`MainActivity` 参照） | `Internal("mic_record
///   interrupted")` |
/// | それ以外 | 内部エラー（エンコード失敗等） | `Internal("mic_record")` |
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", target_os = "android")),
    allow(dead_code)
)]
pub(crate) fn error_from_code(code: i32) -> DeviceError {
    match code {
        1 => DeviceError::PermissionDenied("mic_record"),
        3 => DeviceError::Internal("mic_record busy"),
        4 => DeviceError::Cancelled("mic_record"),
        5 => DeviceError::Internal("mic_record interrupted"),
        _ => DeviceError::Internal("mic_record"),
    }
}

/// Windows の `windows::core::Error::code().0`（HRESULT の生値）→ 段階別の固定
/// ラベルへの写像（SSOT、`device/windows_mic_record.rs` の呼び出し元ごとに同じ
/// 判定を書き直さない）。`stage` は `"mic_record init"`/`"mic_record start"`/
/// `"mic_record stop"`/`"mic_record read"` のいずれか（呼び出し元が固定文字列で
/// 渡す）。`E_ACCESSDENIED`（`0x80070005`、設定 > プライバシー > マイクで拒否）
/// だけ `PermissionDenied("mic_record")` へ、それ以外は `Internal(stage)` へ畳む
/// （マイク未搭載・他アプリ使用中の HRESULT は Microsoft 文書で一意に確定できない
/// ため `Internal` に畳む既知の天井、実装計画 §2.4）。HRESULT の生値・`stage` の
/// 内部識別子は画面へそのまま出さない（`to_message` が `Internal`/`PermissionDenied`
/// の `Display` を通す、security.md A09）。呼び出し元（`windows_mic_record.rs`）は
/// Windows 限定モジュールのため、本関数自体には `cfg` を付けず全ターゲットで
/// コンパイル・テスト対象にする（`biometric_auth::outcome_from_verification` と
/// 同じ方針、ホスト上の `cargo test` で失敗系を固定する）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn error_from_hresult(code: i32, stage: &'static str) -> DeviceError {
    const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
    if code == E_ACCESSDENIED {
        DeviceError::PermissionDenied("mic_record")
    } else {
        DeviceError::Internal(stage)
    }
}

/// Windows の `MediaCapture` から得た生バイト列・経過時間 →
/// `fandhe_device::mic_record::Recording` への変換（mime は macOS/iOS 実装と同じ
/// `"audio/mp4"` 固定。SSOT の [`fandhe_device::mic_record::build_recording`] を
/// 経由し、0 バイト・`duration_ms == 0`・上限超過を弾く。呼び出し元
/// （`windows_mic_record.rs`）は Windows 限定モジュールのため、本関数自体には
/// `cfg` を付けず全ターゲットでコンパイル・テスト対象にする（[`error_from_hresult`]
/// と同じ方針）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn recording_from_capture(
    bytes: Vec<u8>,
    elapsed: std::time::Duration,
) -> Result<fandhe_device::mic_record::Recording, DeviceError> {
    fandhe_device::mic_record::build_recording(
        bytes,
        "audio/mp4".to_string(),
        elapsed.as_millis() as u64,
    )
}

/// macOS/iOS/Android/Windows 経路: OS 側コールバックとの橋渡し（pull 型、
/// `device/request_slot.rs::RequestSlot` の facade。モジュール doc 参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub(crate) mod bridge {
    use fandhe_core::screens::mic_record::RecordOutcome;

    use crate::device::request_slot::RequestSlot;

    /// 画面へ反映する完了結果（`super::to_outcome` 変換後の形）。
    pub(crate) type Outcome = Result<RecordOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 実際にネイティブ側の録音が開始した request_id（`0` は「まだ未開始」の
    /// 番兵、`RequestSlot::next_request_id` が `1` から採番するのと同じ流儀）。
    /// 権限確認待ちの間は経過時間を進めないための合図
    /// （[`mark_started`]/[`has_started`] 参照、Issue #954 レビュー指摘）。
    static STARTED_REQUEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// 録音開始直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// ネイティブ側が実際に録音を開始した瞬間に呼ぶ（macOS/iOS:
    /// `fandhe_rust_mic_record_started` 経由、Android:
    /// `nativeOnMicRecordStarted` 経由。任意スレッド）。
    pub(crate) fn mark_started(request_id: u64) {
        STARTED_REQUEST_ID.store(request_id, std::sync::atomic::Ordering::Relaxed);
    }

    /// `request_id` が実際に録音開始済みか（権限確認待ちの間は `false`）。
    pub(crate) fn has_started(request_id: u64) -> bool {
        STARTED_REQUEST_ID.load(std::sync::atomic::Ordering::Relaxed) == request_id
    }

    /// 現在進行中の要求 ID（ネイティブ側へ停止依頼を渡す際に使う。未開始なら
    /// `None`）。
    pub(crate) fn current_request_id() -> Option<u64> {
        SLOT.snapshot().map(|(request_id, _)| request_id)
    }

    /// OS からの結果を `super::to_outcome` で画面用へ変換してから置く（任意
    /// スレッド。実際には macOS/iOS はメインスレッド、Android は UI スレッド、
    /// Windows はワーカースレッドから届く契約、`apple_mic_record.rs`/`android.rs`/
    /// `windows_mic_record.rs` の doc 参照）。`request_id` 不一致・未登録・既に
    /// 結果ありは捨てて `false`。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<fandhe_device::mic_record::Recording, fandhe_device::DeviceError>,
    ) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }

    /// Android 専用: Activity 再生成直後に `android_main` から呼ぶ
    /// （`device::biometric_auth::bridge::reset` と同じ位置づけ）。[`SLOT`]・
    /// [`STARTED_REQUEST_ID`] はプロセス static のため Activity 再生成をまたいで
    /// 残るが、画面状態（`MicRecordState`）は再生成のたびに `Default`（`Idle`）で
    /// 作り直される。放置すると再生成前に開始した録音が破棄されずに残り、
    /// 新しい `Idle` 状態を [`super::drive_callback`] が見て早期 return するため
    /// 結果が画面へ一切反映されない。ここで両方を丸ごと捨て、以後届く旧
    /// `request_id` の [`deliver`]/[`mark_started`] を確実に無視させる。対になる
    /// 後始末として Java 側（`MainActivity::onCreate`）は保留中の
    /// `micRecordRequest` を `take()` する。呼び出し順に依存しない。
    ///
    /// 呼び出し元（`android::android_main`）は Android にのみ存在するため、
    /// それ以外のターゲット（macOS/iOS/Windows の host 上の `cargo test`）では
    /// 未使用になるが、`error_from_code` と同じ理由で `cfg` を割らず
    /// `allow(dead_code)` に留める。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset();
        STARTED_REQUEST_ID.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// OS へ提示を依頼する（macOS/iOS: `apple_mic_record::present`、Android:
/// `android::request_mic_record_start`）。戻り値は「提示依頼を受理できたか」
/// （`false` は呼び出し元が `Err(Internal("mic_record present"))` として畳む、
/// `device/biometric_auth.rs::present` と同型）。Windows は戻り値の形が異なる
/// （`Result<(), DeviceError>`）ため、この共通関数には含めず `drive_callback` から
/// `windows_mic_record::present` を直接呼ぶ（モジュール doc 参照）。
#[cfg(target_os = "android")]
fn present(request_id: u64, max_duration: std::time::Duration) -> bool {
    super::android::request_mic_record_start(request_id, max_duration.as_millis() as i32)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn present(request_id: u64, max_duration: std::time::Duration) -> bool {
    super::apple_mic_record::present(request_id, max_duration)
}

/// OS へ停止を依頼する（`present` と同じ振り分け。Windows は `drive_callback` から
/// `windows_mic_record::request_stop` を直接呼ぶ）。
#[cfg(target_os = "android")]
fn request_stop(request_id: u64) -> bool {
    super::android::request_mic_record_stop(request_id)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn request_stop(request_id: u64) -> bool {
    super::apple_mic_record::request_stop(request_id)
}

/// 直近の `mic_record_effect` 呼び出し時刻（経過時間の実測用、`device/tilt.rs::
/// LAST_TICK` と同じ役割）。`Recording` を離れたら `None` に戻す
/// （[`measure_dt_ms`] 参照）ことで、次に録音を始めた瞬間に古い時刻との差分を
/// 使ってしまう飛びを防ぐ。macOS/iOS/Android/Windows の `drive_callback` からのみ
/// 使う（それ以外は同期 `drive` で経過時間の計測自体が不要）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
static LAST_TICK: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

/// 1 回の呼び出しで進める経過ミリ秒の上限（バックグラウンド復帰等の飛び対策、
/// `device/tilt.rs::MAX_DT` と同じ考え方）。`device/speech_recognition.rs` も
/// [`measure_dt_ms`] 経由で共用する（Issue #1024、複製回避）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub(crate) const MAX_TICK_MS: u64 = 1_000;

/// 前回呼び出しからの経過ミリ秒を実測する（[`LAST_TICK`] 同様の役割を持つ
/// `Mutex<Option<Instant>>` を呼び出し元から受け取る引数化版、Issue #1024。
/// `device/speech_recognition.rs` が自前の `LAST_TICK` を渡して共用する）。
/// `active` が `false`（計測対象の状態でない）なら実測せず `last` をリセットして
/// `0` を返す（モジュール doc 参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub(crate) fn measure_dt_ms(last: &std::sync::Mutex<Option<Instant>>, active: bool) -> u64 {
    let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
    if !active {
        *last = None;
        return 0;
    }
    let now = Instant::now();
    let dt_ms = match *last {
        Some(prev) => now.duration_since(prev).as_millis() as u64,
        None => 0,
    };
    *last = Some(now);
    dt_ms.min(MAX_TICK_MS)
}

/// macOS/iOS/Android/Windows 経路の effect ドライバ本体（`device/biometric_auth.rs::
/// drive_callback` と同型）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
fn drive_callback(state: &MicRecordState, sender: &CompletionSender<MicRecordState>) {
    if let Some(_request) = take_start_request(state) {
        let _ = sender.send(Box::new(mark_recording));

        let request_id = bridge::begin();
        let max_duration = std::time::Duration::from_millis(MAX_RECORD_MS);
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
        if !present(request_id, max_duration) {
            // 提示依頼自体が同期的に失敗した（コールバックは二度と来ない）。
            bridge::deliver(request_id, Err(DeviceError::Internal("mic_record present")));
        }
        #[cfg(target_os = "windows")]
        if let Err(e) = super::windows_mic_record::present(request_id, max_duration) {
            // 提示依頼自体が同期的に失敗した（録音中で busy、ワーカースレッド自体が
            // 起動しない。`windows_mic_record::present` の doc 参照）。
            bridge::deliver(request_id, Err(e));
        }
        return;
    }

    if take_stop_request(state).is_some() {
        let _ = sender.send(Box::new(mark_stopping));
        // 「現在進行中のセッション」の request_id と照合する（macOS/iOS は `.m` 側、
        // Android は `MainActivity.requestMicRecordStop`、Windows は
        // `windows_mic_record::SESSION` が照合する契約、`.m` ファイル冒頭コメント/
        // `windows_mic_record.rs::request_stop` 参照）。停止依頼自体が届かなくても
        // （既に終了済み等）、自動停止 or 既存の結果配送が追随するため、ここでは
        // 戻り値を追加でハンドリングしない（`biometric_auth.rs` の防御的分岐と
        // 同じ位置づけ）。
        if let Some(request_id) = bridge::current_request_id() {
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
            let _ = request_stop(request_id);
            #[cfg(target_os = "windows")]
            let _ = super::windows_mic_record::request_stop(request_id);
        }
        return;
    }

    // 経過秒数は OS からの継続 push ではなく画面側の時刻で進める
    // （`fandhe_core::screens::mic_record` モジュール doc 参照）。ただし
    // ネイティブ側が実際に録音を開始する（`bridge::mark_started`）までは
    // 進めない: `mark_recording` 自体は「停止」ボタンを出すため権限確認開始
    // 時点で先に届くが、その待ち時間まで 30 秒の予算に含めると初回の許可
    // ダイアログが遅い環境で録音が短く打ち切られる（device-bridge-contracts.md
    // 「期限・キャンセル」節・Issue #954 レビュー指摘）。
    let started = bridge::current_request_id().is_some_and(bridge::has_started);
    let dt_ms = measure_dt_ms(&LAST_TICK, state.phase == RecordPhase::Recording && started);
    if dt_ms > 0 {
        let _ = sender.send(Box::new(move |s: &mut MicRecordState| tick(s, dt_ms)));
    }

    if state.phase != RecordPhase::Recording && state.phase != RecordPhase::Stopping {
        return;
    }

    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut MicRecordState, outcome| {
            // 反映できる状態か（`Idle` へ既に戻っていないか）を確かめてから
            // `finish_record` し、反映できたときだけ `true` を返す（`RequestSlot`
            // 側がそれを見て `clear` する）（`device/biometric_auth.rs` と同じ判断）。
            if s.phase != RecordPhase::Idle {
                finish_record(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// macOS/iOS/Android/Windows 以外向け: `MicRecorder` トレイト経由の同期呼び出し
/// （`device/biometric_auth.rs::drive` と同型）。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
)))]
fn drive(
    state: &MicRecordState,
    sender: &CompletionSender<MicRecordState>,
    recorder: &dyn MicRecorder,
) {
    if let Some(request) = take_start_request(state) {
        let _ = sender.send(Box::new(mark_recording));

        let max_duration = std::time::Duration::from_millis(request.max_duration_ms);
        let result = to_outcome(recorder.start(max_duration).and_then(|()| recorder.stop()));
        let generation = request.generation;
        let _ = sender.send(Box::new(move |s: &mut MicRecordState| {
            finish_record(s, generation, result);
        }));
        return;
    }

    // macOS/iOS/Android/Windows 以外は同期呼び出しのため `Recording`/`Stopping` の
    // 中間状態が観測可能なフレームを持たない（`start`/`stop` を上の分岐でまとめて
    // 呼び切る）。「停止」ボタンは `UnsupportedMicRecorder` 経路では表示されないため
    // （`current_screen` は `Idle`/`Recording` のみボタンを出す契約だが、この経路は
    // `Recording` へ遷移した次のフレームで即座に完了する）到達しない。防御的に
    // no-op のままにする。
    let _ = (state, sender, recorder);
}

/// `registry.rs` の `effect` 区画から `mic_record` 画面用に配線される effect
/// ドライバ（macOS/iOS/Android/Windows 版）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub fn mic_record_effect(state: &MicRecordState, sender: &CompletionSender<MicRecordState>) {
    drive_callback(state, sender);
}

/// `registry.rs` の `effect` 区画から `mic_record` 画面用に配線される effect
/// ドライバ（macOS/iOS/Android/Windows 以外版。常に `Unsupported`）。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
)))]
pub fn mic_record_effect(state: &MicRecordState, sender: &CompletionSender<MicRecordState>) {
    drive(
        state,
        sender,
        &fandhe_device::mic_record::UnsupportedMicRecorder,
    );
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照。
/// `device::biometric_auth::reset_pending_tracking` と同じ命名・位置づけ）。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    bridge::reset();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    fn sender_with_channel() -> (
        CompletionSender<MicRecordState>,
        std::sync::mpsc::Receiver<crate::app::Completion<MicRecordState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    #[test]
    fn error_from_code_maps_unsupported_to_fixed_message() {
        assert_eq!(
            to_message(DeviceError::Unsupported("mic_record")),
            UNAVAILABLE
        );
    }

    #[test]
    fn error_from_code_maps_table() {
        assert!(matches!(
            error_from_code(1),
            DeviceError::PermissionDenied("mic_record")
        ));
        assert!(matches!(
            error_from_code(3),
            DeviceError::Internal("mic_record busy")
        ));
        assert!(matches!(
            error_from_code(4),
            DeviceError::Cancelled("mic_record")
        ));
        assert!(matches!(
            error_from_code(5),
            DeviceError::Internal("mic_record interrupted")
        ));
        assert!(matches!(
            error_from_code(2),
            DeviceError::Internal("mic_record")
        ));
        assert!(matches!(
            error_from_code(999),
            DeviceError::Internal("mic_record")
        ));
    }

    #[test]
    fn to_summary_preserves_size_mime_and_duration() {
        let recording = fandhe_device::mic_record::Recording {
            bytes: vec![0u8; 42],
            mime: "audio/mp4".to_string(),
            duration_ms: 1_234,
        };
        let summary = to_summary(recording);
        assert_eq!(summary.size_bytes, 42);
        assert_eq!(summary.mime, "audio/mp4");
        assert_eq!(summary.duration_ms, 1_234);
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    #[test]
    fn drive_reports_unsupported_via_finish_record() {
        use fandhe_core::screens::mic_record::start_recording;

        let mut state = MicRecordState::default();
        start_recording(&mut state);
        let (sender, rx) = sender_with_channel();
        drive(
            &state,
            &sender,
            &fandhe_device::mic_record::UnsupportedMicRecorder,
        );

        // 1 通目は `mark_recording`、2 通目は `finish_record`。
        let mark = rx.try_recv().expect("mark_recording が送られている");
        mark(&mut state);
        assert_eq!(state.phase, RecordPhase::Recording);

        let finish = rx.try_recv().expect("finish_record が送られている");
        finish(&mut state);
        assert_eq!(state.phase, RecordPhase::Idle);
        assert!(state.error.contains(UNAVAILABLE));
    }

    #[test]
    fn error_from_hresult_maps_access_denied_to_permission_denied() {
        // `HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED)` = 0x80070005（符号付き i32 表現）。
        let code = 0x8007_0005_u32 as i32;
        assert!(matches!(
            error_from_hresult(code, "mic_record start"),
            DeviceError::PermissionDenied("mic_record")
        ));
    }

    #[test]
    fn error_from_hresult_maps_other_codes_to_internal_with_stage() {
        // `E_FAIL`。マイク未搭載・他アプリ使用中を含め既知の天井として `Internal`
        // に畳む（実装計画 §2.4）。
        let code = 0x8000_4005_u32 as i32;
        assert!(matches!(
            error_from_hresult(code, "mic_record stop"),
            DeviceError::Internal("mic_record stop")
        ));
    }

    #[test]
    fn recording_from_capture_rejects_empty_bytes() {
        let err =
            recording_from_capture(Vec::new(), std::time::Duration::from_secs(1)).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("mic_record bytes")));
    }

    #[test]
    fn recording_from_capture_rejects_zero_duration() {
        let err = recording_from_capture(vec![1, 2, 3], std::time::Duration::ZERO).unwrap_err();
        assert!(matches!(
            err,
            DeviceError::InvalidInput("mic_record duration")
        ));
    }

    #[test]
    fn recording_from_capture_accepts_valid_input() {
        let recording =
            recording_from_capture(vec![1, 2, 3], std::time::Duration::from_millis(1_500)).unwrap();
        assert_eq!(recording.bytes, vec![1, 2, 3]);
        assert_eq!(recording.mime, "audio/mp4");
        assert_eq!(recording.duration_ms, 1_500);
    }
}
