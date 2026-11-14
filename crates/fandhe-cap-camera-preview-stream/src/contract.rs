//! Phase 11 の拡充ループ（親 #168、Issue #812）: カメラのライブ映像・フレーム取得（ズーム操作を含む）の
//! 契約。本 crate の `driver`（effect ドライバ）が
//! [`CameraPreview::start`] で映像を開始し、各 OS 実装（iOS: #813、Android: #814、macOS: #1066。macOS は iOS と同じ `.m` を共有する）が本トレイトを実装する。
//! Web は `fandhe-web-dom`（#815）が `getUserMedia` を使い、本トレイトは経由せず、純関数
//! （[`preview_size`]・[`downscale_packed`]・[`zoom_target`]・[`checked_zoom`]）だけを共用する。
//! Windows は #1102（WinRT `MediaCapture` + `MediaFrameReader`）。写像の純関数
//! （[`start_failure_from_windows_hresult`] 等）だけを本モジュールに置く。
//!
//! # 結果の種類（契約表 10）
//!
//! 開始待ち / 映像中（OS の開始通知で確定）/ 中断中（OS が知らせた中断。OS が再開すれば映像中へ戻る）/
//! 停止（利用者の操作・画面を離れた・背景へ移った。いずれも正常で失敗にしない）/
//! 開始失敗（[`StartFailure`]）/ 実行中の失敗（[`PreviewEvent::RuntimeFailed`]）/ 非対応。
//! 失敗・中断を映像中に畳まない。
//!
//! # OS を正にする
//!
//! - 開始・中断・停止・失敗は OS の通知で確定する。期限のタイマーは張らない（開始待ちは「止める」で
//!   取り消せるので、推測のタイマーを置かない。契約表 3）
//! - フレームは OS から届いた時点で受け取り、最新 1 枚だけを残す
//! - 表示するズーム値は、フレームと一緒に OS から読んだ値（[`PreviewFrame::zoom`]）にする
//! - 事前判定をしない。カメラの有無・許可・使用中かは、開く操作の結果をそのまま写す
//!
//! # 解放
//!
//! 止める・画面を離れる（プロセス終了 = `shutdown`）・背景へ移る・開始途中の失敗・実行中の失敗の全経路で
//! 解放する。[`PreviewSession`] の drop が停止と解放で、戻った後はコールバックが走らない。解放後に遅れて
//! 届いたフレーム・通知はドライバの登録表で捨てる。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の順序（開始・停止・ズーム・受信の処理は UI スレッドの 1 本の effect に集約し、
//! OS コールバックが触るのは受信口と wake だけ）、嘘の成功の禁止（失敗・中断・非対応を成功へ畳まない）、
//! 資源の解放。
//! 守らない範囲: フレームの間隔・時刻、前面・背面の切り替え、露出・焦点・ライト、複数カメラ・複数画面、
//! OS が止めた後の自動再開（iOS の中断終了を除く）、端末の向きの追従、OS 側の非同期確定との時間差による
//! 一時的な表示の食い違い。
//!
//! # macOS の違い
//!
//! - ズーム: OS に API が無い（`videoZoomFactor` は macOS で使えない）。[`PreviewSession::set_zoom`] は
//!   OS を呼ばず同期で `Err(ZoomFailure::Unsupported)` を返し、[`PreviewFrame::zoom`] は常に `None`
//! - 背景: ウィンドウが背面・最小化でも映像を続け、[`StopReason::Backgrounded`] を送らない
//! - 中断: 中断の理由が取れないため、OS の中断はすべて [`PreviewEvent::Interrupted`]。中断の終了で
//!   [`PreviewEvent::Running`] へ戻る
//! - カメラの取り外し（`AVCaptureDeviceWasDisconnectedNotification`）は [`PreviewEvent::RuntimeFailed`]
//!
//! # Windows の違い
//!
//! 実装済み（#1102）。受付は専用スレッド 1 本の状態機械で、
//! そのスレッド上で OS の非同期操作の完了を待たない（完了は `SetCompleted` から積む。同期の `Close` は除く）。
//!
//! - 開始手順の途中の停止は、保留中の OS 操作を `Cancel` して即座に戻る。drop はワーカーが OS の完了を待たずに済ませる（同期の `Close` はワーカー上で `Stop` の ack の前に走る）
//! - `Cancel` の戻り値は見ない。完了（取り消しを含む）は `SetCompleted` で必ず 1 回届くので、それを受けてから
//!   `MediaFrameReader` → `MediaCapture` の順に `Close` する。取り消しと競合して届いた reader もここで閉じる
//! - 解放呼び出し（`Remove*`・`StopAsync`・`Close`）の失敗は未解放として持ち越す。次の開始の前に再試行し、
//!   それも失敗したら開始失敗（[`StartFailure::Other`]）にする
//! - 前の解放が OS の完了待ちの間に来た開始は、完了まで開始待ちにする
//! - 実行中の失敗は `MediaCapture.Failed`、`CameraStreamState` の `Shutdown`、状態の取得失敗で確定する
//! - ズーム制御は、取得の失敗を [`ZoomFailure::Failed`]、`Supported == false` を [`ZoomFailure::Unsupported`] にする
//! - 個々のフレームの取得・変換・コピーの失敗は、そのフレームを捨てて続ける。フレームの間隔の欠けとして扱い
//!   （iOS・Android と同じ）、件数の閾値は置かない
//! - フレーム単位の資源（`MediaFrameReference`・`SoftwareBitmap`）の `Close` 失敗は個別に再試行せず、
//!   実行中の失敗（[`PreviewEvent::RuntimeFailed`]）として session ごと解放する
//!
//! Windows で守らない範囲: 開始途中の停止から OS の取り消しが完了するまでの時間差、MTA での初期化の可否
//! （#808 の実機確認しだい）、1080p 以上での負荷。
//!
//! # 後続 OS の要点
//!
//! OS 資源は生ポインタではなく `id`（ドライバの登録表が発行）で持つ。停止は「登録解除 → OS 停止 →
//! 実行中コールバックの終了待ち → 解放」の順で、ロックの内側では待たない。Cocoa 呼び出しは
//! `@autoreleasepool` の内側で行う。権限ダイアログの応答と背景遷移の到着順が入れ替わっても、
//! 正常な許可を中断扱いにしない。イベントコードは [`event_from_code`] の表を `.m` と JNI の両方で使う。
//!
//! 画素はプライバシー情報なので、ログ・`Debug` に出さない。

use std::sync::Arc;

// 縮小の部品は Web の `fandhe-web-dom` と `fandhe-cap-ar` も使うため `fandhe-device` に残してある
// （consumer-app-structure.md §8）。機能 crate は依存先を直接公開せず、ここから再公開する。
pub use fandhe_device::camera_preview_stream::{
    MAX_SOURCE_SIDE, PREVIEW_MAX_SIDE, PackedLayout, ScaledFrame, downscale_packed,
    downscale_yuv420, preview_size,
};

// ---- ズーム ---------------------------------------------------------------------------------

/// ズームの段（画面のボタンに対応）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomStep {
    /// 1 倍。
    One,
    /// 2 倍。
    Two,
}

impl ZoomStep {
    /// 目標の倍率。
    pub fn factor(self) -> f32 {
        match self {
            ZoomStep::One => 1.0,
            ZoomStep::Two => 2.0,
        }
    }
}

/// ズームの失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomFailure {
    /// この環境・カメラではズームできない。
    Unsupported,
    /// OS が値を拒んだ・範囲が不正。
    Failed,
}

/// OS が示すズーム範囲 `range`（`(最小, 最大)`）に収まる目標倍率を返す。
/// 範囲が無ければ `Unsupported`、非有限・`min <= 0`・`min > max` は `Failed`
/// （検証は丸めの前に行う。契約表 5）。2 倍は `max` を超えない。
pub fn zoom_target(step: ZoomStep, range: Option<(f32, f32)>) -> Result<f32, ZoomFailure> {
    let (min, max) = range.ok_or(ZoomFailure::Unsupported)?;
    if !min.is_finite() || !max.is_finite() || min <= 0.0 || min > max {
        return Err(ZoomFailure::Failed);
    }
    Ok(step.factor().clamp(min, max))
}

/// OS から読んだズーム値の検証。有限かつ正のときだけ `Some`。
pub fn checked_zoom(raw: f32) -> Option<f32> {
    (raw.is_finite() && raw > 0.0).then_some(raw)
}

/// OS から届いた 1 フレーム。OS 側（または各 OS 実装）が縮小済みで渡す。
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewFrame {
    /// 元の幅（縮小前、ピクセル）。
    pub source_width: u32,
    /// 元の高さ（縮小前、ピクセル）。
    pub source_height: u32,
    /// 縮小後の画素（長辺 [`PREVIEW_MAX_SIDE`] 以下）。
    pub frame: ScaledFrame,
    /// フレームと一緒に OS から読んだズーム値。読めなければ `None`。
    pub zoom: Option<f32>,
}

// ---- イベント -------------------------------------------------------------------------------

/// 停止の理由（利用者の停止は [`PreviewSession`] の drop で表し、イベントにしない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// アプリが背景へ移った。
    Backgrounded,
    /// 画面を離れた（Web の `pagehide` 等）。
    LeftScreen,
}

/// 開始の失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartFailure {
    /// カメラの権限が無い・拒否された。
    PermissionDenied,
    /// 使えるカメラが見つからない。
    NoCamera,
    /// 他で使用中。
    InUse,
    /// 上記以外。
    Other,
}

/// OS から届く映像の状態変化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewEvent {
    /// 映像が始まった（OS の開始通知）。中断からの再開もこれ。
    Running,
    /// OS が一時的に止めた。OS が再開すれば [`PreviewEvent::Running`] が届く。
    Interrupted,
    /// 正常な停止（終端）。
    Stopped(StopReason),
    /// 開始に失敗した（終端）。
    StartFailed(StartFailure),
    /// 実行中に失敗した（終端）。
    RuntimeFailed,
    /// 非同期のズームが失敗した。
    ZoomFailed(ZoomFailure),
}

impl PreviewEvent {
    /// 以後フレームが届かなくなる終端イベントか。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            PreviewEvent::Stopped(_) | PreviewEvent::StartFailed(_) | PreviewEvent::RuntimeFailed
        )
    }
}

/// `.m`（iOS・macOS）と JNI（Android）が共有するイベントコードの写像（契約表 5: 数値表は 1 か所）。
///
/// | code | 意味 | detail |
/// | --- | --- | --- |
/// | 0 | Running | - |
/// | 1 | Interrupted | - |
/// | 2 | Stopped(Backgrounded) | - |
/// | 3 | Stopped(LeftScreen) | - |
/// | 4 | StartFailed | 0 権限 / 1 カメラなし / 2 使用中 / 他 その他 |
/// | 5 | RuntimeFailed | - |
/// | 6 | ZoomFailed | 0 非対応 / 他 失敗 |
///
/// 未知のコードは `RuntimeFailed`（成功へは倒さない）。
pub fn event_from_code(code: i32, detail: i32) -> PreviewEvent {
    match code {
        0 => PreviewEvent::Running,
        1 => PreviewEvent::Interrupted,
        2 => PreviewEvent::Stopped(StopReason::Backgrounded),
        3 => PreviewEvent::Stopped(StopReason::LeftScreen),
        4 => PreviewEvent::StartFailed(match detail {
            0 => StartFailure::PermissionDenied,
            1 => StartFailure::NoCamera,
            2 => StartFailure::InUse,
            _ => StartFailure::Other,
        }),
        6 => PreviewEvent::ZoomFailed(match detail {
            0 => ZoomFailure::Unsupported,
            _ => ZoomFailure::Failed,
        }),
        _ => PreviewEvent::RuntimeFailed,
    }
}

// ---- Windows の写像（Issue #1102）---------------------------------------------------------

/// `InitializeWithSettingsAsync` の HRESULT を開始失敗へ写す（Windows 実装用。数値表は 1 か所）。
///
/// | HRESULT | 意味 | 写像 |
/// | --- | --- | --- |
/// | `0x80070005` | `E_ACCESSDENIED`（プライバシー設定で拒否） | `PermissionDenied` |
/// | `0xC00DABE0` | `MF_E_NO_CAPTURE_DEVICES_AVAILABLE` | `NoCamera` |
/// | 他 | 意味を確かめていない値 | `Other` |
pub fn start_failure_from_windows_hresult(code: i32) -> StartFailure {
    match code as u32 {
        0x8007_0005 => StartFailure::PermissionDenied,
        0xC00D_ABE0 => StartFailure::NoCamera,
        _ => StartFailure::Other,
    }
}

/// `MediaFrameReaderStartStatus` を開始失敗へ写す。`Success`（0）は `None`（開始の確定）。
/// 1 `UnknownFailure` / 2 `DeviceNotAvailable` / 3 `OutputFormatNotSupported` /
/// 4 `ExclusiveControlNotAvailable`。未知の値は `Other`（成功へは倒さない）。
pub fn start_failure_from_windows_reader_status(status: i32) -> Option<StartFailure> {
    match status {
        0 => None,
        2 => Some(StartFailure::NoCamera),
        4 => Some(StartFailure::InUse),
        _ => Some(StartFailure::Other),
    }
}

/// `CameraStreamState` を映像の状態変化へ写す。`None` は何もしない（0 `NotStreaming` は自分の停止でも
/// 届く、4 `Throttled` は映像が続く）。1 `Streaming` は `Running`（呼び出し側が開始確定後の復帰
/// だけを通す）、2 `BlockedForPrivacy` は `Interrupted`、3 `Shutdown` と未知の値は `RuntimeFailed`。
pub fn event_from_windows_stream_state(state: i32) -> Option<PreviewEvent> {
    match state {
        0 | 4 => None,
        1 => Some(PreviewEvent::Running),
        2 => Some(PreviewEvent::Interrupted),
        _ => Some(PreviewEvent::RuntimeFailed),
    }
}

/// OS が返した寸法 `width` x `height` の Bgra8 バッファ長（バイト）。0 以下・[`MAX_SOURCE_SIDE`] 超・
/// あふれは `None`。`Buffer::Create` の前に検証する（巨大確保による DoS を防ぐ。契約表 5）。
pub fn windows_bgra_len(width: i32, height: i32) -> Option<u32> {
    let (w, h) = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
    if !(1..=MAX_SOURCE_SIDE).contains(&w) || !(1..=MAX_SOURCE_SIDE).contains(&h) {
        return None;
    }
    w.checked_mul(h)?.checked_mul(4)
}

// ---- トレイト -------------------------------------------------------------------------------

/// OS からのフレーム・通知の受け口。任意のスレッドから呼ばれうる。
///
/// 受け口の中で session を drop しない（受信口に書いて wake するだけ。drop は停止と解放を行うため、
/// コールバックの中で呼ぶと自分自身の終了待ちになりうる）。
pub trait PreviewSink: Send + Sync {
    /// フレームを渡す。呼び出し側が縮小済み（長辺 [`PREVIEW_MAX_SIDE`] 以下）であること。
    fn frame(&self, frame: PreviewFrame);
    /// 状態変化を渡す。
    fn event(&self, event: PreviewEvent);
}

/// 開始した映像 1 件。drop で停止・解放し、戻った後はコールバックが走らない。
pub trait PreviewSession: Send {
    /// ズームを変える。UI スレッドから呼ばれ、すぐ戻る。非同期の失敗は
    /// [`PreviewEvent::ZoomFailed`] で返す。
    fn set_zoom(&self, step: ZoomStep) -> Result<(), ZoomFailure>;
}

/// 開始の失敗（同期で分かるもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewStartError {
    /// この環境では使えない。
    Unsupported,
    /// 開始できなかった。
    Failed(StartFailure),
}

/// カメラの映像を受け取る端末機能。
pub trait CameraPreview: Send + Sync {
    /// 映像を始める。許可待ち・open の途中でもすぐ session を返し、その drop で取り消せる。
    /// 同期で分かる失敗は `Err` で返し、そのとき OS 資源は残さない。
    fn start(
        &self,
        sink: Arc<dyn PreviewSink>,
    ) -> Result<Box<dyn PreviewSession>, PreviewStartError>;
}

/// 非対応環境用。常に `Err(Unsupported)`（ダミーの成功実装へは倒さない）。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedCameraPreview;

impl CameraPreview for UnsupportedCameraPreview {
    fn start(
        &self,
        _sink: Arc<dyn PreviewSink>,
    ) -> Result<Box<dyn PreviewSession>, PreviewStartError> {
        Err(PreviewStartError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_target_rules() {
        assert_eq!(
            zoom_target(ZoomStep::One, None),
            Err(ZoomFailure::Unsupported)
        );
        assert_eq!(zoom_target(ZoomStep::Two, Some((1.0, 5.0))), Ok(2.0));
        assert_eq!(zoom_target(ZoomStep::Two, Some((1.0, 1.5))), Ok(1.5));
        assert_eq!(zoom_target(ZoomStep::One, Some((1.5, 4.0))), Ok(1.5));
        for bad in [
            (f32::NAN, 2.0),
            (1.0, f32::INFINITY),
            (3.0, 2.0),
            (0.0, 2.0),
            (-1.0, 2.0),
        ] {
            assert_eq!(
                zoom_target(ZoomStep::One, Some(bad)),
                Err(ZoomFailure::Failed),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn checked_zoom_rejects_invalid() {
        assert_eq!(checked_zoom(2.0), Some(2.0));
        for bad in [f32::NAN, 0.0, -1.0, f32::INFINITY] {
            assert_eq!(checked_zoom(bad), None);
        }
    }

    #[test]
    fn event_codes_never_fold_to_success() {
        assert_eq!(event_from_code(0, 0), PreviewEvent::Running);
        assert_eq!(event_from_code(1, 0), PreviewEvent::Interrupted);
        assert_eq!(
            event_from_code(2, 0),
            PreviewEvent::Stopped(StopReason::Backgrounded)
        );
        assert_eq!(
            event_from_code(3, 0),
            PreviewEvent::Stopped(StopReason::LeftScreen)
        );
        assert_eq!(
            event_from_code(4, 0),
            PreviewEvent::StartFailed(StartFailure::PermissionDenied)
        );
        assert_eq!(
            event_from_code(4, 1),
            PreviewEvent::StartFailed(StartFailure::NoCamera)
        );
        assert_eq!(
            event_from_code(4, 2),
            PreviewEvent::StartFailed(StartFailure::InUse)
        );
        assert_eq!(
            event_from_code(4, 99),
            PreviewEvent::StartFailed(StartFailure::Other)
        );
        assert_eq!(
            event_from_code(6, 0),
            PreviewEvent::ZoomFailed(ZoomFailure::Unsupported)
        );
        assert_eq!(
            event_from_code(6, 1),
            PreviewEvent::ZoomFailed(ZoomFailure::Failed)
        );
        for code in [5, 7, -1, i32::MAX] {
            assert_eq!(
                event_from_code(code, 0),
                PreviewEvent::RuntimeFailed,
                "{code}"
            );
        }
    }

    #[test]
    fn windows_mappings_never_fold_to_success() {
        assert_eq!(
            start_failure_from_windows_hresult(0x8007_0005_u32 as i32),
            StartFailure::PermissionDenied
        );
        assert_eq!(
            start_failure_from_windows_hresult(0xC00D_ABE0_u32 as i32),
            StartFailure::NoCamera
        );
        assert_eq!(
            start_failure_from_windows_hresult(0x1234),
            StartFailure::Other
        );
        assert_eq!(start_failure_from_windows_reader_status(0), None);
        assert_eq!(
            start_failure_from_windows_reader_status(2),
            Some(StartFailure::NoCamera)
        );
        assert_eq!(
            start_failure_from_windows_reader_status(4),
            Some(StartFailure::InUse)
        );
        for s in [1, 3, 99, -1] {
            assert_eq!(
                start_failure_from_windows_reader_status(s),
                Some(StartFailure::Other),
                "{s}"
            );
        }
        assert_eq!(event_from_windows_stream_state(0), None);
        assert_eq!(event_from_windows_stream_state(4), None);
        assert_eq!(
            event_from_windows_stream_state(1),
            Some(PreviewEvent::Running)
        );
        assert_eq!(
            event_from_windows_stream_state(2),
            Some(PreviewEvent::Interrupted)
        );
        for s in [3, 5, -1] {
            assert_eq!(
                event_from_windows_stream_state(s),
                Some(PreviewEvent::RuntimeFailed),
                "{s}"
            );
        }
    }

    #[test]
    fn windows_bgra_len_bounds() {
        assert_eq!(windows_bgra_len(640, 480), Some(640 * 480 * 4));
        assert_eq!(windows_bgra_len(0, 480), None);
        assert_eq!(windows_bgra_len(640, -1), None);
        assert_eq!(windows_bgra_len(MAX_SOURCE_SIDE as i32 + 1, 1), None);
        assert!(windows_bgra_len(MAX_SOURCE_SIDE as i32, MAX_SOURCE_SIDE as i32).is_some());
    }

    #[test]
    fn terminal_events() {
        assert!(PreviewEvent::RuntimeFailed.is_terminal());
        assert!(PreviewEvent::StartFailed(StartFailure::Other).is_terminal());
        assert!(PreviewEvent::Stopped(StopReason::Backgrounded).is_terminal());
        assert!(!PreviewEvent::Running.is_terminal());
        assert!(!PreviewEvent::Interrupted.is_terminal());
        assert!(!PreviewEvent::ZoomFailed(ZoomFailure::Failed).is_terminal());
    }

    #[test]
    fn debug_hides_pixels() {
        let f = PreviewFrame {
            source_width: 4,
            source_height: 4,
            frame: ScaledFrame {
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 4],
            },
            zoom: None,
        };
        let s = format!("{f:?}");
        assert!(s.contains("rgba_len") && !s.contains("[1, 2, 3, 4]"), "{s}");
    }

    struct NopSink;
    impl PreviewSink for NopSink {
        fn frame(&self, _: PreviewFrame) {}
        fn event(&self, _: PreviewEvent) {}
    }

    #[test]
    fn unsupported_never_succeeds() {
        let r = UnsupportedCameraPreview.start(Arc::new(NopSink));
        assert!(matches!(r, Err(PreviewStartError::Unsupported)));
    }
}
