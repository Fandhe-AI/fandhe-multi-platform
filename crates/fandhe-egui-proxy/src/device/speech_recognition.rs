//! Phase 11 の拡充ループ（親 #1023、Issue #1024）: 型サンプル「音声認識」の effect
//! ドライバ。`fandhe_core::screens::speech_recognize`（端末依存ゼロ）の要求を観測し、
//! 結果を `CompletionSender` 経由で返す（`crate::screens::registry` の `effect`
//! 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。実装済みは macOS・Windows（#1024）・iOS（#1025）・
//! Android（#1026）（Web は親 #1023 の兄弟 issue が担当、実装計画 §0 参照）。
//!
//! # macOS/iOS/Windows/Android: pull 型 `bridge`（`mic_record.rs` と同型）
//!
//! macOS/iOS の権限確認（`SFSpeechRecognizer.requestAuthorization:`/マイク権限）・
//! Android の `RECORD_AUDIO` ランタイム許可・認識完了はいずれも非同期に届く。Windows は権限プロンプト自体は出ないが、
//! `CompileConstraintsAsync`/`RecognizeAsync`/`StopRecognitionAsync` が非同期 API で、
//! 同一フレーム内で `start`→`stop` を呼び切る同期 `drive` では録音時間 0 ms 相当の
//! 問題が生じるため、macOS/iOS/Android と同じ pull 型に乗せる（`mic_record.rs` モジュール doc
//! 「同期 drive 経路が使えない理由」と同じ判断）。3 OS とも `SpeechRecognizer`
//! トレイトを **通らず**コアの状態機械へ直接書き戻す（`fandhe_device::
//! speech_recognition` モジュール doc 参照）。Android の `present`/`request_stop` は
//! `android::request_speech_start`/`android::request_speech_stop` へ振り分ける
//! （`mic_record.rs` の Android 分岐と同型、Issue #1026）。
//!
//! # macOS/iOS/Windows/Android 以外: `SpeechRecognizer` トレイト経由（`UnsupportedSpeechRecognizer` 固定）
//!
//! Web/Linux は `fandhe_device::speech_recognition::
//! UnsupportedSpeechRecognizer` を経由し、固定文言（[`UNAVAILABLE`]）を表示する
//! （将来横展開する Issue がこの分岐を実装へ差し替える）。

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
use std::sync::Mutex;
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
use std::time::Instant;

use fandhe_core::screens::speech_recognize::{
    finish_listen, mark_listening, take_start_request, ListenOutcome, SpeechRecognizeState,
};
// ListenPhase は macOS/Windows/Android の `drive_callback`（本番コード）と、それ以外の
// `drive_reports_unsupported_via_finish_listen`（テストのみ）から使う（`mic_record.rs`
// の `RecordPhase` import と同じ理由）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android",
    test
))]
use fandhe_core::screens::speech_recognize::ListenPhase;
// mark_stopping/take_stop_request/tick/MAX_LISTEN_MS は macOS/Windows/Android の pull 型
// `drive_callback` でのみ使う（それ以外は同期 `drive` が start/stop をまとめて
// 呼び切るため中間状態を持たない、`mic_record.rs` と同じ判断）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
use fandhe_core::screens::speech_recognize::{
    mark_stopping, take_stop_request, tick, MAX_LISTEN_MS,
};
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
)))]
use fandhe_device::speech_recognition::SpeechRecognizer;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `SpeechRecognizer`/`bridge::deliver` 未対応環境（Unsupported）で画面へ出す固定案内
/// （`DeviceError` の内部構造は含めない、security.md A09。`device/mic_record.rs::
/// UNAVAILABLE` と同型）。
const UNAVAILABLE: &str = "この環境では音声認識を使えません";

/// `DeviceError` を画面へ渡す文言へ変換する。`Unsupported` は固定文言へ畳み、
/// それ以外は `Display` の文言をそのまま転送する（`device/mic_record.rs::
/// to_message` と同じ方針）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => UNAVAILABLE.to_string(),
        other => other.to_string(),
    }
}

/// OS/`SpeechRecognizer` からの生の結果 → [`finish_listen`] へ渡す
/// `Result<ListenOutcome, String>` への変換（SSOT）。`DeviceError::Cancelled` は
/// 正常な利用者操作（権限確認中の停止・OS 側のキャンセル）であり失敗ではないため、
/// `to_message` を経由させず `Ok(ListenOutcome::Cancelled)` へ写す（`mic_record.rs::
/// to_outcome` の Cancelled 分岐と同じ判断）。
fn to_outcome(
    result: Result<fandhe_device::speech_recognition::SpeechOutcome, DeviceError>,
) -> Result<ListenOutcome, String> {
    use fandhe_device::speech_recognition::SpeechOutcome;
    match result {
        Ok(SpeechOutcome::Recognized(text)) => Ok(ListenOutcome::Recognized(text)),
        Ok(SpeechOutcome::NoMatch) => Ok(ListenOutcome::NoMatch),
        Ok(SpeechOutcome::Cancelled) => Ok(ListenOutcome::Cancelled),
        Err(DeviceError::Cancelled(_)) => Ok(ListenOutcome::Cancelled),
        Err(other) => Err(to_message(other)),
    }
}

/// iOS/macOS `.m`/Android `MainActivity` 側から届く結果コード（`code`）→ `DeviceError` の
/// 写像（SSOT、実装計画 §3.3・§2.2 の表参照）。Android のコード 5 は `mic_record.rs::
/// error_from_code` のコード 5（ライフサイクル中断）と同じ意味。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 1 | 権限拒否（マイク / 音声認識） | `PermissionDenied("speech_recognition")` |
/// | 3 | 二重開始（busy） | `Internal("speech_recognition busy")` |
/// | 4 | 許可確認中に利用者が「停止」でキャンセル | `Ok(Cancelled)`（`to_outcome` が写す） |
/// | 5 | 中断（iOS: バックグラウンド遷移で破棄、Issue #1025。Android: `onPause`/`onDestroy`/非前景での開始） | `Internal("speech_recognition interrupted")` |
/// | 6 | 認識器を作れない（`SFSpeechRecognizer` が nil・言語未対応。Android: `isRecognitionAvailable == false`） | `Unsupported("speech_recognition")` |
/// | 7 | 認識器が一時的に使えない（`isAvailable == NO`） | `Disabled("speech_recognition")` |
/// | 8 | 無音（`kAFAssistantErrorDomain` 1110 等） | `Ok(NoMatch)`（`to_outcome` が写す） |
/// | それ以外 | 内部エラー（エンジン起動失敗等） | `Internal("speech_recognition")` |
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", target_os = "android")),
    allow(dead_code)
)]
pub(crate) fn error_from_code(
    code: i32,
) -> Result<fandhe_device::speech_recognition::SpeechOutcome, DeviceError> {
    use fandhe_device::speech_recognition::SpeechOutcome;
    match code {
        4 => Ok(SpeechOutcome::Cancelled),
        8 => Ok(SpeechOutcome::NoMatch),
        1 => Err(DeviceError::PermissionDenied("speech_recognition")),
        3 => Err(DeviceError::Internal("speech_recognition busy")),
        5 => Err(DeviceError::Internal("speech_recognition interrupted")),
        6 => Err(DeviceError::Unsupported("speech_recognition")),
        7 => Err(DeviceError::Disabled("speech_recognition")),
        _ => Err(DeviceError::Internal("speech_recognition")),
    }
}

/// Android の `RecognitionListener.onError(int)` が渡す生のエラーコード（`android.speech.
/// SpeechRecognizer` の `ERROR_*` 定数）→ `DeviceError` の写像（SSOT、実装計画 §2.3 の
/// 表参照）。Java 側では switch に書かず本関数へ委譲する（API 31 以降の定数を直接
/// 参照すると `InlinedApi` 警告が出るうえ、host での単体テストができなくなるため、
/// `biometric_auth::outcome_from_android_code` と同じ判断）。定数値は
/// developer.android.com/reference/android/speech/SpeechRecognizer で一次確認済み
/// （取得日 2026-10-01）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn outcome_from_android_error(
    error: i32,
) -> Result<fandhe_device::speech_recognition::SpeechOutcome, DeviceError> {
    use fandhe_device::speech_recognition::SpeechOutcome;
    match error {
        // ERROR_NETWORK_TIMEOUT = 1
        // ERROR_NETWORK = 2
        // ERROR_SERVER = 4
        // ERROR_TOO_MANY_REQUESTS = 10
        // ERROR_SERVER_DISCONNECTED = 11
        // ERROR_LANGUAGE_UNAVAILABLE = 13
        // いずれも一時的に使えない（オフライン等、`Disabled` に畳む `error_from_code`
        // コード 7 と同じ意味づけ）。
        1 | 2 | 4 | 10 | 11 | 13 => Err(DeviceError::Disabled("speech_recognition")),
        // ERROR_AUDIO = 3
        3 => Err(DeviceError::Internal("speech_recognition audio")),
        // ERROR_CLIENT = 5
        5 => Err(DeviceError::Internal("speech_recognition client")),
        // ERROR_SPEECH_TIMEOUT = 6
        // ERROR_NO_MATCH = 7
        6 | 7 => Ok(SpeechOutcome::NoMatch),
        // ERROR_RECOGNIZER_BUSY = 8
        8 => Err(DeviceError::Internal("speech_recognition busy")),
        // ERROR_INSUFFICIENT_PERMISSIONS = 9
        9 => Err(DeviceError::PermissionDenied("speech_recognition")),
        // ERROR_LANGUAGE_NOT_SUPPORTED = 12
        12 => Err(DeviceError::Unsupported("speech_recognition")),
        // それ以外（14: ERROR_CANNOT_CHECK_SUPPORT、15: ERROR_CANNOT_LISTEN_TO_DOWNLOAD_EVENTS、
        // 将来 OS が追加する未知の値）は内部エラーへ畳む既知の天井。
        _ => Err(DeviceError::Internal("speech_recognition")),
    }
}

/// Windows の `windows::core::Error::code().0`（HRESULT の生値）→ 段階別の固定
/// ラベルへの写像（SSOT、`device/windows_speech_recognition.rs` の呼び出し元ごとに
/// 同じ判定を書き直さない、`device/mic_record.rs::error_from_hresult` と同型）。
/// `E_ACCESSDENIED`（`0x80070005`、マイクのプライバシー設定で拒否）と、音声認識の
/// オンライン音声認識プライバシー設定が未同意のときに返る `0x80045509` のみ
/// `PermissionDenied("speech_recognition")` へ、それ以外は `Internal(stage)` へ畳む
/// （既知の天井、実装計画 §3.5）。呼び出し元（`windows_speech_recognition.rs`）は
/// Windows 限定モジュールのため、本関数自体には `cfg` を付けず全ターゲットで
/// コンパイル・テスト対象にする（`mic_record.rs::error_from_hresult` と同じ方針）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn error_from_hresult(code: i32, stage: &'static str) -> DeviceError {
    const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
    const E_PRIVACY_STATEMENT_DECLINED: i32 = 0x8004_5509_u32 as i32;
    if code == E_ACCESSDENIED || code == E_PRIVACY_STATEMENT_DECLINED {
        DeviceError::PermissionDenied("speech_recognition")
    } else {
        DeviceError::Internal(stage)
    }
}

/// Windows の `SpeechRecognitionResultStatus`（`windows::Media::SpeechRecognition::
/// SpeechRecognitionResultStatus`）→ [`fandhe_device::speech_recognition::
/// SpeechOutcome`]/`DeviceError` の写像（SSOT、実装計画 §3.3 の表参照）。呼び出し元
/// （`windows_speech_recognition.rs`）は Windows 限定モジュールのため、本関数自体には
/// `cfg` を付けず全ターゲットでコンパイル・テスト対象にする（[`error_from_hresult`]
/// と同じ方針）。`status` の生値は `windows` crate の列挙値をそのまま受け取る
/// （呼び出し元が `.0` を渡す想定、下記テスト参照）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn outcome_from_status(
    status: i32,
    text: String,
) -> Result<fandhe_device::speech_recognition::SpeechOutcome, DeviceError> {
    use fandhe_device::speech_recognition::{outcome_from_transcript, SpeechOutcome};
    // `windows::Media::SpeechRecognition::SpeechRecognitionResultStatus` の値
    // （Microsoft Learn 確認済み。Success = 0、TopicLanguageNotSupported = 1、
    // GrammarLanguageMismatch = 2、GrammarCompilationFailure = 3、
    // AudioQualityFailure = 4、UserCanceled = 5、Unknown = 6、
    // TimeoutExceeded = 7、PauseLimitExceeded = 8、NetworkFailure = 9、
    // MicrophoneUnavailable = 10。cursor 指摘: 旧実装は実際の値と異なる並びを
    // 誤って仮定しており、UserCanceled を NoMatch へ、Unknown を Cancelled へ
    // 誤写像していた）。
    match status {
        0 => outcome_from_transcript(text),
        // 無音・沈黙起因の打ち切りは「結果なし」として扱う（AudioQualityFailure/
        // TimeoutExceeded/PauseLimitExceeded）。
        4 | 7 | 8 => Ok(SpeechOutcome::NoMatch),
        5 => Ok(SpeechOutcome::Cancelled),
        1 | 2 => Err(DeviceError::Unsupported("speech_recognition")),
        3 => Err(DeviceError::Internal("speech_recognition grammar")),
        9 => Err(DeviceError::Internal("speech_recognition network")),
        10 => Err(DeviceError::Internal("speech_recognition microphone")),
        _ => Err(DeviceError::Internal("speech_recognition")),
    }
}

/// macOS/Windows 経路: OS 側コールバックとの橋渡し（pull 型、`device/
/// request_slot.rs::RequestSlot` の facade。モジュール doc 参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub(crate) mod bridge {
    use fandhe_core::screens::speech_recognize::ListenOutcome;

    use crate::device::request_slot::RequestSlot;

    /// 画面へ反映する完了結果（`super::to_outcome` 変換後の形）。
    pub(crate) type Outcome = Result<ListenOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 実際にネイティブ側の聞き取りが開始した request_id（`0` は「まだ未開始」の
    /// 番兵、`RequestSlot::next_request_id` が `1` から採番するのと同じ流儀）。
    /// 権限確認待ちの間は経過時間を進めないための合図（`mic_record::bridge::
    /// STARTED_REQUEST_ID` と同じ役割）。
    static STARTED_REQUEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// 聞き取り開始直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// ネイティブ側が実際に聞き取りを開始した瞬間に呼ぶ（macOS:
    /// `fandhe_rust_speech_started` 経由、Windows: `present` のワーカーから直接。
    /// 任意スレッド）。
    pub(crate) fn mark_started(request_id: u64) {
        STARTED_REQUEST_ID.store(request_id, std::sync::atomic::Ordering::Relaxed);
    }

    /// `request_id` が実際に聞き取り開始済みか（権限確認待ちの間は `false`）。
    pub(crate) fn has_started(request_id: u64) -> bool {
        STARTED_REQUEST_ID.load(std::sync::atomic::Ordering::Relaxed) == request_id
    }

    /// 現在進行中の要求 ID（ネイティブ側へ停止依頼を渡す際に使う。未開始なら
    /// `None`）。
    pub(crate) fn current_request_id() -> Option<u64> {
        SLOT.snapshot().map(|(request_id, _)| request_id)
    }

    /// OS からの結果を `super::to_outcome` で画面用へ変換してから置く（任意
    /// スレッド。macOS はメインスレッド、Windows はワーカースレッドから届く契約、
    /// `apple_speech_recognition.rs`/`windows_speech_recognition.rs` の doc
    /// 参照）。`request_id` 不一致・未登録・既に結果ありは捨てて `false`。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<
            fandhe_device::speech_recognition::SpeechOutcome,
            fandhe_device::DeviceError,
        >,
    ) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }

    /// Android 専用: Activity 再生成直後に `android_main` から呼ぶ
    /// （`device::mic_record::bridge::reset` と同じ位置づけ）。[`SLOT`]・
    /// [`STARTED_REQUEST_ID`] はプロセス static のため Activity 再生成をまたいで
    /// 残るが、画面状態（`SpeechRecognizeState`）は再生成のたびに `Default`（`Idle`）
    /// で作り直される。放置すると再生成前に開始した聞き取りが破棄されずに残り、
    /// 新しい `Idle` 状態を [`super::drive_callback`] が見て早期 return するため
    /// 結果が画面へ一切反映されない。ここで両方を丸ごと捨て、以後届く旧
    /// `request_id` の [`deliver`]/[`mark_started`] を確実に無視させる。対になる
    /// 後始末として Java 側（`MainActivity::onCreate`）は保留中の `speechRequest` を
    /// `take()` する。呼び出し順に依存しない（`mic_record::bridge::reset` doc 参照）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset();
        STARTED_REQUEST_ID.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// OS へ聞き取り開始を依頼する（iOS/macOS: `apple_speech_recognition::present`、Android:
/// `android::request_speech_start`。
/// Windows は戻り値の形が異なる（`Result<(), DeviceError>`）ため、この共通関数には
/// 含めず `drive_callback` から `windows_speech_recognition::present` を直接呼ぶ、
/// `mic_record.rs` モジュール doc と同じ判断）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn present(request_id: u64, max: std::time::Duration) -> bool {
    super::apple_speech_recognition::present(request_id, max)
}

#[cfg(target_os = "android")]
fn present(request_id: u64, max: std::time::Duration) -> bool {
    super::android::request_speech_start(request_id, max.as_millis() as i32)
}

/// OS へ停止を依頼する（`present` と同じ振り分け）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn request_stop(request_id: u64) -> bool {
    super::apple_speech_recognition::request_stop(request_id)
}

#[cfg(target_os = "android")]
fn request_stop(request_id: u64) -> bool {
    super::android::request_speech_stop(request_id)
}

/// 直近の `speech_recognize_effect` 呼び出し時刻（経過時間の実測用）。`mic_record.rs::
/// LAST_TICK`/`measure_dt_ms` を共用する（Issue #1024、複製回避、`mic_record.rs`
/// 「1 回の呼び出しで進める経過ミリ秒の上限」節参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
static LAST_TICK: Mutex<Option<Instant>> = Mutex::new(None);

/// macOS/iOS/Windows/Android 経路の effect ドライバ本体（`device/mic_record.rs::drive_callback`
/// と同型）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
fn drive_callback(state: &SpeechRecognizeState, sender: &CompletionSender<SpeechRecognizeState>) {
    if let Some(_request) = take_start_request(state) {
        let _ = sender.send(Box::new(mark_listening));

        let request_id = bridge::begin();
        let max = std::time::Duration::from_millis(MAX_LISTEN_MS);
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
        if !present(request_id, max) {
            // 提示依頼自体が同期的に失敗した（コールバックは二度と来ない）。
            bridge::deliver(
                request_id,
                Err(DeviceError::Internal("speech_recognition present")),
            );
        }
        #[cfg(target_os = "windows")]
        if let Err(e) = super::windows_speech_recognition::present(request_id, max) {
            // 提示依頼自体が同期的に失敗した（聞き取り中で busy、ワーカースレッド
            // 自体が起動しない。`windows_speech_recognition::present` の doc 参照）。
            bridge::deliver(request_id, Err(e));
        }
        return;
    }

    if take_stop_request(state).is_some() {
        let _ = sender.send(Box::new(mark_stopping));
        // 「現在進行中のセッション」の request_id と照合する（macOS は `.m` 側、
        // Windows は `windows_speech_recognition::SESSION` が照合する契約、
        // `mic_record.rs::drive_callback` の対応する分岐と同じ位置づけ）。停止依頼
        // 自体が届かなくても（既に終了済み等）、自動停止 or 既存の結果配送が
        // 追随するため、ここでは戻り値を追加でハンドリングしない。
        if let Some(request_id) = bridge::current_request_id() {
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
            let _ = request_stop(request_id);
            #[cfg(target_os = "windows")]
            let _ = super::windows_speech_recognition::request_stop(request_id);
        }
        return;
    }

    // 経過秒数は OS からの継続 push ではなく画面側の時刻で進める
    // （`fandhe_core::screens::speech_recognize` モジュール doc 参照）。ただし
    // ネイティブ側が実際に聞き取りを開始する（`bridge::mark_started`）までは
    // 進めない（`mic_record.rs::drive_callback` の対応する分岐と同じ判断）。
    let started = bridge::current_request_id().is_some_and(bridge::has_started);
    let dt_ms = super::mic_record::measure_dt_ms(
        &LAST_TICK,
        state.phase == ListenPhase::Listening && started,
    );
    if dt_ms > 0 {
        let _ = sender.send(Box::new(move |s: &mut SpeechRecognizeState| tick(s, dt_ms)));
    }

    if state.phase != ListenPhase::Listening && state.phase != ListenPhase::Stopping {
        return;
    }

    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut SpeechRecognizeState, outcome| {
            // 反映できる状態か（`Idle` へ既に戻っていないか）を確かめてから
            // `finish_listen` し、反映できたときだけ `true` を返す（`RequestSlot`
            // 側がそれを見て `clear` する）（`mic_record.rs` と同じ判断）。
            if s.phase != ListenPhase::Idle {
                finish_listen(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// macOS/Windows/Android 以外向け: `SpeechRecognizer` トレイト経由の同期呼び出し
/// （`device/mic_record.rs::drive` と同型）。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
)))]
fn drive(
    state: &SpeechRecognizeState,
    sender: &CompletionSender<SpeechRecognizeState>,
    recognizer: &dyn SpeechRecognizer,
) {
    if let Some(request) = take_start_request(state) {
        let _ = sender.send(Box::new(mark_listening));

        let max = std::time::Duration::from_millis(request.max_listen_ms);
        let result = to_outcome(recognizer.start(max).and_then(|()| recognizer.stop()));
        let generation = request.generation;
        let _ = sender.send(Box::new(move |s: &mut SpeechRecognizeState| {
            finish_listen(s, generation, result);
        }));
        return;
    }

    // macOS/Windows/Android 以外は同期呼び出しのため `Listening`/`Stopping` の中間
    // 状態が観測可能なフレームを持たない（`mic_record.rs::drive` と同じ理由）。
    let _ = (state, sender, recognizer);
}

/// `registry.rs` の `effect` 区画から `speech_recognize` 画面用に配線される effect
/// ドライバ（macOS/iOS/Windows/Android 版）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub fn speech_recognize_effect(
    state: &SpeechRecognizeState,
    sender: &CompletionSender<SpeechRecognizeState>,
) {
    drive_callback(state, sender);
}

/// `registry.rs` の `effect` 区画から `speech_recognize` 画面用に配線される effect
/// ドライバ（macOS/iOS/Windows/Android 以外版。常に `Unsupported`）。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
)))]
pub fn speech_recognize_effect(
    state: &SpeechRecognizeState,
    sender: &CompletionSender<SpeechRecognizeState>,
) {
    drive(
        state,
        sender,
        &fandhe_device::speech_recognition::UnsupportedSpeechRecognizer,
    );
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照。
/// `device::mic_record::reset_pending_tracking` と同じ命名・位置づけ）。
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
        CompletionSender<SpeechRecognizeState>,
        std::sync::mpsc::Receiver<crate::app::Completion<SpeechRecognizeState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    #[test]
    fn to_message_maps_unsupported_to_fixed_message() {
        assert_eq!(
            to_message(DeviceError::Unsupported("speech_recognition")),
            UNAVAILABLE
        );
    }

    #[test]
    fn error_from_code_maps_table() {
        use fandhe_device::speech_recognition::SpeechOutcome;
        assert_eq!(error_from_code(4).unwrap(), SpeechOutcome::Cancelled);
        assert_eq!(error_from_code(8).unwrap(), SpeechOutcome::NoMatch);
        assert!(matches!(
            error_from_code(1).unwrap_err(),
            DeviceError::PermissionDenied("speech_recognition")
        ));
        assert!(matches!(
            error_from_code(3).unwrap_err(),
            DeviceError::Internal("speech_recognition busy")
        ));
        assert!(matches!(
            error_from_code(5).unwrap_err(),
            DeviceError::Internal("speech_recognition interrupted")
        ));
        assert!(matches!(
            error_from_code(6).unwrap_err(),
            DeviceError::Unsupported("speech_recognition")
        ));
        assert!(matches!(
            error_from_code(7).unwrap_err(),
            DeviceError::Disabled("speech_recognition")
        ));
        assert!(matches!(
            error_from_code(2).unwrap_err(),
            DeviceError::Internal("speech_recognition")
        ));
        assert!(matches!(
            error_from_code(999).unwrap_err(),
            DeviceError::Internal("speech_recognition")
        ));
    }

    #[test]
    fn outcome_from_android_error_maps_table() {
        use fandhe_device::speech_recognition::SpeechOutcome;
        // ERROR_SPEECH_TIMEOUT / ERROR_NO_MATCH → 聞き取れず。
        assert_eq!(
            outcome_from_android_error(6).unwrap(),
            SpeechOutcome::NoMatch
        );
        assert_eq!(
            outcome_from_android_error(7).unwrap(),
            SpeechOutcome::NoMatch
        );
        // ERROR_INSUFFICIENT_PERMISSIONS → 権限拒否。
        assert!(matches!(
            outcome_from_android_error(9).unwrap_err(),
            DeviceError::PermissionDenied("speech_recognition")
        ));
        // ネットワーク・サーバー系は一時的に使えない扱い。
        for code in [1, 2, 4, 10, 11, 13] {
            assert!(
                matches!(
                    outcome_from_android_error(code).unwrap_err(),
                    DeviceError::Disabled("speech_recognition")
                ),
                "code={code} は Disabled であるべき"
            );
        }
        // ERROR_LANGUAGE_NOT_SUPPORTED → 非対応。
        assert!(matches!(
            outcome_from_android_error(12).unwrap_err(),
            DeviceError::Unsupported("speech_recognition")
        ));
        // ERROR_RECOGNIZER_BUSY → 二重開始。
        assert!(matches!(
            outcome_from_android_error(8).unwrap_err(),
            DeviceError::Internal("speech_recognition busy")
        ));
        // ERROR_AUDIO / ERROR_CLIENT → 内部エラー（段階別）。
        assert!(matches!(
            outcome_from_android_error(3).unwrap_err(),
            DeviceError::Internal("speech_recognition audio")
        ));
        assert!(matches!(
            outcome_from_android_error(5).unwrap_err(),
            DeviceError::Internal("speech_recognition client")
        ));
        // 未知の値（14・15・999）は内部エラーへ畳む既知の天井。
        for code in [14, 15, 999] {
            assert!(
                matches!(
                    outcome_from_android_error(code).unwrap_err(),
                    DeviceError::Internal("speech_recognition")
                ),
                "code={code} は Internal であるべき"
            );
        }
    }

    #[test]
    fn error_from_hresult_maps_access_denied_to_permission_denied() {
        let code = 0x8007_0005_u32 as i32;
        assert!(matches!(
            error_from_hresult(code, "speech_recognition start"),
            DeviceError::PermissionDenied("speech_recognition")
        ));
    }

    #[test]
    fn error_from_hresult_maps_privacy_declined_to_permission_denied() {
        let code = 0x8004_5509_u32 as i32;
        assert!(matches!(
            error_from_hresult(code, "speech_recognition init"),
            DeviceError::PermissionDenied("speech_recognition")
        ));
    }

    #[test]
    fn error_from_hresult_maps_other_codes_to_internal_with_stage() {
        let code = 0x8000_4005_u32 as i32;
        assert!(matches!(
            error_from_hresult(code, "speech_recognition stop"),
            DeviceError::Internal("speech_recognition stop")
        ));
    }

    #[test]
    fn outcome_from_status_success_delegates_to_transcript() {
        use fandhe_device::speech_recognition::SpeechOutcome;
        assert_eq!(
            outcome_from_status(0, "こんにちは".to_string()).unwrap(),
            SpeechOutcome::Recognized("こんにちは".to_string())
        );
        assert_eq!(
            outcome_from_status(0, "".to_string()).unwrap(),
            SpeechOutcome::NoMatch
        );
    }

    #[test]
    fn outcome_from_status_maps_timeout_and_quality_to_no_match() {
        use fandhe_device::speech_recognition::SpeechOutcome;
        // TimeoutExceeded = 7, PauseLimitExceeded = 8, AudioQualityFailure = 4
        // （実際の WinRT 値。cursor 指摘で修正済み）。
        assert_eq!(
            outcome_from_status(7, String::new()).unwrap(),
            SpeechOutcome::NoMatch
        );
        assert_eq!(
            outcome_from_status(8, String::new()).unwrap(),
            SpeechOutcome::NoMatch
        );
        assert_eq!(
            outcome_from_status(4, String::new()).unwrap(),
            SpeechOutcome::NoMatch
        );
    }

    #[test]
    fn outcome_from_status_maps_user_canceled_to_cancelled() {
        use fandhe_device::speech_recognition::SpeechOutcome;
        // UserCanceled = 5（実際の WinRT 値。cursor 指摘で修正済み）。
        assert_eq!(
            outcome_from_status(5, String::new()).unwrap(),
            SpeechOutcome::Cancelled
        );
    }

    #[test]
    fn outcome_from_status_maps_language_mismatch_to_unsupported() {
        assert!(matches!(
            outcome_from_status(1, String::new()).unwrap_err(),
            DeviceError::Unsupported("speech_recognition")
        ));
        assert!(matches!(
            outcome_from_status(2, String::new()).unwrap_err(),
            DeviceError::Unsupported("speech_recognition")
        ));
    }

    #[test]
    fn outcome_from_status_maps_microphone_and_network_to_internal() {
        // GrammarCompilationFailure = 3, NetworkFailure = 9,
        // MicrophoneUnavailable = 10（実際の WinRT 値。cursor 指摘で修正済み）。
        assert!(matches!(
            outcome_from_status(3, String::new()).unwrap_err(),
            DeviceError::Internal("speech_recognition grammar")
        ));
        assert!(matches!(
            outcome_from_status(9, String::new()).unwrap_err(),
            DeviceError::Internal("speech_recognition network")
        ));
        assert!(matches!(
            outcome_from_status(10, String::new()).unwrap_err(),
            DeviceError::Internal("speech_recognition microphone")
        ));
    }

    #[test]
    fn outcome_from_status_maps_unknown_variant_to_internal() {
        // Unknown = 6（実際の WinRT 値。旧実装は誤って Cancelled へ写していた、
        // cursor 指摘で修正済み）。
        assert!(matches!(
            outcome_from_status(6, String::new()).unwrap_err(),
            DeviceError::Internal("speech_recognition")
        ));
    }

    #[test]
    fn outcome_from_status_maps_unknown_to_internal() {
        assert!(matches!(
            outcome_from_status(999, String::new()).unwrap_err(),
            DeviceError::Internal("speech_recognition")
        ));
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    #[test]
    fn drive_reports_unsupported_via_finish_listen() {
        use fandhe_core::screens::speech_recognize::start_listening;

        let mut state = SpeechRecognizeState::default();
        start_listening(&mut state);
        let (sender, rx) = sender_with_channel();
        drive(
            &state,
            &sender,
            &fandhe_device::speech_recognition::UnsupportedSpeechRecognizer,
        );

        // 1 通目は `mark_listening`、2 通目は `finish_listen`。
        let mark = rx.try_recv().expect("mark_listening が送られている");
        mark(&mut state);
        assert_eq!(state.phase, ListenPhase::Listening);

        let finish = rx.try_recv().expect("finish_listen が送られている");
        finish(&mut state);
        assert_eq!(state.phase, ListenPhase::Idle);
        assert!(state.error.contains(UNAVAILABLE));
    }
}
