//! Phase 11 の拡充ループ（親 #953、Issue #954・#956）: 型サンプル「マイク録音」の
//! effect ドライバ。`fandhe_core::screens::mic_record`（端末依存ゼロ）の要求を観測し、
//! 結果を `CompletionSender` 経由で返す（`fandhe-gallery` の `screens/registry.rs` の `effect`
//! 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! 全ターゲットでコンパイルされる（OS ごとの実装は各 OS の cfg で切り替える）。
//!
//! # macOS/iOS/Android/Windows: pull 型 `bridge`（`biometric_auth.rs`/`share.rs` と同型）
//!
//! macOS/iOS の権限確認（`AVCaptureDevice.requestAccessForMediaType:`）・Android の
//! `RECORD_AUDIO` ランタイム許可ダイアログ、および録音完了（利用者の「停止」操作、
//! `MediaRecorder`/`AVAudioRecorder` の自動停止、`onStop`/`onDestroy` による中断）は
//! いずれも非同期に届く。Windows（Issue #955）は権限プロンプト自体は出ないが、
//! `MediaCapture` の初期化・録音開始・停止がいずれも `IAsyncAction`/
//! `IAsyncOperation` の非同期 API で、同一フレーム内で `start`→`stop` を
//! 呼び切る同期 `drive` では `duration_ms == 0` になり必ず失敗するため
//! （`windows.rs` モジュール doc 参照）、macOS/iOS/Android と同じ
//! pull 型に乗せる。4 OS とも `MicRecorder` トレイトを **通らず**コアの状態機械へ
//! 直接書き戻す（`crate::contract` の doc 参照。Windows の
//! `WindowsMicRecorder` はエンジンとしてのみ `MicRecorder` を実装し、駆動は
//! `windows.rs::present`/`request_stop` が担う）。[`bridge`] は
//! `fandhe-runtime/src/request_slot.rs::RequestSlot` の facade で、`CompletionSender` も
//! 画面の `generation` も保持しない pull 型。1 回の録音セッション（開始 →
//! 自動停止・明示停止・ライフサイクル中断のいずれか片方が届く）につき
//! `deliver` は 1 回だけ呼ばれる契約（`RequestSlot` の first-wins）。
//! [`present`]/[`request_stop`] が macOS/iOS/Android の実装（macOS/iOS:
//! `apple`、Android: `android::request_mic_record_start`/
//! `android::request_mic_record_stop`）へ振り分け、Windows は
//! `windows::present`/`request_stop` を `drive_callback` から直接呼ぶ
//! （エラー型が `Result<(), MicRecordError>` で macOS/iOS/Android の `bool` 戻り値と
//! 異なるため、共通の [`present`]/[`request_stop`] には含めない）。
//!
//! # macOS/iOS/Android/Windows 以外: `MicRecorder` トレイト経由（`UnsupportedMicRecorder` 固定）
//!
//! Web（#958）/Linux は `crate::contract::UnsupportedMicRecorder` を
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
    MicRecordState, RecordOutcome, RecordSummary, finish_record, mark_recording, take_start_request,
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
use crate::contract::{MicRecorder, UnsupportedMicRecorder};
use crate::error::MicRecordError;
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
use fandhe_core::screens::mic_record::{MAX_RECORD_MS, mark_stopping, take_stop_request, tick};

use fandhe_runtime::CompletionSender;

/// `MicRecorder`/`bridge::deliver` 未対応環境（Unsupported）で画面へ出す固定案内
/// （`MicRecordError` の内部構造は含めない、security.md A09。`fandhe-cap-biometric-auth` の `driver.rs::
/// UNAVAILABLE` と同型）。
const UNAVAILABLE: &str = "この環境ではマイク録音を使えません";

/// `MicRecordError` を画面へ渡す文言へ変換する。`Unsupported` は固定文言へ畳み、
/// それ以外は `Display` の文言をそのまま転送する（`fandhe-cap-biometric-auth` の `driver.rs::
/// to_message` と同じ方針）。
fn to_message(e: MicRecordError) -> String {
    match e {
        MicRecordError::Unsupported => UNAVAILABLE.to_string(),
        other => other.to_string(),
    }
}

/// `crate::contract::Recording`（生バイト列を保持）→ 画面表示用の
/// [`RecordSummary`]（サイズのみ、生バイト列は保持しない）への変換
/// （`fandhe-core` は `fandhe-device` に依存しないため、写像は本クレート側の責務）。
fn to_summary(recording: crate::contract::Recording) -> RecordSummary {
    RecordSummary {
        size_bytes: recording.bytes.len(),
        mime: recording.mime,
        duration_ms: recording.duration_ms,
    }
}

/// OS/`MicRecorder` からの生の結果 → [`finish_record`] へ渡す `Result<RecordOutcome,
/// String>` への変換（SSOT）。`MicRecordError::Cancelled` は正常な利用者操作
/// （権限確認プロンプト中の停止）であり失敗ではないため、`to_message` を経由させず
/// `Ok(RecordOutcome::Cancelled)` へ写す（PR #959 レビュー指摘「Cancel shown as
/// recording failure」対応。`to_message` を通すと `FAILED_PREFIX` 付きの `error` に
/// 畳まれ、内部識別子 `"mic_record"` まで画面へ漏れていた）。
fn to_outcome(
    result: Result<crate::contract::Recording, MicRecordError>,
) -> Result<RecordOutcome, String> {
    match result {
        Ok(recording) => Ok(RecordOutcome::Recorded(to_summary(recording))),
        Err(MicRecordError::Cancelled(_)) => Ok(RecordOutcome::Cancelled),
        Err(other) => Err(to_message(other)),
    }
}

/// OS 側から届く結果コード（`code`）→ `MicRecordError` の写像（macOS/iOS `.m`/Android
/// `MicRecordHelper` 共通の SSOT。3 実装とも二重に写像しない、Issue #957）。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 1 | 権限拒否 | `PermissionDenied("mic_record")` |
/// | 3 | 二重開始（busy） | `Internal("mic_record busy")` |
/// | 4 | 許可確認中に利用者が「停止」でキャンセル（macOS/iOS: `.m` 側「許可確認中の
///   キャンセル」節、Android: `MicRecordHelper.requestMicRecordStop` の権限確認中
///   分岐参照） | `Cancelled("mic_record")` |
/// | 5 | ライフサイクル中断（iOS: バックグラウンド遷移により破棄、`.m` 側
///   「バックグラウンド」節参照、Issue #956。Android: 構成変更でない `onStop`・
///   終了による `onDestroy` で破棄、`MicRecordHelper` 参照） | `Internal("mic_record
///   interrupted")` |
/// | それ以外 | 内部エラー（エンコード失敗等） | `Internal("mic_record")` |
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", target_os = "android")),
    allow(dead_code)
)]
pub(crate) fn error_from_code(code: i32) -> MicRecordError {
    match code {
        1 => MicRecordError::PermissionDenied,
        3 => MicRecordError::Internal("mic_record busy"),
        4 => MicRecordError::Cancelled("mic_record"),
        5 => MicRecordError::Internal("mic_record interrupted"),
        _ => MicRecordError::Internal("mic_record"),
    }
}

/// Windows の `windows::core::Error::code().0`（HRESULT の生値）→ 段階別の固定
/// ラベルへの写像（SSOT、`windows.rs` の呼び出し元ごとに同じ
/// 判定を書き直さない）。`stage` は `"mic_record init"`/`"mic_record start"`/
/// `"mic_record stop"`/`"mic_record read"` のいずれか（呼び出し元が固定文字列で
/// 渡す）。`E_ACCESSDENIED`（`0x80070005`、設定 > プライバシー > マイクで拒否）
/// だけ `PermissionDenied("mic_record")` へ、それ以外は `Internal(stage)` へ畳む
/// （マイク未搭載・他アプリ使用中の HRESULT は Microsoft 文書で一意に確定できない
/// ため `Internal` に畳む既知の天井、実装計画 §2.4）。HRESULT の生値・`stage` の
/// 内部識別子は画面へそのまま出さない（`to_message` が `Internal`/`PermissionDenied`
/// の `Display` を通す、security.md A09）。呼び出し元（`windows.rs`）は
/// Windows 限定モジュールのため、本関数自体には `cfg` を付けず全ターゲットで
/// コンパイル・テスト対象にする（`biometric_auth::outcome_from_verification` と
/// 同じ方針、ホスト上の `cargo test` で失敗系を固定する）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn error_from_hresult(code: i32, stage: &'static str) -> MicRecordError {
    const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
    if code == E_ACCESSDENIED {
        MicRecordError::PermissionDenied
    } else {
        MicRecordError::Internal(stage)
    }
}

/// Windows の `MediaCapture` から得た生バイト列・経過時間 →
/// `crate::contract::Recording` への変換（mime は macOS/iOS 実装と同じ
/// `"audio/mp4"` 固定。SSOT の [`crate::contract::build_recording`] を
/// 経由し、0 バイト・`duration_ms == 0`・上限超過を弾く。呼び出し元
/// （`windows.rs`）は Windows 限定モジュールのため、本関数自体には
/// `cfg` を付けず全ターゲットでコンパイル・テスト対象にする（[`error_from_hresult`]
/// と同じ方針）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn recording_from_capture(
    bytes: Vec<u8>,
    elapsed: std::time::Duration,
) -> Result<crate::contract::Recording, MicRecordError> {
    crate::contract::build_recording(bytes, "audio/mp4".to_string(), elapsed.as_millis() as u64)
}

/// macOS/iOS/Android/Windows 経路: OS 側コールバックとの橋渡し（pull 型、
/// `fandhe-runtime/src/request_slot.rs::RequestSlot` の facade。モジュール doc 参照）。
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
    pub(crate) type Outcome = Result<crate::contract::Recording, crate::error::MicRecordError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 公開入口 [`super::begin_mic_record`] が追う自分の分の要求（画面側が置き換え・横取りしない）。
    pub(crate) static PUBLIC_OWNER: SlotOwner = SlotOwner::new();
    /// 公開側の poll 本体（持ち主の照合・取得・喪失判定・解放）を直列にする。並行 poll で成功と喪失が二重に返らないため。
    pub(crate) static PUBLIC_POLL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    /// 画面 effect が追う自分の分の要求（公開入口と slot を共有するため区別する）。
    pub(crate) static SCREEN_OWNER: SlotOwner = SlotOwner::new();

    /// 実際にネイティブ側の録音が開始した request_id（`0` は「まだ未開始」の
    /// 番兵、`RequestSlot::next_request_id` が `1` から採番するのと同じ流儀）。
    /// 権限確認待ちの間は経過時間を進めないための合図
    /// （[`mark_started`]/[`has_started`] 参照、Issue #954 レビュー指摘）。
    static STARTED_REQUEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// 持ち主を付けずに登録する（テスト用。本番は [`SCREEN_OWNER`]・[`PUBLIC_OWNER`] 経由で登録する）。
    #[cfg(test)]
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

    /// 現在進行中の要求 ID（テスト用。本番は持ち主ごとの `current()` を使う）。
    #[cfg(test)]
    pub(crate) fn current_request_id() -> Option<u64> {
        SLOT.snapshot().map(|(request_id, _)| request_id)
    }

    /// OS からの結果を `super::to_outcome` で画面用へ変換してから置く（任意
    /// スレッド。実際には macOS/iOS はメインスレッド、Android は UI スレッド、
    /// Windows はワーカースレッドから届く契約、`fandhe-cap-mic-record` の `apple.rs`/`android.rs`/
    /// `windows.rs` の doc 参照）。`request_id` 不一致・未登録・既に
    /// 結果ありは捨てて `false`。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<crate::contract::Recording, crate::error::MicRecordError>,
    ) -> bool {
        SLOT.deliver(request_id, result)
    }

    /// Android 専用: Activity 再生成直後に `android_main` から呼ぶ
    /// （`fandhe-cap-biometric-auth` の `bridge::reset` と同じ位置づけ）。[`SLOT`]・
    /// [`STARTED_REQUEST_ID`] はプロセス static のため Activity 再生成をまたいで
    /// 残るが、画面状態（`MicRecordState`）は再生成のたびに `Default`（`Idle`）で
    /// 作り直される。放置すると再生成前に開始した録音が破棄されずに残り、
    /// 新しい `Idle` 状態を [`super::drive_callback`] が見て早期 return するため
    /// 結果が画面へ一切反映されない。ここで両方を丸ごと捨て、以後届く旧
    /// `request_id` の [`deliver`]/[`mark_started`] を確実に無視させる。Java 側
    /// （`MicRecordHelper`）は再生成で保留中の要求を捨てず、権限ダイアログの結果を新しい helper が
    /// 引き継いで同じ id で 1 度だけ届ける（終了による破棄は中断（5）を 1 度だけ届ける）。
    /// 画面状態が `Idle` へ戻る構成（`fandhe-gallery`）向けで、状態を保つ外部アプリは呼ばなくてよい。
    /// 呼び出し順に依存しない。
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

/// OS へ提示を依頼する（macOS/iOS: `apple::present`、Android:
/// `android::request_mic_record_start`）。戻り値は「提示依頼を受理できたか」
/// （`false` は呼び出し元が `Err(Internal("mic_record present"))` として畳む、
/// `fandhe-cap-biometric-auth` の `driver.rs::present` と同型）。Windows は戻り値の形が異なる
/// （`Result<(), MicRecordError>`）ため、この共通関数には含めず `drive_callback` から
/// `windows::present` を直接呼ぶ（モジュール doc 参照）。
#[cfg(target_os = "android")]
fn present(request_id: u64, max_duration: std::time::Duration) -> bool {
    crate::android::request_mic_record_start(request_id, max_duration.as_millis() as i32)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn present(request_id: u64, max_duration: std::time::Duration) -> bool {
    crate::apple::present(request_id, max_duration)
}

/// OS へ停止を依頼する（`present` と同じ振り分け。Windows は `drive_callback` から
/// `windows::request_stop` を直接呼ぶ）。
#[cfg(target_os = "android")]
fn request_stop(request_id: u64) -> bool {
    crate::android::request_mic_record_stop(request_id)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn request_stop(request_id: u64) -> bool {
    crate::apple::request_stop(request_id)
}

/// 直近の `mic_record_effect` 呼び出し時刻（経過時間の実測用、`fandhe-cap-tilt` の `driver.rs::
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

/// 前回呼び出しからの経過ミリ秒を実測する。実体は `fandhe_runtime::tick::measure_dt_ms`
/// （画面録画の機能 crate と共用。`fandhe-cap-speech-recognition` の driver が自前の `LAST_TICK` を渡して共用する、Issue #1024）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub(crate) use fandhe_runtime::tick::measure_dt_ms;

/// macOS/iOS/Android/Windows 経路の effect ドライバ本体（`fandhe-cap-biometric-auth` の `driver.rs::
/// drive_callback` と同型）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
fn drive_callback(state: &MicRecordState, sender: &CompletionSender<MicRecordState>) {
    if let Some(request) = take_start_request(state) {
        let _ = sender.send(Box::new(mark_recording));

        // 公開入口の進行中・未取得の要求は置き換えず、画面側を処理中の失敗で終える。
        let Some(request_id) = bridge::SCREEN_OWNER.begin_keeping_foreign(&bridge::SLOT) else {
            let generation = request.generation;
            let message = to_message(MicRecordError::Internal("mic_record busy"));
            let _ = sender.send(Box::new(move |s: &mut MicRecordState| {
                finish_record(s, generation, Err(message));
            }));
            return;
        };
        let max_duration = std::time::Duration::from_millis(MAX_RECORD_MS);
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
        if !present(request_id, max_duration) {
            // 提示依頼自体が同期的に失敗した（コールバックは二度と来ない）。
            bridge::deliver(
                request_id,
                Err(MicRecordError::Internal("mic_record present")),
            );
        }
        #[cfg(target_os = "windows")]
        if let Err(e) = crate::windows::present(request_id, max_duration) {
            // 提示依頼自体が同期的に失敗した（録音中で busy、ワーカースレッド自体が
            // 起動しない。`windows::present` の doc 参照）。
            bridge::deliver(request_id, Err(e));
        }
        return;
    }

    if take_stop_request(state).is_some() {
        let _ = sender.send(Box::new(mark_stopping));
        // 「現在進行中のセッション」の request_id と照合する（macOS/iOS は `.m` 側、
        // Android は `MicRecordHelper.requestMicRecordStop`、Windows は
        // `windows::SESSION` が照合する契約、`.m` ファイル冒頭コメント/
        // `windows.rs::request_stop` 参照）。停止依頼自体が届かなくても
        // （既に終了済み等）、自動停止 or 既存の結果配送が追随するため、ここでは
        // 戻り値を追加でハンドリングしない（`biometric_auth.rs` の防御的分岐と
        // 同じ位置づけ）。
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
    // （`fandhe_core::screens::mic_record` モジュール doc 参照）。ただし
    // ネイティブ側が実際に録音を開始する（`bridge::mark_started`）までは
    // 進めない: `mark_recording` 自体は「停止」ボタンを出すため権限確認開始
    // 時点で先に届くが、その待ち時間まで 30 秒の予算に含めると初回の許可
    // ダイアログが遅い環境で録音が短く打ち切られる（device-bridge-contracts.md
    // 「期限・キャンセル」節・Issue #954 レビュー指摘）。
    let own = bridge::SCREEN_OWNER.current();
    let started = own != 0 && bridge::has_started(own);
    let dt_ms = measure_dt_ms(&LAST_TICK, state.phase == RecordPhase::Recording && started);
    if dt_ms > 0 {
        let _ = sender.send(Box::new(move |s: &mut MicRecordState| tick(s, dt_ms)));
    }

    if state.phase != RecordPhase::Recording && state.phase != RecordPhase::Stopping {
        return;
    }

    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &bridge::SCREEN_OWNER,
        sender,
        |_request_id| {},
        move |s: &mut MicRecordState, outcome| {
            let outcome = to_outcome(outcome);
            // 反映できる状態か（`Idle` へ既に戻っていないか）を確かめてから
            // `finish_record` し、反映できたときだけ `true` を返す（`RequestSlot`
            // 側がそれを見て `clear` する）（`fandhe-cap-biometric-auth` の `driver.rs` と同じ判断）。
            if s.phase != RecordPhase::Idle {
                finish_record(s, generation, outcome);
                true
            } else {
                false
            }
        },
        // 自分の要求が置き換わった・reset で失われた。待ち続けず失敗で終える（Idle なら何もしない）。
        move |s: &mut MicRecordState| {
            if s.phase != RecordPhase::Idle {
                let message = to_message(MicRecordError::Internal("mic_record lost"));
                finish_record(s, generation, Err(message));
            }
        },
    );
}

/// macOS/iOS/Android/Windows 以外向け: `MicRecorder` トレイト経由の同期呼び出し
/// （`fandhe-cap-biometric-auth` の `driver.rs::drive` と同型）。
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
    drive(state, sender, &crate::contract::UnsupportedMicRecorder);
}

/// 同期 `MicRecorder` の入口。macOS・iOS・Android・Windows は結果が非同期に届く pull 型の bridge で受け、`MicRecorder` トレイトを
/// 通らない（モジュール doc 参照）ため、成功を偽装せず `UnsupportedMicRecorder`（常に `Unsupported`）を返す。
/// 画面の外のアプリが録音するなら [`begin_mic_record`] / [`request_stop_mic_record`] / [`poll_mic_record`] を使う。
/// 画面は effect（`mic_record_effect`）の経路を使う。それ以外の OS（Linux 等）も同じく `Unsupported` で、固定の案内（`UNAVAILABLE`）が画面へ出る。
/// 同期実装が要る OS が出たら、ここで OS ごとの実装へ分岐する。
pub fn platform_mic_record() -> Box<dyn MicRecorder> {
    Box::new(UnsupportedMicRecorder)
}

/// 録音を OS へ依頼し、結果を [`poll_mic_record`] で受け取るための要求 id を返す（macOS・iOS・Android・Windows）。
///
/// 画面の状態型に依存しない非同期の入口で、フレームワークの外のアプリも使える。`max_duration` は 0 なら `InvalidInput`、
/// 上限（30 秒）を超える分は上限へ丸める。1 プロセスに 1 要求で、画面 effect と slot を共有するため、進行中・結果が
/// 未取得の要求（画面側の分を含む）があるうちは `Internal`（busy）で断る。画面 effect 側も、この入口の進行中・未取得の
/// 要求があれば置き換えず処理中の失敗で終える。依頼自体の失敗は `Err`（結果は届かない）。権限の確認は OS が行い、拒否は結果に
/// `PermissionDenied` で届く。それ以外の OS は `Unsupported`。
///
/// 画面側の未取得の結果・進行中の要求が残る間も busy で断られる（画面状態が破棄されて effect が回らないと解消しない。
/// lan-discovery と同じ構造）。
/// 前の要求が失われた（reset 等）あと、その id を poll せずに開始すると、旧 id の喪失は通知されない（旧 id の poll は `None`）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android"
))]
pub fn begin_mic_record(max_duration: std::time::Duration) -> Result<u64, MicRecordError> {
    if max_duration.is_zero() {
        return Err(MicRecordError::InvalidInput("mic_record duration"));
    }
    let max_duration = max_duration.min(std::time::Duration::from_millis(MAX_RECORD_MS));
    let request_id = bridge::PUBLIC_OWNER
        .begin_excluding(
            &bridge::SLOT,
            &[&bridge::SCREEN_OWNER, &bridge::PUBLIC_OWNER],
        )
        .ok_or(MicRecordError::Internal("mic_record busy"))?;
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    let started = if present(request_id, max_duration) {
        Ok(())
    } else {
        Err(MicRecordError::Internal("mic_record present"))
    };
    #[cfg(target_os = "windows")]
    let started = crate::windows::present(request_id, max_duration);
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
pub fn begin_mic_record(max_duration: std::time::Duration) -> Result<u64, MicRecordError> {
    let _ = max_duration;
    Err(MicRecordError::Unsupported)
}

/// [`begin_mic_record`] の録音の停止を依頼する。依頼を OS へ渡せたら `true`。`request_id` が自分の進行中の要求でない・
/// 既に終わっている場合は `false`（結果は自動停止か既存の配送で届く）。停止後の結果は [`poll_mic_record`] で取る。
pub fn request_stop_mic_record(request_id: u64) -> bool {
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

/// [`begin_mic_record`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・公開入口でない id・取り出し済みは `None`。
///
/// slot の要求が失われたとき（Android の reset 等）は、1 度だけ `Err(Internal("mic_record lost"))` を返し、以後は `None`。
/// 失われた後に poll せず別の [`begin_mic_record`] をすると持ち主が新しい id へ替わり、旧 id の喪失は通知されず `None` のまま。
/// 並行して同じ id を poll しても、結果と喪失が二重に返ることはない。
///
/// 結果は 3 値: 録音できた（`Ok`）、利用者の停止による権限確認中のキャンセル（`Err(Cancelled)`）、
/// 失敗（`Err(PermissionDenied)`・ライフサイクル中断を表す `Internal`・その他）。0 バイト・長さ 0 の録音は成功にせず `InvalidInput`。
pub fn poll_mic_record(
    request_id: u64,
) -> Option<Result<crate::contract::Recording, MicRecordError>> {
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    ))]
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
        Some(Err(MicRecordError::Internal("mic_record lost")))
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
/// `android_main` で effect の駆動前に呼ぶ前提で、開始（`begin_mic_record`・画面 effect）と並走させない
/// （reset と Java 側 `abortPending` の間に別スレッドの開始が登録されると、その要求は pending のまま残りうる）。
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
        CompletionSender<MicRecordState>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<MicRecordState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            CompletionSender::new(tx, std::sync::Arc::new(NoopWaker)),
            rx,
        )
    }

    #[test]
    fn error_from_code_maps_unsupported_to_fixed_message() {
        assert_eq!(to_message(MicRecordError::Unsupported), UNAVAILABLE);
    }

    #[test]
    fn error_from_code_maps_table() {
        assert!(matches!(
            error_from_code(1),
            MicRecordError::PermissionDenied
        ));
        assert!(matches!(
            error_from_code(3),
            MicRecordError::Internal("mic_record busy")
        ));
        assert!(matches!(
            error_from_code(4),
            MicRecordError::Cancelled("mic_record")
        ));
        assert!(matches!(
            error_from_code(5),
            MicRecordError::Internal("mic_record interrupted")
        ));
        assert!(matches!(
            error_from_code(2),
            MicRecordError::Internal("mic_record")
        ));
        assert!(matches!(
            error_from_code(999),
            MicRecordError::Internal("mic_record")
        ));
    }

    #[test]
    fn to_summary_preserves_size_mime_and_duration() {
        let recording = crate::contract::Recording {
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
        drive(&state, &sender, &crate::contract::UnsupportedMicRecorder);

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
            MicRecordError::PermissionDenied
        ));
    }

    #[test]
    fn error_from_hresult_maps_other_codes_to_internal_with_stage() {
        // `E_FAIL`。マイク未搭載・他アプリ使用中を含め既知の天井として `Internal`
        // に畳む（実装計画 §2.4）。
        let code = 0x8000_4005_u32 as i32;
        assert!(matches!(
            error_from_hresult(code, "mic_record stop"),
            MicRecordError::Internal("mic_record stop")
        ));
    }

    #[test]
    fn recording_from_capture_rejects_empty_bytes() {
        let err =
            recording_from_capture(Vec::new(), std::time::Duration::from_secs(1)).unwrap_err();
        assert!(matches!(
            err,
            MicRecordError::InvalidInput("mic_record bytes")
        ));
    }

    #[test]
    fn recording_from_capture_rejects_zero_duration() {
        let err = recording_from_capture(vec![1, 2, 3], std::time::Duration::ZERO).unwrap_err();
        assert!(matches!(
            err,
            MicRecordError::InvalidInput("mic_record duration")
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

    /// 中断（code 5）は 1 度だけ届き、古い id の中断は後から始めた要求を完了させない
    /// （Java 側が再生成・終了で届ける中断の Rust 側の受け手）。
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows"))]
    #[test]
    fn stale_interrupt_does_not_complete_new_request_and_delivers_once() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old = bridge::begin();
        assert!(bridge::deliver(old, Err(error_from_code(5))));
        assert!(!bridge::deliver(old, Err(error_from_code(5))));

        let stale = bridge::begin();
        let current = bridge::begin();
        assert!(!bridge::deliver(stale, Err(error_from_code(5))));
        assert!(bridge::deliver(current, Err(error_from_code(5))));
    }

    /// reset 後に届いた古い id の結果は何も完了させず、reset 後に始めた要求だけが完了する
    /// （Java 側 `abortPending` が結果を届けない前提と、reset 後の旧結果の遅延到着の両方で安全）。
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows"))]
    #[test]
    fn result_for_pre_reset_request_is_ignored() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = bridge::begin();
        bridge::reset();
        assert!(!bridge::deliver(before, Err(error_from_code(5))));
        assert!(bridge::current_request_id().is_none());
        let after = bridge::begin();
        assert!(!bridge::deliver(before, Err(error_from_code(5))));
        assert!(bridge::deliver(after, Err(error_from_code(5))));
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
        assert!(poll_mic_record(public).is_none());
        // 結果は 3 値のまま届く（キャンセルは Err(Cancelled)）。
        assert!(bridge::deliver(public, Err(error_from_code(4))));
        // 未取得の公開入口の結果を画面側が落とさない。
        assert!(
            bridge::SCREEN_OWNER
                .begin_keeping_foreign(&bridge::SLOT)
                .is_none()
        );
        assert!(matches!(
            poll_mic_record(public),
            Some(Err(MicRecordError::Cancelled(_)))
        ));
        assert!(poll_mic_record(public).is_none());
        // 取り出したあとは画面側が使える。
        let screen = bridge::SCREEN_OWNER
            .begin_keeping_foreign(&bridge::SLOT)
            .unwrap();
        assert!(poll_mic_record(screen).is_none());
        bridge::reset();
    }

    /// reset で要求を失ったら、公開側は 1 度だけ失敗を返し、以後 `None`、再び開始できる。
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows"))]
    #[test]
    fn public_request_lost_by_reset_fails_once() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let begin = || {
            bridge::PUBLIC_OWNER.begin_excluding(
                &bridge::SLOT,
                &[&bridge::SCREEN_OWNER, &bridge::PUBLIC_OWNER],
            )
        };
        let id = begin().unwrap();
        assert!(poll_mic_record(id).is_none());
        bridge::reset();
        assert!(matches!(
            poll_mic_record(id),
            Some(Err(MicRecordError::Internal(_)))
        ));
        assert!(poll_mic_record(id).is_none());
        assert!(begin().is_some());
        bridge::reset();
    }

    /// 同じ id を 2 スレッドが同時に poll しても、`Some` は合計 1 件で、成功と喪失が混ざらない。
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows"))]
    #[test]
    fn concurrent_poll_delivers_once() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..500 {
            bridge::reset();
            let id = bridge::PUBLIC_OWNER
                .begin_excluding(
                    &bridge::SLOT,
                    &[&bridge::SCREEN_OWNER, &bridge::PUBLIC_OWNER],
                )
                .unwrap();
            assert!(bridge::deliver(id, Err(error_from_code(4))));
            let barrier = std::sync::Barrier::new(2);
            let results: Vec<_> = std::thread::scope(|s| {
                let hs: Vec<_> = (0..2)
                    .map(|_| {
                        s.spawn(|| {
                            barrier.wait();
                            poll_mic_record(id)
                        })
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap()).collect()
            });
            assert_eq!(results.iter().filter(|r| r.is_some()).count(), 1);
            assert!(
                !results
                    .iter()
                    .flatten()
                    .any(|r| matches!(r, Err(MicRecordError::Internal(m)) if m.ends_with("lost")))
            );
        }
        bridge::reset();
    }

    /// 旧 id・公開入口でない id・0 の poll は `None`。
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows"))]
    #[test]
    fn poll_of_unknown_id_is_none() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let old = bridge::PUBLIC_OWNER
            .begin_excluding(
                &bridge::SLOT,
                &[&bridge::SCREEN_OWNER, &bridge::PUBLIC_OWNER],
            )
            .unwrap();
        bridge::reset();
        let new = bridge::PUBLIC_OWNER
            .begin_excluding(
                &bridge::SLOT,
                &[&bridge::SCREEN_OWNER, &bridge::PUBLIC_OWNER],
            )
            .unwrap();
        assert!(poll_mic_record(0).is_none());
        assert!(poll_mic_record(old).is_none());
        assert!(poll_mic_record(new + 1).is_none());
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
            begin_mic_record(std::time::Duration::from_secs(1)),
            Err(MicRecordError::Unsupported)
        );
        assert!(!request_stop_mic_record(1));
        assert!(poll_mic_record(1).is_none());
    }
}
