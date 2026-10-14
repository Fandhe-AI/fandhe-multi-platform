//! Windows 向けマイク録音実装（Issue #955）。`device/mic_record.rs` の Windows 経路
//! から呼ばれ、WinRT `Windows.Media.Capture.MediaCapture` による音声録音を
//! [`MicRecorder`] 実装（[`WindowsMicRecorder`]、OS 呼び出しのエンジン）として書く。
//! セッション管理（ワーカースレッド・期限タイマ・停止合図）は本モジュールの
//! [`present`]/[`request_stop`] が担う（`MicRecorder` トレイトはエンジン、駆動は
//! `bridge` 経由、`fandhe_device::mic_record` モジュール doc 参照）。
//!
//! # 同期 `drive` 経路が使えない理由
//!
//! `device/mic_record.rs` の非 macOS/Windows 経路（`drive`）は `start`→`stop` を
//! 同一フレーム内で連続呼び出しするため、実録音を通すと 0 ms で停止し
//! `build_recording` の `duration_ms == 0` 拒否で必ず `Err` になる。Windows は
//! macOS と同じ pull 型 `bridge`（`device/request_slot.rs::RequestSlot`）へ乗せ、
//! `present`/`request_stop` を `device/mic_record.rs::drive_callback` の
//! Windows 分岐から呼ぶ（`biometric_auth.rs` の Windows 分岐と同型）。
//!
//! # WinRT 呼び出しはワーカースレッド + `.get()` で同期待ち
//!
//! `InitializeWithSettingsAsync`/`StartRecordToStreamAsync`/`StopRecordAsync`/
//! `DataReader::LoadAsync` は `windows-future` の `.get()`（`windows_biometric.rs`
//! 参照）でブロック待ちできるが、egui update ループ（UI スレッド）をブロックしない
//! よう `std::thread::spawn` のワーカー上で呼ぶ（`secure_credential.rs::drive` の
//! スレッド方式と同型）。`RoInitialize` は呼ばない（`windows-core` の
//! `CO_E_NOTINITIALIZED` → MTA フォールバック、#898/#907/#913/#923 と同じ前提）。
//! `windows` クレート側が safe fn としてラップ済みのため `unsafe` は不要
//! （`windows_secure_credential.rs`/`windows_text_scale.rs` と同型。`lib.rs` の
//! Windows `deny(unsafe_code)` に対する `allow` も付けない）。
//!
//! # 期限はワーカーのタイマで一致させる
//!
//! ワーカーは `start()` 成功後 `mpsc::Receiver::recv_timeout(max_duration)` で待ち、
//! 停止合図（[`request_stop`]）または期限到来のどちらでも `stop()` を 1 回だけ呼ぶ。
//! `max_duration` は `drive_callback` が渡す `MAX_RECORD_MS`（30 秒）で、コアの
//! `tick` が同じ値で自動停止するのと揃える（`device-bridge-contracts.md`
//! 「期限・キャンセル」節）。`MediaCapture.RecordLimitationExceeded`（OS の 3 時間
//! 上限）は 30 秒では到達しないため購読しない。
//!
//! # 失敗の写像（嘘の成功を作らない）
//!
//! 初期化・開始・停止・読み出しの各失敗は [`super::mic_record::error_from_hresult`]
//! （`device/mic_record.rs` に `cfg` 無しで置き、ホストの `cargo test` で固定する）
//! で段階別の固定ラベルへ畳む。マイク未搭載・他アプリ使用中の HRESULT は
//! Microsoft 文書で一意に確定できないため `Internal` に畳む（既知の天井、
//! 対応表 note 参照。偽の成功にはならない）。
//!
//! # 並行性・世代
//!
//! セッションは [`SESSION`]（`Mutex<Option<Session>>`）。「現在の要求か」の判定と
//! 更新は同じロック内で行う（`device-bridge-contracts.md` 2 節）。完了配送は
//! `device/mic_record::bridge::deliver`（`RequestSlot` の first-wins・request_id
//! 照合）に任せ、古い世代の結果は捨てられる。権限プロンプトは Windows では出ない
//! （拒否は `InitializeAsync` の同期失敗）ため、`start()` 成功直後に
//! `bridge::mark_started` を呼び、そこから `tick` が進む。
//!
//! # 既知の天井
//!
//! マイク未搭載・他アプリ使用中の HRESULT 個別判定は行わず `Internal` に畳む
//! （実機で値を確認してから別 Issue、実装計画 §8）。AAC モノラル・30 秒固定・
//! 他アプリ割り込みからの復帰なし。

use std::cell::RefCell;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Media::Capture::{
    MediaCapture, MediaCaptureInitializationSettings, StreamingCaptureMode,
};
use windows::Media::MediaProperties::{AudioEncodingQuality, MediaEncodingProfile};
use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};

use fandhe_device::mic_record::{MicRecorder, Recording, MAX_RECORDING_BYTES};
use fandhe_device::DeviceError;

use super::mic_record::{bridge, error_from_hresult, recording_from_capture};

/// 進行中のワーカーへ「今すぐ停止」を伝える合図の送り先（[`present`] が
/// スレッド起動時に作り、[`request_stop`] が一致する `request_id` にだけ送る）。
struct Session {
    request_id: u64,
    stop_tx: mpsc::Sender<()>,
}

/// 現在進行中のセッション（プロセス static、1 度に 1 セッションのみ許可）。
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// 録音中の `MediaCapture`/バッファと開始時刻（[`WindowsMicRecorder::start`] が
/// 保持し、[`WindowsMicRecorder::stop`] が取り出す）。COM オブジェクト
/// （`IUnknown` 実装、`NonNull<c_void>` を内部に持つ）は `Send` ではないため、
/// `MicRecorder: Send + Sync` を満たす [`WindowsMicRecorder`] の構造体フィールドには
/// 置けない。[`present`] のワーカースレッドは `start` から `stop` まで単一スレッド上
/// で完結する契約（モジュール doc 参照）のため、スレッドローカル
/// （[`ACTIVE`]、`windows_biometric.rs::HELD` と同じ方針）へ保持する。
struct Active {
    capture: MediaCapture,
    stream: InMemoryRandomAccessStream,
    started_at: Instant,
}

thread_local! {
    /// [`Active`] の保持先（モジュール doc「録音中の `MediaCapture`」節参照）。
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
}

/// WinRT `MediaCapture`（Audio モード）による [`MicRecorder`] 実装（ゼロサイズ・
/// フィールド無しのため自動的に `Send + Sync`。状態は [`ACTIVE`]
/// スレッドローカルに置く、モジュール doc 参照）。エンジンのみを担い、セッション
/// 管理は [`present`]/[`request_stop`] が担う。
struct WindowsMicRecorder;

impl MicRecorder for WindowsMicRecorder {
    fn start(&self, _max_duration: Duration) -> Result<(), DeviceError> {
        // `max_duration` は受け取るが自動停止は呼び出し元（`present` のワーカー）の
        // タイマが担う（モジュール doc 参照。`MediaCapture` 自体に自動停止 API は
        // 使わない）。
        if ACTIVE.with_borrow(|a| a.is_some()) {
            return Err(DeviceError::Internal("mic_record busy"));
        }

        let settings = MediaCaptureInitializationSettings::new().map_err(|_| init_error())?;
        settings
            .SetStreamingCaptureMode(StreamingCaptureMode::Audio)
            .map_err(|_| init_error())?;

        let capture = MediaCapture::new().map_err(|_| init_error())?;
        capture
            .InitializeWithSettingsAsync(&settings)
            .map_err(|e| error_from_hresult(e.code().0, "mic_record init"))?
            .get()
            .map_err(|e| error_from_hresult(e.code().0, "mic_record init"))?;

        // `InitializeWithSettingsAsync` 成功後にここから先で失敗すると、`capture` を
        // 束縛したまま drop するだけではマイクを掴んだままになる（`Close`/
        // `StopRecordAsync` を呼ばない限り解放されない）。次回録音が汎用の内部
        // エラーで失敗し続ける（PR #960 レビュー指摘）ため、失敗時は必ず `Close()`
        // で解放してから返す。
        match start_recording_stream(&capture) {
            Ok((stream, started_at)) => {
                ACTIVE.with_borrow_mut(|a| {
                    *a = Some(Active {
                        capture,
                        stream,
                        started_at,
                    })
                });
                Ok(())
            }
            Err(e) => {
                let _ = capture.Close();
                Err(e)
            }
        }
    }

    fn stop(&self) -> Result<Recording, DeviceError> {
        let Active {
            capture,
            stream,
            started_at,
        } = ACTIVE
            .with_borrow_mut(|a| a.take())
            .ok_or(DeviceError::Internal("mic_record not recording"))?;
        let elapsed = started_at.elapsed();

        // `stop_and_read` の成否によらず、ここで必ず `capture.Close()` を呼ぶ
        // （正常終了・即時キャンセル・停止失敗・サイズ不正・読み出し失敗の
        // どの経路でも解放する。`Close()` 自体の失敗は握りつぶす —
        // `stop()` の戻り値は録音データの成否を表すため、後始末の失敗で
        // 上書きしない。PR #960 レビュー指摘・device-bridge-contracts.md
        // 「嘘の成功」節）。
        let result = stop_and_read(&capture, &stream, elapsed);
        let _ = capture.Close();
        result
    }
}

/// `StopRecordAsync` から読み出し完了までの本体（[`WindowsMicRecorder::stop`]
/// から分離。呼び出し元が `capture.Close()` を全経路で一箇所から呼べるように
/// するための切り出し）。
fn stop_and_read(
    capture: &MediaCapture,
    stream: &InMemoryRandomAccessStream,
    elapsed: Duration,
) -> Result<Recording, DeviceError> {
    // `StopRecordAsync` の結果は即座に処理せず保留する。`stop_rx.recv_timeout`
    // に既に停止合図が積まれていた場合（`present` のワーカーが `start()`
    // （WinRT 同期待ち）を終える前に「停止」が押された場合）、`start()`
    // 復帰直後の数 ms（スケジューリング遅延を含む）で `stop()` が呼ばれる。
    // これは実際には録音が始まる前のキャンセルであり失敗ではないため、
    // macOS 実装（`apple_mic_record.rs::error_from_code` の code 4）と
    // 同じく `Cancelled` として扱う（PR #960 レビュー指摘、Bugbot 再指摘）。
    // 開始直後は WinRT 側もまだ録音状態へ遷移しきっていないことがあり、
    // `StopRecordAsync` 自体が stop-too-soon の HRESULT で失敗する場合が
    // あるため、その失敗（`Internal` 化）や後続の size==0 判定（`InvalidInput`
    // 化）より先に、経過時間だけでキャンセル判定を確定させる（0ms 固定では
    // 数 ms のスケジューリング遅延を取りこぼすため閾値で判定する）。
    const IMMEDIATE_STOP_THRESHOLD: Duration = Duration::from_millis(50);
    let stop_result = capture.StopRecordAsync().and_then(|op| op.get());
    if elapsed < IMMEDIATE_STOP_THRESHOLD {
        return Err(DeviceError::Cancelled("mic_record"));
    }
    stop_result.map_err(|e| error_from_hresult(e.code().0, "mic_record stop"))?;

    let size = stream.Size().map_err(|_| read_error())?;
    if size == 0 || size as usize > MAX_RECORDING_BYTES {
        return Err(DeviceError::InvalidInput("mic_record size"));
    }

    let input = stream.GetInputStreamAt(0).map_err(|_| read_error())?;
    let reader = DataReader::CreateDataReader(&input).map_err(|_| read_error())?;
    reader
        .LoadAsync(size as u32)
        .map_err(|e| error_from_hresult(e.code().0, "mic_record read"))?
        .get()
        .map_err(|e| error_from_hresult(e.code().0, "mic_record read"))?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).map_err(|_| read_error())?;

    recording_from_capture(bytes, elapsed)
}

/// 初期化済みの `capture` に対して録音先ストリームを作り録音を開始する
/// （[`WindowsMicRecorder::start`] から分離。呼び出し元が失敗時の `Close()` を
/// 一箇所で担えるようにするための切り出し）。
fn start_recording_stream(
    capture: &MediaCapture,
) -> Result<(InMemoryRandomAccessStream, Instant), DeviceError> {
    let stream = InMemoryRandomAccessStream::new().map_err(|_| init_error())?;
    let profile =
        MediaEncodingProfile::CreateM4a(AudioEncodingQuality::Auto).map_err(|_| init_error())?;
    capture
        .StartRecordToStreamAsync(&profile, &stream)
        .map_err(|e| error_from_hresult(e.code().0, "mic_record start"))?
        .get()
        .map_err(|e| error_from_hresult(e.code().0, "mic_record start"))?;
    Ok((stream, Instant::now()))
}

fn init_error() -> DeviceError {
    DeviceError::Internal("mic_record init")
}

fn read_error() -> DeviceError {
    DeviceError::Internal("mic_record read")
}

/// `device/mic_record.rs` の Windows 経路（`drive_callback`）から呼ばれる。
/// `request_id` は `bridge::begin` が払い出したプロセス内一意な ID。録音中
/// （[`SESSION`] が `Some`）なら `Err(Internal("mic_record busy"))` を返し
/// （二重起動を許さない）、呼び出し元が `bridge::deliver` する。
///
/// 受理できたときはワーカースレッドを起動して即座に返る（`start()`/`stop()` を
/// 呼ぶのはワーカー側、モジュール doc「同期 `drive` 経路が使えない理由」参照）。
pub(crate) fn present(request_id: u64, max_duration: Duration) -> Result<(), DeviceError> {
    let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if session.is_some() {
        return Err(DeviceError::Internal("mic_record busy"));
    }
    let (stop_tx, stop_rx) = mpsc::channel();
    *session = Some(Session {
        request_id,
        stop_tx,
    });
    drop(session);

    let spawn_result = std::thread::Builder::new().spawn(move || {
        let recorder = WindowsMicRecorder;
        let result = match recorder.start(max_duration) {
            Ok(()) => {
                bridge::mark_started(request_id);
                // 停止合図（`Ok`）・期限到来（`Timeout`）・送信側 drop（`Disconnected`、
                // 通常は起きないが防御的に扱う）のいずれでも停止へ進む。
                let _ = stop_rx.recv_timeout(max_duration);
                recorder.stop()
            }
            Err(e) => Err(e),
        };

        // `bridge::deliver` より前に `SESSION` を解除する（自分の request_id と
        // 一致するときだけ、後続要求を消さない、モジュール doc「並行性・世代」
        // 参照）。この時点で OS 側の録音は既に完全に停止済み（`recorder.stop()`
        // 完了後）のため解除して安全。順序を逆にすると、UI スレッドが
        // `bridge::deliver` の結果を取り込んで即座に次の録音を開始したとき、
        // まだ `SESSION` が `Some` のままで `present` が誤って busy を返す
        // （PR #960 レビュー指摘、codex-review P1・device-bridge-contracts.md
        // 「並行性・世代」節）。
        let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        if session.as_ref().is_some_and(|s| s.request_id == request_id) {
            *session = None;
        }
        drop(session);

        bridge::deliver(request_id, result);
    });

    // OS のスレッド生成自体が失敗した場合（プロセスのスレッド上限・メモリ上限に
    // 達した状態での録音開始）、`std::thread::spawn` は panic するため
    // `std::thread::Builder::spawn` の `Result` を必ず処理する。ここで積んだ
    // `SESSION` はワーカーが存在しない以上誰も解除しないため、この場で
    // （ワーカー側と同じ「自分の request_id のときだけ解除」の作法で）解除する
    // （PR #960 レビュー指摘、codex-review P1）。
    if spawn_result.is_err() {
        let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        if session.as_ref().is_some_and(|s| s.request_id == request_id) {
            *session = None;
        }
        drop(session);
        return Err(DeviceError::Internal("mic_record present"));
    }

    Ok(())
}

/// `device/mic_record.rs` の「停止」ボタンハンドラから呼ばれる。`request_id` が
/// 現在進行中のセッションと一致するときだけ停止合図を送る。不一致・未登録は
/// `false`（無視。自動停止 or 既存の結果配送が追随する、`apple_mic_record.rs::
/// request_stop` と同じ位置づけ）。
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
    fn stop_without_start_is_rejected() {
        let recorder = WindowsMicRecorder;
        let err = recorder.stop().unwrap_err();
        assert!(matches!(
            err,
            DeviceError::Internal("mic_record not recording")
        ));
    }

    #[test]
    fn request_stop_without_session_returns_false() {
        assert!(!request_stop(1));
    }
}
