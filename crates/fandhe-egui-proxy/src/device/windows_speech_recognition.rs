//! Windows 向け音声認識実装（Issue #1024）。`device/speech_recognition.rs` の
//! Windows 経路から呼ばれ、WinRT `Windows.Media.SpeechRecognition.SpeechRecognizer`
//! を [`present`]/[`request_stop`] のセッション管理（ワーカースレッド・期限タイマ・
//! 停止合図）で駆動する（`SpeechRecognizer` トレイトは通さない、`fandhe_device::
//! speech_recognition` モジュール doc・`windows_mic_record.rs` モジュール doc
//! 「同期 `drive` 経路が使えない理由」と同じ判断）。
//!
//! # WinRT 呼び出しはワーカースレッド + `.get()` で同期待ち
//!
//! `CompileConstraintsAsync`/`RecognizeAsync`/`StopRecognitionAsync` は
//! `windows-future` の `.get()`（`windows_mic_record.rs` 参照）でブロック待ちできる
//! が、egui update ループ（UI スレッド）をブロックしないよう `std::thread::spawn`
//! のワーカー上で呼ぶ（`windows_mic_record.rs::present` と同型）。`RoInitialize` は
//! 呼ばない（`windows-core` の `CO_E_NOTINITIALIZED` → MTA フォールバック、既存の
//! Windows 実装群と同じ前提）。`windows` crate 側が safe fn としてラップ済みのため
//! `unsafe` は不要。
//!
//! # 期限はワーカーのタイマで一致させる
//!
//! `RecognizeAsync` は非同期に開始し（呼び出し自体はブロックしない）、ワーカーは
//! `mpsc::Receiver::recv_timeout(max)` で停止合図（[`request_stop`]）または期限
//! 到来を待ってから `StopRecognitionAsync` を 1 回だけ呼ぶ。`max` は
//! `drive_callback` が渡す `MAX_LISTEN_MS`（30 秒、`fandhe_core::screens::
//! mic_record::MAX_RECORD_MS` と同じ SSOT）で、コアの `tick` が同じ値で自動停止
//! するのと揃える（`device-bridge-contracts.md`「期限・キャンセル」節）。
//! `StopRecognitionAsync` は `RecognizeAsync` の `IAsyncOperation` を強制的に
//! 完了させる契約（WinRT API 設計）のため、その後の `op.get()` は速やかに戻る。
//!
//! # 失敗の写像
//!
//! `CompileConstraintsAsync`/`RecognizeAsync` の結果 `Status` は
//! [`super::speech_recognition::outcome_from_status`]（SSOT、`device/
//! speech_recognition.rs` に `cfg` 無しで置き、ホストの `cargo test` で固定する）で
//! 写す。HRESULT 失敗は [`super::speech_recognition::error_from_hresult`] で写す。
//!
//! # 並行性・世代
//!
//! セッションは [`SESSION`]（`Mutex<Option<Session>>`）。「現在の要求か」の判定と
//! 更新は同じロック内で行う（`device-bridge-contracts.md` 2 節）。完了配送は
//! `device/speech_recognition::bridge::deliver`（`RequestSlot` の first-wins・
//! request_id 照合）に任せ、古い世代の結果は捨てられる。権限プロンプトは
//! Windows では出ない（拒否は `CompileConstraintsAsync`/`RecognizeAsync` の同期
//! 失敗、HRESULT で判定）ため、`RecognizeAsync` 開始直後に `bridge::mark_started`
//! を呼び、そこから `tick` が進む。
//!
//! # 既知の天井
//!
//! 既定の dictation は Windows の「オンライン音声認識」への同意が要る（未同意は
//! `PermissionDenied`）。停止後の確定結果の有無は OS 任せで、取り消しになれば
//! `Cancelled`。連続認識・言語選択（システム言語固定）は範囲外（実装計画 §8）。

use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Media::SpeechRecognition::SpeechRecognizer as WinSpeechRecognizer;
use windows_future::AsyncStatus;

use fandhe_device::DeviceError;

use super::speech_recognition::{bridge, error_from_hresult, outcome_from_status};

/// 進行中のワーカーへ「今すぐ停止」を伝える合図の送り先（[`present`] が
/// スレッド起動時に作り、[`request_stop`] が一致する `request_id` にだけ送る）。
struct Session {
    request_id: u64,
    stop_tx: mpsc::Sender<()>,
}

/// 現在進行中のセッション（プロセス static、1 度に 1 セッションのみ許可）。
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn init_error() -> DeviceError {
    DeviceError::Internal("speech_recognition init")
}

/// `device/speech_recognition.rs` の Windows 経路（`drive_callback`）から呼ばれる。
/// `request_id` は `bridge::begin` が払い出したプロセス内一意な ID。聞き取り中
/// （[`SESSION`] が `Some`）なら `Err(Internal("speech_recognition busy"))` を返し
/// （二重起動を許さない）、呼び出し元が `bridge::deliver` する。
///
/// 受理できたときはワーカースレッドを起動して即座に返る（`CompileConstraintsAsync`/
/// `RecognizeAsync`/`StopRecognitionAsync` を呼ぶのはワーカー側、モジュール doc
/// 参照）。
pub(crate) fn present(request_id: u64, max: Duration) -> Result<(), DeviceError> {
    let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if session.is_some() {
        return Err(DeviceError::Internal("speech_recognition busy"));
    }
    let (stop_tx, stop_rx) = mpsc::channel();
    *session = Some(Session {
        request_id,
        stop_tx,
    });
    drop(session);

    let spawn_result = std::thread::Builder::new().spawn(move || {
        let result = run_session(request_id, max, &stop_rx);

        // `bridge::deliver` より前に `SESSION` を解除する（自分の request_id と
        // 一致するときだけ、後続要求を消さない、`windows_mic_record.rs::present`
        // と同じ順序の理由: UI スレッドが結果を取り込んで即座に次の聞き取りを
        // 開始したとき、まだ `SESSION` が `Some` のままで `present` が誤って
        // busy を返すのを防ぐ）。
        let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        if session.as_ref().is_some_and(|s| s.request_id == request_id) {
            *session = None;
        }
        drop(session);

        bridge::deliver(request_id, result);
    });

    if spawn_result.is_err() {
        // OS のスレッド生成自体が失敗した（`windows_mic_record.rs::present` と
        // 同じ理由でここで解除する）。
        let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        if session.as_ref().is_some_and(|s| s.request_id == request_id) {
            *session = None;
        }
        drop(session);
        return Err(DeviceError::Internal("speech_recognition present"));
    }

    Ok(())
}

/// ワーカースレッド本体。`CompileConstraintsAsync` → `RecognizeAsync` → 停止合図/
/// 期限到来待ち → `StopRecognitionAsync` → 結果取得、の一連を担う
/// （モジュール doc 参照）。
fn run_session(
    request_id: u64,
    max: Duration,
    stop_rx: &mpsc::Receiver<()>,
) -> Result<fandhe_device::speech_recognition::SpeechOutcome, DeviceError> {
    let recognizer = WinSpeechRecognizer::new().map_err(|_| init_error())?;

    // 既定の `EndSilenceTimeout`（150ms）/`InitialSilenceTimeout`（5s）のままだと
    // 発話中の短い間や話し始めの間で `RecognizeAsync` が自然完了し、以降の発話が
    // 失われる（cursor 指摘）。Rust 側の期限（`max`、`MAX_LISTEN_MS`）と揃えて
    // 「利用者が話し終えるまで OS 側では打ち切らない」方針にする（打ち切りは
    // `request_stop`/期限到来のポーリングが担う、上記ループ参照）。取得・設定の
    // いずれかに失敗すると既定値のまま `RecognizeAsync` が進んでしまい、画面が
    // 示す 30 秒契約より前に `NoMatch` になり得る（codex 指摘）ため、続行せず
    // 開始エラーとして返す。
    let timeouts = recognizer.Timeouts().map_err(|_| init_error())?;
    let span = windows::Foundation::TimeSpan {
        // `TimeSpan::Duration` の単位は 100ns（1ms = 10_000）。
        Duration: (max.as_millis() as i64).saturating_mul(10_000),
    };
    timeouts
        .SetEndSilenceTimeout(span)
        .map_err(|_| init_error())?;
    timeouts
        .SetInitialSilenceTimeout(span)
        .map_err(|_| init_error())?;

    let compile_result = recognizer
        .CompileConstraintsAsync()
        .map_err(|e| error_from_hresult(e.code().0, "speech_recognition init"))?
        .get()
        .map_err(|e| error_from_hresult(e.code().0, "speech_recognition init"))?;
    let compile_status = compile_result.Status().map_err(|_| init_error())?;
    // `SpeechRecognitionResultStatus::Success`（0）以外はコンパイル自体の失敗
    // （実装計画 §3.5「開始」節）。`compile_status.0` を渡す契約
    // （`outcome_from_status` のテスト参照）。
    if compile_status.0 != 0 {
        return Err(match outcome_from_status(compile_status.0, String::new()) {
            Err(e) => e,
            Ok(_) => DeviceError::Internal("speech_recognition init"),
        });
    }

    let op = recognizer
        .RecognizeAsync()
        .map_err(|e| error_from_hresult(e.code().0, "speech_recognition start"))?;
    bridge::mark_started(request_id);

    // 停止合図（`Ok`）・期限到来・`RecognizeAsync` の自然完了（`op.Status()` が
    // `Started` を脱した）のいずれか最初に起きたものまで待つ。`recv_timeout(max)`
    // を待ち切ってから読むと、確定済みの結果が最大 `max` 秒間画面へ届かない
    // （codex 指摘）ため、短い間隔（`POLL_INTERVAL`）でポーリングして早期に
    // 気づく。送信側 drop（`Disconnected`）は通常起きないが防御的に停止扱いする
    // （`windows_mic_record.rs::present` の待ち方と同型）。
    const POLL_INTERVAL: Duration = Duration::from_millis(100);
    let deadline = Instant::now() + max;
    loop {
        match stop_rx.recv_timeout(POLL_INTERVAL) {
            Ok(()) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    break;
                }
                // `Status()` 取得に失敗しても待ち続ける（次のポーリングか期限で
                // 決着する。ここで `?` にすると一時的な取得失敗で早期 return し、
                // 進行中のセッションを取り消さないまま抜けてしまう）。
                if op.Status().is_ok_and(|s| s != AsyncStatus::Started) {
                    break;
                }
            }
        }
    }

    // `op` が自然完了済み（ポーリングで検知した「自然完了」経路）なら
    // `StopRecognitionAsync` を呼ばない（完了済みセッションへの停止要求は
    // WinRT 側で不正状態になり得るため）。`Started` のままなら停止合図または
    // 期限到来なので、明示的に停止を求める。
    if op.Status().is_ok_and(|s| s == AsyncStatus::Started) {
        recognizer
            .StopRecognitionAsync()
            .and_then(|stop_op| stop_op.get())
            .map_err(|e| error_from_hresult(e.code().0, "speech_recognition stop"))?;
    }

    // 上記のいずれの経路でも `op` は完了済みのはず（WinRT の契約、モジュール doc
    // 「期限」節参照）。取り出した結果を `outcome_from_status` で写す。
    let result = op
        .get()
        .map_err(|e| error_from_hresult(e.code().0, "speech_recognition stop"))?;
    let status = result.Status().map_err(|_| init_error())?;
    // `Status() == Success`（0）のとき `Text()` の取得失敗を無音（空文字列）へ
    // 畳むと `outcome_from_status` が `NoMatch` を返し、WinRT 側の実際の取得
    // 失敗が嘘の成功（無音）に化ける（codex 指摘、device-bridge-contracts.md
    // 「6. 嘘の成功」）。Success 以外は `outcome_from_status` がテキストを見ない
    // ため、取得失敗は空文字列で許容する。
    let text = if status.0 == 0 {
        result
            .Text()
            .map(|s| s.to_string_lossy())
            .map_err(|_| DeviceError::Internal("speech_recognition text"))?
    } else {
        result
            .Text()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default()
    };
    outcome_from_status(status.0, text)
}

/// `device/speech_recognition.rs` の「停止」ボタンハンドラから呼ばれる。
/// `request_id` が現在進行中のセッションと一致するときだけ停止合図を送る。
/// 不一致・未登録は `false`（無視。自動停止 or 既存の結果配送が追随する、
/// `apple_speech_recognition.rs::request_stop` と同じ位置づけ）。
pub(crate) fn request_stop(request_id: u64) -> bool {
    let session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    match session.as_ref() {
        Some(s) if s.request_id == request_id => s.stop_tx.send(()).is_ok(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_stop_without_session_returns_false() {
        assert!(!request_stop(1));
    }
}
