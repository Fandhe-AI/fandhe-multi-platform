//! Phase 11 の拡充ループ（親 #1023、Issue #1024）: 型サンプル「音声認識」の effect
//! ドライバ。`fandhe_core::screens::speech_recognize`（端末依存ゼロ）の要求を観測し、
//! 結果を `CompletionSender` 経由で返す（`fandhe-gallery` の `screens/registry.rs` の `effect`
//! 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! 全ターゲットでコンパイルされる（OS ごとの実装は各 OS の cfg で切り替える）。実装済みは macOS・Windows（#1024）・iOS（#1025）・
//! Android（#1026）（Web は親 #1023 の兄弟 issue が担当、実装計画 §0 参照）。
//!
//! # macOS/iOS/Windows/Android: pull 型 `bridge`（`fandhe-cap-mic-record` の `driver.rs` と同型）
//!
//! macOS/iOS の権限確認（`SFSpeechRecognizer.requestAuthorization:`/マイク権限）・
//! Android の `RECORD_AUDIO` ランタイム許可・認識完了はいずれも非同期に届く。Windows は権限プロンプト自体は出ないが、
//! `CompileConstraintsAsync`/`RecognizeAsync`/`StopRecognitionAsync` が非同期 API で、
//! 同一フレーム内で `start`→`stop` を呼び切る同期 `drive` では録音時間 0 ms 相当の
//! 問題が生じるため、macOS/iOS/Android と同じ pull 型に乗せる（`fandhe-cap-mic-record` の `driver.rs` モジュール doc
//! 「同期 drive 経路が使えない理由」と同じ判断）。3 OS とも `SpeechRecognizer`
//! トレイトを **通らず**コアの状態機械へ直接書き戻す（`crate::contract` の doc 参照）。Android の `present`/`request_stop` は
//! `android::request_speech_start`/`android::request_speech_stop` へ振り分ける
//! （`fandhe-cap-mic-record` の `driver.rs` の Android 分岐と同型、Issue #1026）。
//!
//! # macOS/iOS/Windows/Android 以外: `SpeechRecognizer` トレイト経由（`UnsupportedSpeechRecognizer` 固定）
//!
//! Web/Linux は `crate::contract::
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
    ListenOutcome, SpeechRecognizeState, finish_listen, mark_listening, take_start_request,
};
// ListenPhase は macOS/Windows/Android の `drive_callback`（本番コード）と、それ以外の
// `drive_reports_unsupported_via_finish_listen`（テストのみ）から使う（`fandhe-cap-mic-record` の `driver.rs`
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
// 呼び切るため中間状態を持たない、`fandhe-cap-mic-record` の `driver.rs` と同じ判断）。
use crate::contract::{SpeechRecognizer, UnsupportedSpeechRecognizer};
use crate::error::SpeechRecognitionError;
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
use fandhe_core::screens::speech_recognize::{
    MAX_LISTEN_MS, mark_stopping, take_stop_request, tick,
};

use fandhe_runtime::CompletionSender;

/// `SpeechRecognizer`/`bridge::deliver` 未対応環境（Unsupported）で画面へ出す固定案内
/// （`SpeechRecognitionError` の内部構造は含めない、security.md A09。`fandhe-cap-mic-record` の driver の `
/// UNAVAILABLE` と同型）。
const UNAVAILABLE: &str = "この環境では音声認識を使えません";

/// `SpeechRecognitionError` を画面へ渡す文言へ変換する。`Unsupported` は固定文言へ畳み、
/// それ以外は `Display` の文言をそのまま転送する（`fandhe-cap-mic-record` の driver の `
/// to_message` と同じ方針）。
fn to_message(e: SpeechRecognitionError) -> String {
    match e {
        SpeechRecognitionError::Unsupported => UNAVAILABLE.to_string(),
        other => other.to_string(),
    }
}

/// OS/`SpeechRecognizer` からの生の結果 → [`finish_listen`] へ渡す
/// `Result<ListenOutcome, String>` への変換（SSOT）。`SpeechRecognitionError::Cancelled` は
/// 正常な利用者操作（権限確認中の停止・OS 側のキャンセル）であり失敗ではないため、
/// `to_message` を経由させず `Ok(ListenOutcome::Cancelled)` へ写す（`mic_record.rs::
/// to_outcome` の Cancelled 分岐と同じ判断）。
fn to_outcome(
    result: Result<crate::contract::SpeechOutcome, SpeechRecognitionError>,
) -> Result<ListenOutcome, String> {
    use crate::contract::SpeechOutcome;
    match result {
        Ok(SpeechOutcome::Recognized(text)) => Ok(ListenOutcome::Recognized(text)),
        Ok(SpeechOutcome::NoMatch) => Ok(ListenOutcome::NoMatch),
        Ok(SpeechOutcome::Cancelled) => Ok(ListenOutcome::Cancelled),
        Err(SpeechRecognitionError::Cancelled(_)) => Ok(ListenOutcome::Cancelled),
        Err(other) => Err(to_message(other)),
    }
}

/// iOS/macOS `.m`/Android `SpeechRecognitionHelper` 側から届く結果コード（`code`）→ `SpeechRecognitionError` の
/// 写像（SSOT、実装計画 §3.3・§2.2 の表参照）。Android のコード 5 は `mic_record.rs::
/// error_from_code` のコード 5（ライフサイクル中断）と同じ意味。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 1 | 権限拒否（マイク / 音声認識） | `PermissionDenied("speech_recognition")` |
/// | 3 | 二重開始（busy） | `Internal("speech_recognition busy")` |
/// | 4 | 許可確認中に利用者が「停止」でキャンセル | `Ok(Cancelled)`（`to_outcome` が写す） |
/// | 5 | 中断（iOS: バックグラウンド遷移で破棄、Issue #1025。Android: 構成変更でない `onStop`・`onDestroy`・非前景での開始） | `Internal("speech_recognition interrupted")` |
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
) -> Result<crate::contract::SpeechOutcome, SpeechRecognitionError> {
    use crate::contract::SpeechOutcome;
    match code {
        4 => Ok(SpeechOutcome::Cancelled),
        8 => Ok(SpeechOutcome::NoMatch),
        1 => Err(SpeechRecognitionError::PermissionDenied),
        3 => Err(SpeechRecognitionError::Internal("speech_recognition busy")),
        5 => Err(SpeechRecognitionError::Internal(
            "speech_recognition interrupted",
        )),
        6 => Err(SpeechRecognitionError::Unsupported),
        7 => Err(SpeechRecognitionError::Disabled("speech_recognition")),
        _ => Err(SpeechRecognitionError::Internal("speech_recognition")),
    }
}

/// Android の `RecognitionListener.onError(int)` が渡す生のエラーコード（`android.speech.
/// SpeechRecognizer` の `ERROR_*` 定数）→ `SpeechRecognitionError` の写像（SSOT、実装計画 §2.3 の
/// 表参照）。Java 側では switch に書かず本関数へ委譲する（API 31 以降の定数を直接
/// 参照すると `InlinedApi` 警告が出るうえ、host での単体テストができなくなるため、
/// `biometric_auth::outcome_from_android_code` と同じ判断）。定数値は
/// developer.android.com/reference/android/speech/SpeechRecognizer で一次確認済み
/// （取得日 2026-10-01）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn outcome_from_android_error(
    error: i32,
) -> Result<crate::contract::SpeechOutcome, SpeechRecognitionError> {
    use crate::contract::SpeechOutcome;
    match error {
        // ERROR_NETWORK_TIMEOUT = 1
        // ERROR_NETWORK = 2
        // ERROR_SERVER = 4
        // ERROR_TOO_MANY_REQUESTS = 10
        // ERROR_SERVER_DISCONNECTED = 11
        // ERROR_LANGUAGE_UNAVAILABLE = 13
        // いずれも一時的に使えない（オフライン等、`Disabled` に畳む `error_from_code`
        // コード 7 と同じ意味づけ）。
        1 | 2 | 4 | 10 | 11 | 13 => Err(SpeechRecognitionError::Disabled("speech_recognition")),
        // ERROR_AUDIO = 3
        3 => Err(SpeechRecognitionError::Internal("speech_recognition audio")),
        // ERROR_CLIENT = 5
        5 => Err(SpeechRecognitionError::Internal(
            "speech_recognition client",
        )),
        // ERROR_SPEECH_TIMEOUT = 6
        // ERROR_NO_MATCH = 7
        6 | 7 => Ok(SpeechOutcome::NoMatch),
        // ERROR_RECOGNIZER_BUSY = 8
        8 => Err(SpeechRecognitionError::Internal("speech_recognition busy")),
        // ERROR_INSUFFICIENT_PERMISSIONS = 9
        9 => Err(SpeechRecognitionError::PermissionDenied),
        // ERROR_LANGUAGE_NOT_SUPPORTED = 12
        12 => Err(SpeechRecognitionError::Unsupported),
        // それ以外（14: ERROR_CANNOT_CHECK_SUPPORT、15: ERROR_CANNOT_LISTEN_TO_DOWNLOAD_EVENTS、
        // 将来 OS が追加する未知の値）は内部エラーへ畳む既知の天井。
        _ => Err(SpeechRecognitionError::Internal("speech_recognition")),
    }
}

/// Windows の `windows::core::Error::code().0`（HRESULT の生値）→ 段階別の固定
/// ラベルへの写像（SSOT、`windows.rs` の呼び出し元ごとに
/// 同じ判定を書き直さない、`fandhe-cap-mic-record` の driver の `error_from_hresult` と同型）。
/// `E_ACCESSDENIED`（`0x80070005`、マイクのプライバシー設定で拒否）と、音声認識の
/// オンライン音声認識プライバシー設定が未同意のときに返る `0x80045509` のみ
/// `PermissionDenied("speech_recognition")` へ、それ以外は `Internal(stage)` へ畳む
/// （既知の天井、実装計画 §3.5）。呼び出し元（`windows.rs`）は
/// Windows 限定モジュールのため、本関数自体には `cfg` を付けず全ターゲットで
/// コンパイル・テスト対象にする（`mic_record.rs::error_from_hresult` と同じ方針）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn error_from_hresult(code: i32, stage: &'static str) -> SpeechRecognitionError {
    const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
    const E_PRIVACY_STATEMENT_DECLINED: i32 = 0x8004_5509_u32 as i32;
    if code == E_ACCESSDENIED || code == E_PRIVACY_STATEMENT_DECLINED {
        SpeechRecognitionError::PermissionDenied
    } else {
        SpeechRecognitionError::Internal(stage)
    }
}

/// Windows の `SpeechRecognitionResultStatus`（`windows::Media::SpeechRecognition::
/// SpeechRecognitionResultStatus`）→ [`crate::contract::
/// SpeechOutcome`]/`SpeechRecognitionError` の写像（SSOT、実装計画 §3.3 の表参照）。呼び出し元
/// （`windows.rs`）は Windows 限定モジュールのため、本関数自体には
/// `cfg` を付けず全ターゲットでコンパイル・テスト対象にする（[`error_from_hresult`]
/// と同じ方針）。`status` の生値は `windows` crate の列挙値をそのまま受け取る
/// （呼び出し元が `.0` を渡す想定、下記テスト参照）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn outcome_from_status(
    status: i32,
    text: String,
) -> Result<crate::contract::SpeechOutcome, SpeechRecognitionError> {
    use crate::contract::{SpeechOutcome, outcome_from_transcript};
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
        1 | 2 => Err(SpeechRecognitionError::Unsupported),
        3 => Err(SpeechRecognitionError::Internal(
            "speech_recognition grammar",
        )),
        9 => Err(SpeechRecognitionError::Internal(
            "speech_recognition network",
        )),
        10 => Err(SpeechRecognitionError::Internal(
            "speech_recognition microphone",
        )),
        _ => Err(SpeechRecognitionError::Internal("speech_recognition")),
    }
}

/// macOS/Windows 経路: OS 側コールバックとの橋渡し（pull 型、`fandhe-runtime/src/request_slot.rs::RequestSlot` の facade。モジュール doc 参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub(crate) mod bridge {
    use fandhe_runtime::request_slot::{RequestSlot, SlotOwner};

    /// OS からの生の結果。画面へ渡すときは `super::to_outcome` で変換し、公開入口の poll はそのまま返す
    /// （どちらも同じ slot を共有するため、変換前の形で置く）。
    pub(crate) type Outcome =
        Result<crate::contract::SpeechOutcome, crate::error::SpeechRecognitionError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 公開入口 [`super::begin_speech_recognition`] が追う自分の分の要求（画面側が置き換え・横取りしない）。
    pub(crate) static PUBLIC_OWNER: SlotOwner = SlotOwner::new();
    /// 画面 effect が追う自分の分の要求（公開入口と slot を共有するため区別する）。
    pub(crate) static SCREEN_OWNER: SlotOwner = SlotOwner::new();

    /// 実際にネイティブ側の聞き取りが開始した request_id（`0` は「まだ未開始」の
    /// 番兵、`RequestSlot::next_request_id` が `1` から採番するのと同じ流儀）。
    /// 権限確認待ちの間は経過時間を進めないための合図（`mic_record::bridge::
    /// STARTED_REQUEST_ID` と同じ役割）。
    static STARTED_REQUEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// 持ち主を付けずに登録する（テスト用。本番は [`SCREEN_OWNER`]・[`PUBLIC_OWNER`] 経由で登録する）。
    #[cfg(test)]
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

    /// 現在進行中の要求 ID（テスト用。本番は持ち主ごとの `current()` を使う）。
    #[cfg(test)]
    pub(crate) fn current_request_id() -> Option<u64> {
        SLOT.snapshot().map(|(request_id, _)| request_id)
    }

    /// OS からの結果を `super::to_outcome` で画面用へ変換してから置く（任意
    /// スレッド。macOS はメインスレッド、Windows はワーカースレッドから届く契約、
    /// `apple.rs`/`windows.rs` の doc
    /// 参照）。`request_id` 不一致・未登録・既に結果ありは捨てて `false`。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<crate::contract::SpeechOutcome, crate::error::SpeechRecognitionError>,
    ) -> bool {
        SLOT.deliver(request_id, result)
    }

    /// Android 専用: Activity 再生成直後に `android_main` から呼ぶ
    /// （`fandhe-cap-mic-record` の `bridge::reset` と同じ位置づけ）。[`SLOT`]・
    /// [`STARTED_REQUEST_ID`] はプロセス static のため Activity 再生成をまたいで
    /// 残るが、画面状態（`SpeechRecognizeState`）は再生成のたびに `Default`（`Idle`）
    /// で作り直される。放置すると再生成前に開始した聞き取りが破棄されずに残り、
    /// 新しい `Idle` 状態を [`super::drive_callback`] が見て早期 return するため
    /// 結果が画面へ一切反映されない。ここで両方を丸ごと捨て、以後届く旧
    /// `request_id` の [`deliver`]/[`mark_started`] を確実に無視させる。Java 側
    /// （`SpeechRecognitionHelper`）は再生成で保留中の要求を捨てず、権限ダイアログの結果を新しい
    /// helper が引き継いで同じ id で 1 度だけ届ける（終了による破棄は中断（5）を 1 度だけ届ける）。
    /// 画面状態が `Idle` へ戻る構成（`fandhe-gallery`）向けで、状態を保つ外部アプリは呼ばなくてよい。
    /// 呼び出し順に依存しない（`fandhe-cap-mic-record` の `bridge::reset` doc 参照）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset();
        STARTED_REQUEST_ID.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// OS へ聞き取り開始を依頼する（iOS/macOS: `apple::present`、Android:
/// `android::request_speech_start`。
/// Windows は戻り値の形が異なる（`Result<(), SpeechRecognitionError>`）ため、この共通関数には
/// 含めず `drive_callback` から `windows::present` を直接呼ぶ、
/// `fandhe-cap-mic-record` の `driver.rs` モジュール doc と同じ判断）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn present(request_id: u64, max: std::time::Duration) -> bool {
    crate::apple::present(request_id, max)
}

#[cfg(target_os = "android")]
fn present(request_id: u64, max: std::time::Duration) -> bool {
    crate::android::request_speech_start(request_id, max.as_millis() as i32)
}

/// OS へ停止を依頼する（`present` と同じ振り分け）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn request_stop(request_id: u64) -> bool {
    crate::apple::request_stop(request_id)
}

#[cfg(target_os = "android")]
fn request_stop(request_id: u64) -> bool {
    crate::android::request_speech_stop(request_id)
}

/// 直近の `speech_recognize_effect` 呼び出し時刻（経過時間の実測用）。`mic_record.rs::
/// LAST_TICK`/`measure_dt_ms` を共用する（Issue #1024、複製回避、`fandhe-cap-mic-record` の `driver.rs`
/// 「1 回の呼び出しで進める経過ミリ秒の上限」節参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
static LAST_TICK: Mutex<Option<Instant>> = Mutex::new(None);

/// macOS/iOS/Windows/Android 経路の effect ドライバ本体（`fandhe-cap-mic-record` の driver の `drive_callback`
/// と同型）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
fn drive_callback(state: &SpeechRecognizeState, sender: &CompletionSender<SpeechRecognizeState>) {
    if let Some(request) = take_start_request(state) {
        let _ = sender.send(Box::new(mark_listening));

        // 公開入口の進行中・未取得の要求は置き換えず、画面側を処理中の失敗で終える。
        let Some(request_id) = bridge::SCREEN_OWNER.begin_keeping_foreign(&bridge::SLOT) else {
            let generation = request.generation;
            let message = to_message(SpeechRecognitionError::Internal("speech_recognition busy"));
            let _ = sender.send(Box::new(move |s: &mut SpeechRecognizeState| {
                finish_listen(s, generation, Err(message));
            }));
            return;
        };
        let max = std::time::Duration::from_millis(MAX_LISTEN_MS);
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
        if !present(request_id, max) {
            // 提示依頼自体が同期的に失敗した（コールバックは二度と来ない）。
            bridge::deliver(
                request_id,
                Err(SpeechRecognitionError::Internal(
                    "speech_recognition present",
                )),
            );
        }
        #[cfg(target_os = "windows")]
        if let Err(e) = crate::windows::present(request_id, max) {
            // 提示依頼自体が同期的に失敗した（聞き取り中で busy、ワーカースレッド
            // 自体が起動しない。`windows::present` の doc 参照）。
            bridge::deliver(request_id, Err(e));
        }
        return;
    }

    if take_stop_request(state).is_some() {
        let _ = sender.send(Box::new(mark_stopping));
        // 「現在進行中のセッション」の request_id と照合する（macOS は `.m` 側、
        // Windows は `windows::SESSION` が照合する契約、
        // `mic_record.rs::drive_callback` の対応する分岐と同じ位置づけ）。停止依頼
        // 自体が届かなくても（既に終了済み等）、自動停止 or 既存の結果配送が
        // 追随するため、ここでは戻り値を追加でハンドリングしない。
        let request_id = bridge::SCREEN_OWNER.current();
        if request_id != 0 {
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
            let _ = request_stop(request_id);
            #[cfg(target_os = "windows")]
            let _ = crate::windows::request_stop(request_id);
        }
        return;
    }

    // 経過秒数は OS からの継続 push ではなく画面側の時刻で進める
    // （`fandhe_core::screens::speech_recognize` モジュール doc 参照）。ただし
    // ネイティブ側が実際に聞き取りを開始する（`bridge::mark_started`）までは
    // 進めない（`mic_record.rs::drive_callback` の対応する分岐と同じ判断）。
    let own = bridge::SCREEN_OWNER.current();
    let started = own != 0 && bridge::has_started(own);
    let dt_ms = fandhe_runtime::tick::measure_dt_ms(
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
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &bridge::SCREEN_OWNER,
        sender,
        |_request_id| {},
        move |s: &mut SpeechRecognizeState, outcome| {
            let outcome = to_outcome(outcome);
            // 反映できる状態か（`Idle` へ既に戻っていないか）を確かめてから
            // `finish_listen` し、反映できたときだけ `true` を返す（`RequestSlot`
            // 側がそれを見て `clear` する）（`fandhe-cap-mic-record` の `driver.rs` と同じ判断）。
            if s.phase != ListenPhase::Idle {
                finish_listen(s, generation, outcome);
                true
            } else {
                false
            }
        },
        // 自分の要求が置き換わった・reset で失われた。待ち続けず失敗で終える（Idle なら何もしない）。
        move |s: &mut SpeechRecognizeState| {
            if s.phase != ListenPhase::Idle {
                let message =
                    to_message(SpeechRecognitionError::Internal("speech_recognition lost"));
                finish_listen(s, generation, Err(message));
            }
        },
    );
}

/// macOS/Windows/Android 以外向け: `SpeechRecognizer` トレイト経由の同期呼び出し
/// （`fandhe-cap-mic-record` の driver の `drive` と同型）。
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
    drive(state, sender, &crate::contract::UnsupportedSpeechRecognizer);
}

/// 同期 `SpeechRecognizer` の入口。macOS・iOS・Android・Windows は結果が非同期に届く pull 型の bridge で受け、`SpeechRecognizer` トレイトを
/// 通らない（モジュール doc 参照）ため、成功を偽装せず `UnsupportedSpeechRecognizer`（常に `Unsupported`）を返す。
/// 画面の外のアプリが聞き取るなら [`begin_speech_recognition`] / [`request_stop_speech_recognition`] / [`poll_speech_recognition`] を使う。
/// 画面は effect（`speech_recognize_effect`）の経路を使う。それ以外の OS（Linux 等）も同じく `Unsupported` で、固定の案内（`UNAVAILABLE`）が画面へ出る。
/// 同期実装が要る OS が出たら、ここで OS ごとの実装へ分岐する。
pub fn platform_speech_recognition() -> Box<dyn SpeechRecognizer> {
    Box::new(UnsupportedSpeechRecognizer)
}

/// 聞き取りを OS へ依頼し、結果を [`poll_speech_recognition`] で受け取るための要求 id を返す（macOS・iOS・Android・Windows）。
///
/// 画面の状態型に依存しない非同期の入口で、フレームワークの外のアプリも使える。`max` は 0 なら `InvalidInput`、
/// 上限（30 秒）を超える分は上限へ丸める。1 プロセスに 1 要求で、画面 effect と slot を共有するため、進行中・結果が
/// 未取得の要求（画面側の分を含む）があるうちは `Internal`（busy）で断る。画面 effect 側も、この入口の進行中・未取得の
/// 要求があれば置き換えず処理中の失敗で終える。依頼自体の失敗は `Err`（結果は届かない）。権限の確認は OS が行い、拒否は結果に
/// `PermissionDenied` で届く。それ以外の OS は `Unsupported`。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub fn begin_speech_recognition(max: std::time::Duration) -> Result<u64, SpeechRecognitionError> {
    if max.is_zero() {
        return Err(SpeechRecognitionError::InvalidInput(
            "speech_recognition duration",
        ));
    }
    let max = max.min(std::time::Duration::from_millis(MAX_LISTEN_MS));
    let request_id = bridge::PUBLIC_OWNER
        .begin_excluding(
            &bridge::SLOT,
            &[&bridge::SCREEN_OWNER, &bridge::PUBLIC_OWNER],
        )
        .ok_or(SpeechRecognitionError::Internal("speech_recognition busy"))?;
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    let started = if present(request_id, max) {
        Ok(())
    } else {
        Err(SpeechRecognitionError::Internal(
            "speech_recognition present",
        ))
    };
    #[cfg(target_os = "windows")]
    let started = crate::windows::present(request_id, max);
    if let Err(e) = started {
        bridge::SLOT.clear(request_id);
        bridge::PUBLIC_OWNER.release(request_id);
        return Err(e);
    }
    Ok(request_id)
}

/// 非対応 OS では成功へ倒さず `Unsupported`。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
)))]
pub fn begin_speech_recognition(max: std::time::Duration) -> Result<u64, SpeechRecognitionError> {
    let _ = max;
    Err(SpeechRecognitionError::Unsupported)
}

/// [`begin_speech_recognition`] の聞き取りの停止を依頼する。依頼を OS へ渡せたら `true`。`request_id` が自分の進行中の要求でない・
/// 既に終わっている場合は `false`（結果は自動停止か既存の配送で届く）。停止後の結果は [`poll_speech_recognition`] で取る。
pub fn request_stop_speech_recognition(request_id: u64) -> bool {
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    ))]
    {
        if request_id == 0 || bridge::PUBLIC_OWNER.current() != request_id {
            return false;
        }
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
        return request_stop(request_id);
        #[cfg(target_os = "windows")]
        return crate::windows::request_stop(request_id);
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        let _ = request_id;
        false
    }
}

/// [`begin_speech_recognition`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・取り出し済みは `None`。
/// 結果は 3 値: 認識できた・無音（`Ok(Recognized / NoMatch)`）、利用者の停止や OS 側のキャンセル（`Ok(Cancelled)`。
/// `Err(Cancelled)` はこちらへ寄せる）、失敗（`Err`: 権限拒否・ライフサイクル中断を表す `Internal`・認識器なし・その他）。
pub fn poll_speech_recognition(
    request_id: u64,
) -> Option<Result<crate::contract::SpeechOutcome, SpeechRecognitionError>> {
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    ))]
    {
        if request_id == 0 || bridge::PUBLIC_OWNER.current() != request_id {
            return None;
        }
        // 照合・取得・消去を同じロックで行う（並行 poll でも Some は 1 件だけ）。
        let result = bridge::SLOT.take_ready(request_id)?;
        bridge::PUBLIC_OWNER.release(request_id);
        Some(match result {
            Err(SpeechRecognitionError::Cancelled(_)) => {
                Ok(crate::contract::SpeechOutcome::Cancelled)
            }
            other => other,
        })
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        let _ = request_id;
        None
    }
}

/// Rust 側の保留記録を捨て、Java 側の保留・実行中の要求も止める（`abortPending`、結果は届けない）。
/// 権限ダイアログの許可コールバックが先に来て開始済みでも、後から来ても、画面に反映されない録音・聞き取りは残らない。
/// 外部アプリ（呼ばない構成）では Java 側が再生成を引き継ぐ。
/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照。
/// `fandhe-cap-biometric-auth` の `reset_pending_tracking` と同じ命名・位置づけ）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
    // Java 側の保留・実行中の要求も止める（残すと権限許可後に画面へ反映されない録音・聞き取りが走る）。
    crate::android::abort_pending();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    struct NoopWaker;
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    fn sender_with_channel() -> (
        CompletionSender<SpeechRecognizeState>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<SpeechRecognizeState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            CompletionSender::new(tx, std::sync::Arc::new(NoopWaker)),
            rx,
        )
    }

    #[test]
    fn to_message_maps_unsupported_to_fixed_message() {
        assert_eq!(to_message(SpeechRecognitionError::Unsupported), UNAVAILABLE);
    }

    #[test]
    fn error_from_code_maps_table() {
        use crate::contract::SpeechOutcome;
        assert_eq!(error_from_code(4).unwrap(), SpeechOutcome::Cancelled);
        assert_eq!(error_from_code(8).unwrap(), SpeechOutcome::NoMatch);
        assert!(matches!(
            error_from_code(1).unwrap_err(),
            SpeechRecognitionError::PermissionDenied
        ));
        assert!(matches!(
            error_from_code(3).unwrap_err(),
            SpeechRecognitionError::Internal("speech_recognition busy")
        ));
        assert!(matches!(
            error_from_code(5).unwrap_err(),
            SpeechRecognitionError::Internal("speech_recognition interrupted")
        ));
        assert!(matches!(
            error_from_code(6).unwrap_err(),
            SpeechRecognitionError::Unsupported
        ));
        assert!(matches!(
            error_from_code(7).unwrap_err(),
            SpeechRecognitionError::Disabled("speech_recognition")
        ));
        assert!(matches!(
            error_from_code(2).unwrap_err(),
            SpeechRecognitionError::Internal("speech_recognition")
        ));
        assert!(matches!(
            error_from_code(999).unwrap_err(),
            SpeechRecognitionError::Internal("speech_recognition")
        ));
    }

    #[test]
    fn outcome_from_android_error_maps_table() {
        use crate::contract::SpeechOutcome;
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
            SpeechRecognitionError::PermissionDenied
        ));
        // ネットワーク・サーバー系は一時的に使えない扱い。
        for code in [1, 2, 4, 10, 11, 13] {
            assert!(
                matches!(
                    outcome_from_android_error(code).unwrap_err(),
                    SpeechRecognitionError::Disabled("speech_recognition")
                ),
                "code={code} は Disabled であるべき"
            );
        }
        // ERROR_LANGUAGE_NOT_SUPPORTED → 非対応。
        assert!(matches!(
            outcome_from_android_error(12).unwrap_err(),
            SpeechRecognitionError::Unsupported
        ));
        // ERROR_RECOGNIZER_BUSY → 二重開始。
        assert!(matches!(
            outcome_from_android_error(8).unwrap_err(),
            SpeechRecognitionError::Internal("speech_recognition busy")
        ));
        // ERROR_AUDIO / ERROR_CLIENT → 内部エラー（段階別）。
        assert!(matches!(
            outcome_from_android_error(3).unwrap_err(),
            SpeechRecognitionError::Internal("speech_recognition audio")
        ));
        assert!(matches!(
            outcome_from_android_error(5).unwrap_err(),
            SpeechRecognitionError::Internal("speech_recognition client")
        ));
        // 未知の値（14・15・999）は内部エラーへ畳む既知の天井。
        for code in [14, 15, 999] {
            assert!(
                matches!(
                    outcome_from_android_error(code).unwrap_err(),
                    SpeechRecognitionError::Internal("speech_recognition")
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
            SpeechRecognitionError::PermissionDenied
        ));
    }

    #[test]
    fn error_from_hresult_maps_privacy_declined_to_permission_denied() {
        let code = 0x8004_5509_u32 as i32;
        assert!(matches!(
            error_from_hresult(code, "speech_recognition init"),
            SpeechRecognitionError::PermissionDenied
        ));
    }

    #[test]
    fn error_from_hresult_maps_other_codes_to_internal_with_stage() {
        let code = 0x8000_4005_u32 as i32;
        assert!(matches!(
            error_from_hresult(code, "speech_recognition stop"),
            SpeechRecognitionError::Internal("speech_recognition stop")
        ));
    }

    #[test]
    fn outcome_from_status_success_delegates_to_transcript() {
        use crate::contract::SpeechOutcome;
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
        use crate::contract::SpeechOutcome;
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
        use crate::contract::SpeechOutcome;
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
            SpeechRecognitionError::Unsupported
        ));
        assert!(matches!(
            outcome_from_status(2, String::new()).unwrap_err(),
            SpeechRecognitionError::Unsupported
        ));
    }

    #[test]
    fn outcome_from_status_maps_microphone_and_network_to_internal() {
        // GrammarCompilationFailure = 3, NetworkFailure = 9,
        // MicrophoneUnavailable = 10（実際の WinRT 値。cursor 指摘で修正済み）。
        assert!(matches!(
            outcome_from_status(3, String::new()).unwrap_err(),
            SpeechRecognitionError::Internal("speech_recognition grammar")
        ));
        assert!(matches!(
            outcome_from_status(9, String::new()).unwrap_err(),
            SpeechRecognitionError::Internal("speech_recognition network")
        ));
        assert!(matches!(
            outcome_from_status(10, String::new()).unwrap_err(),
            SpeechRecognitionError::Internal("speech_recognition microphone")
        ));
    }

    #[test]
    fn outcome_from_status_maps_unknown_variant_to_internal() {
        // Unknown = 6（実際の WinRT 値。旧実装は誤って Cancelled へ写していた、
        // cursor 指摘で修正済み）。
        assert!(matches!(
            outcome_from_status(6, String::new()).unwrap_err(),
            SpeechRecognitionError::Internal("speech_recognition")
        ));
    }

    #[test]
    fn outcome_from_status_maps_unknown_to_internal() {
        assert!(matches!(
            outcome_from_status(999, String::new()).unwrap_err(),
            SpeechRecognitionError::Internal("speech_recognition")
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
            &crate::contract::UnsupportedSpeechRecognizer,
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

    /// reset 後に届いた古い id の結果は何も完了させず、reset 後に始めた要求だけが完了する
    /// （Java 側 `abortPending` が結果を届けない前提と、reset 後の旧結果の遅延到着の両方で安全）。
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows"))]
    #[test]
    fn result_for_pre_reset_request_is_ignored() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = bridge::begin();
        bridge::reset();
        assert!(!bridge::deliver(before, error_from_code(5)));
        assert!(bridge::current_request_id().is_none());
        let after = bridge::begin();
        assert!(!bridge::deliver(before, error_from_code(5)));
        assert!(bridge::deliver(after, error_from_code(5)));
    }

    /// 同じ static slot を触るテストを直列にする。
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows"))]
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 公開入口の要求は 1 件だけで、結果は 1 度だけ取れ、画面側が置き換え・横取りしない。
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows"))]
    #[test]
    fn public_request_is_exclusive_and_polled_once() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let public = bridge::PUBLIC_OWNER
            .begin_excluding(
                &bridge::SLOT,
                &[&bridge::SCREEN_OWNER, &bridge::PUBLIC_OWNER],
            )
            .unwrap();
        // 進行中は公開入口の 2 件目も画面側も断る（BUSY）。
        assert!(
            bridge::PUBLIC_OWNER
                .begin_excluding(
                    &bridge::SLOT,
                    &[&bridge::SCREEN_OWNER, &bridge::PUBLIC_OWNER]
                )
                .is_none()
        );
        assert!(
            bridge::SCREEN_OWNER
                .begin_keeping_foreign(&bridge::SLOT)
                .is_none()
        );
        assert!(poll_speech_recognition(public).is_none());
        // 結果は 3 値のまま届く（キャンセルは Ok(Cancelled)）。
        assert!(bridge::deliver(public, error_from_code(4)));
        // 未取得の公開入口の結果を画面側が落とさない。
        assert!(
            bridge::SCREEN_OWNER
                .begin_keeping_foreign(&bridge::SLOT)
                .is_none()
        );
        assert_eq!(
            poll_speech_recognition(public),
            Some(Ok(crate::contract::SpeechOutcome::Cancelled))
        );
        assert!(poll_speech_recognition(public).is_none());
        // 取り出したあとは画面側が使える。
        assert!(
            bridge::SCREEN_OWNER
                .begin_keeping_foreign(&bridge::SLOT)
                .is_some()
        );
        bridge::reset();
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    #[test]
    fn public_entry_is_unsupported_on_other_os() {
        assert_eq!(
            begin_speech_recognition(std::time::Duration::from_secs(1)),
            Err(SpeechRecognitionError::Unsupported)
        );
        assert!(!request_stop_speech_recognition(1));
        assert!(poll_speech_recognition(1).is_none());
    }
}
