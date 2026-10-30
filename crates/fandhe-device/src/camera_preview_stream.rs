//! Phase 11 の拡充ループ（親 #168、Issue #812）: カメラのライブ映像・フレーム取得（ズーム操作を含む）の
//! 契約。`fandhe-egui-proxy` の `device/camera_preview_stream.rs`（effect ドライバ）が
//! [`CameraPreview::start`] で映像を開始し、各 OS 実装（iOS: #813、Android: #814、macOS: #1066。macOS は iOS と同じ `.m` を共有する）が本トレイトを実装する。
//! Web は `fandhe-web-dom`（#815）が `getUserMedia` を使い、本トレイトは経由せず、純関数
//! （[`preview_size`]・[`downscale_packed`]・[`zoom_target`]・[`checked_zoom`]）だけを共用する。
//! Windows は #1102（WinRT `MediaCapture` + `MediaFrameReader`）で、写像の純関数
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
//! - ズーム: `VideoDeviceController.ZoomControl` を使う。`Supported` が false のカメラ（PC の Web
//!   カメラに多い）への要求は、同期ではなく非同期の [`PreviewEvent::ZoomFailed`]
//!   （[`ZoomFailure::Unsupported`]）で返す。`Configure` の失敗・範囲不正は [`ZoomFailure::Failed`]
//! - 背景: ウィンドウが最小化・背面でも映像を続け、[`StopReason::Backgrounded`] を送らない
//! - 中断: `CameraStreamState::BlockedForPrivacy` が [`PreviewEvent::Interrupted`]、`Streaming` への
//!   復帰が [`PreviewEvent::Running`]。`Shutdown` と `MediaCapture.Failed` は [`PreviewEvent::RuntimeFailed`]
//! - 開始失敗: `InitializeWithSettingsAsync` の HRESULT は [`start_failure_from_windows_hresult`]、
//!   `MediaFrameReader.StartAsync` の status は [`start_failure_from_windows_reader_status`] で写す。
//!   開始の確定は status が `Success` のとき
//! - 解放: 解放呼び出し（`Remove*` / `StopAsync` / `Close`）の失敗は握りつぶさず、次の開始の前に
//!   再試行する。再試行も失敗したら新しい開始を `StartFailed(Other)` にする（嘘の成功にしない）
//! - 守らない範囲: 初期化は STA 指定の公式文書に対し MTA のワーカーで呼ぶ（実機で確認する）。
//!   初期化の途中に `Close` したときの挙動、1080p 以上の解像度でのワーカーの負荷
//!
//! # 後続 OS の要点
//!
//! OS 資源は生ポインタではなく `id`（ドライバの登録表が発行）で持つ。停止は「登録解除 → OS 停止 →
//! 実行中コールバックの終了待ち → 解放」の順で、ロックの内側では待たない。Cocoa 呼び出しは
//! `@autoreleasepool` の内側で行う。権限ダイアログの応答と背景遷移の到着順が入れ替わっても、
//! 正常な許可を中断扱いにしない。イベントコードは [`event_from_code`] の表を `.m` と JNI の両方で使う。
//!
//! 画素はプライバシー情報なので、ログ・`Debug` に出さない。

use std::fmt;
use std::sync::Arc;

/// 画面へ渡す縮小後フレームの長辺の上限（ピクセル）。
pub const PREVIEW_MAX_SIDE: u32 = 320;

/// OS バッファの寸法の上限（ピクセル）。巨大確保による DoS を防ぐ仮置き値で、実機確認で見直す。
pub const MAX_SOURCE_SIDE: u32 = 8192;

/// 元寸法 `width` x `height` を縦横比を保って長辺 [`PREVIEW_MAX_SIDE`] 以下へ縮めた寸法。
/// すでに収まる寸法はそのまま返す（冪等）。0 を含む入力は `(0, 0)`。
pub fn preview_size(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (0, 0);
    }
    let long = width.max(height);
    if long <= PREVIEW_MAX_SIDE {
        return (width, height);
    }
    let scale = |v: u32| ((v as u64 * PREVIEW_MAX_SIDE as u64 / long as u64) as u32).max(1);
    (scale(width), scale(height))
}

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

// ---- フレーム -------------------------------------------------------------------------------

/// 縮小済みの RGBA フレーム（行優先・パディングなし。`rgba.len() == width * height * 4`）。
#[derive(Clone, PartialEq, Eq)]
pub struct ScaledFrame {
    /// 幅（ピクセル）。
    pub width: u32,
    /// 高さ（ピクセル）。
    pub height: u32,
    /// 画素。
    pub rgba: Vec<u8>,
}

impl fmt::Debug for ScaledFrame {
    /// 画素はプライバシー情報なので寸法と長さだけを出す。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScaledFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rgba_len", &self.rgba.len())
            .finish()
    }
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

/// 入力 RGBA ピクセルの取得関数でブロック平均縮小し、`ScaledFrame` を作る。
fn downscale_with(
    width: usize,
    height: usize,
    px: impl Fn(usize, usize) -> [u8; 4],
) -> ScaledFrame {
    let (ow, oh) = preview_size(width as u32, height as u32);
    let (ow, oh) = (ow as usize, oh as usize);
    let mut out = Vec::with_capacity(ow * oh * 4);
    for oy in 0..oh {
        let y0 = oy * height / oh;
        let y1 = ((oy + 1) * height / oh).max(y0 + 1).min(height);
        for ox in 0..ow {
            let x0 = ox * width / ow;
            let x1 = ((ox + 1) * width / ow).max(x0 + 1).min(width);
            let mut sum = [0u64; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = px(x, y);
                    for (s, v) in sum.iter_mut().zip(p) {
                        *s += v as u64;
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u64;
            out.extend(sum.iter().map(|s| (s / count) as u8));
        }
    }
    ScaledFrame {
        width: ow as u32,
        height: oh as u32,
        rgba: out,
    }
}

fn dims_ok(width: u32, height: u32) -> bool {
    (1..=MAX_SOURCE_SIDE).contains(&width) && (1..=MAX_SOURCE_SIDE).contains(&height)
}

/// 画素の並び（1 画素 4 バイト）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackedLayout {
    /// R, G, B, A（Web の `ImageData` 等）。
    Rgba8,
    /// B, G, R, A（iOS の `kCVPixelFormatType_32BGRA`）。
    Bgra8,
}

/// 行ごとのパディング付き 4 バイト画素の画像を縮め、RGBA へ並べ替える。
///
/// `row_stride >= width * 4` と `data.len() >= row_stride * (height - 1) + width * 4` を
/// `checked_*` で検査し、寸法が 0 または [`MAX_SOURCE_SIDE`] 超なら `None`（panic を契約にしない）。
pub fn downscale_packed(
    width: u32,
    height: u32,
    row_stride: usize,
    layout: PackedLayout,
    data: &[u8],
) -> Option<ScaledFrame> {
    if !dims_ok(width, height) {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    let row_bytes = w.checked_mul(4)?;
    if row_stride < row_bytes {
        return None;
    }
    let needed = row_stride.checked_mul(h - 1)?.checked_add(row_bytes)?;
    if data.len() < needed {
        return None;
    }
    Some(downscale_with(w, h, |x, y| {
        let i = y * row_stride + x * 4;
        match layout {
            PackedLayout::Rgba8 => [data[i], data[i + 1], data[i + 2], data[i + 3]],
            PackedLayout::Bgra8 => [data[i + 2], data[i + 1], data[i], data[i + 3]],
        }
    }))
}

/// `YUV_420_888`（Android の `ImageReader`）を BT.601 full range（JFIF）で RGBA へ変換しながら縮める。
///
/// 色差面は縦横 1/2（切り上げ）。各面の stride・長さを `checked_*` で検査し、不正なら `None`。
#[allow(clippy::too_many_arguments)]
pub fn downscale_yuv420(
    width: u32,
    height: u32,
    y: &[u8],
    y_row_stride: usize,
    u: &[u8],
    v: &[u8],
    uv_row_stride: usize,
    uv_pixel_stride: usize,
) -> Option<ScaledFrame> {
    if !dims_ok(width, height) || uv_pixel_stride == 0 {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    if y_row_stride < w || y.len() < y_row_stride.checked_mul(h - 1)?.checked_add(w)? {
        return None;
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let row_span = (cw - 1).checked_mul(uv_pixel_stride)?.checked_add(1)?;
    if uv_row_stride < row_span {
        return None;
    }
    let chroma_needed = uv_row_stride.checked_mul(ch - 1)?.checked_add(row_span)?;
    if u.len() < chroma_needed || v.len() < chroma_needed {
        return None;
    }
    Some(downscale_with(w, h, |x, yy| {
        let ci = (yy / 2) * uv_row_stride + (x / 2) * uv_pixel_stride;
        let luma = y[yy * y_row_stride + x] as i32;
        let (cb, cr) = (u[ci] as i32 - 128, v[ci] as i32 - 128);
        let r = luma + ((359 * cr) >> 8);
        let g = luma - ((88 * cb + 183 * cr) >> 8);
        let b = luma + ((454 * cb) >> 8);
        let c = |v: i32| v.clamp(0, 255) as u8;
        [c(r), c(g), c(b), 255]
    }))
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
    if !dims_ok(w, h) {
        return None;
    }
    w.checked_mul(h)?.checked_mul(4)
}

// ---- トレイト -------------------------------------------------------------------------------

/// OS からのフレーム・通知の受け口。任意のスレッドから呼ばれうる。
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
    fn preview_size_keeps_aspect_and_is_idempotent() {
        assert_eq!(preview_size(640, 480), (320, 240));
        assert_eq!(preview_size(480, 640), (240, 320));
        assert_eq!(preview_size(100, 50), (100, 50));
        let (w, h) = preview_size(4000, 3000);
        assert_eq!(preview_size(w, h), (w, h));
        assert_eq!(preview_size(0, 10), (0, 0));
        assert_eq!(preview_size(10, 0), (0, 0));
        assert_eq!(preview_size(10000, 1), (320, 1));
    }

    #[test]
    fn packed_bgra_swaps_and_skips_padding() {
        // 2x1、stride 12（4 バイトのパディング）。
        let data = [10, 20, 30, 255, 40, 50, 60, 255, 9, 9, 9, 9];
        let f = downscale_packed(2, 1, 12, PackedLayout::Bgra8, &data).unwrap();
        assert_eq!((f.width, f.height), (2, 1));
        assert_eq!(f.rgba, vec![30, 20, 10, 255, 60, 50, 40, 255]);
        let f = downscale_packed(2, 1, 12, PackedLayout::Rgba8, &data).unwrap();
        assert_eq!(&f.rgba[..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn packed_downscale_averages_blocks() {
        let mut data = Vec::new();
        for _ in 0..(640 * 480) {
            data.extend_from_slice(&[100, 100, 100, 255]);
        }
        let f = downscale_packed(640, 480, 640 * 4, PackedLayout::Rgba8, &data).unwrap();
        assert_eq!((f.width, f.height), (320, 240));
        assert_eq!(f.rgba.len(), 320 * 240 * 4);
        assert!(f.rgba.chunks(4).all(|p| p == [100, 100, 100, 255]));
    }

    #[test]
    fn packed_rejects_bad_input() {
        let data = vec![0u8; 16];
        assert!(downscale_packed(2, 2, 7, PackedLayout::Rgba8, &data).is_none());
        assert!(downscale_packed(2, 2, 8, PackedLayout::Rgba8, &data[..15]).is_none());
        assert!(downscale_packed(0, 2, 8, PackedLayout::Rgba8, &data).is_none());
        assert!(
            downscale_packed(
                MAX_SOURCE_SIDE + 1,
                1,
                usize::MAX,
                PackedLayout::Rgba8,
                &data
            )
            .is_none()
        );
        assert!(downscale_packed(2, 2, usize::MAX, PackedLayout::Rgba8, &data).is_none());
    }

    #[test]
    fn yuv_known_colors() {
        // 2x2。白・黒・赤付近を 1 画素ずつ別の呼び出しで確かめる（色差は 2x2 で共有）。
        let conv = |yv: u8, uv: u8, vv: u8| {
            let f = downscale_yuv420(2, 2, &[yv; 4], 2, &[uv], &[vv], 1, 1).unwrap();
            [f.rgba[0], f.rgba[1], f.rgba[2], f.rgba[3]]
        };
        assert_eq!(conv(255, 128, 128), [255, 255, 255, 255]);
        assert_eq!(conv(0, 128, 128), [0, 0, 0, 255]);
        let red = conv(76, 85, 255);
        assert!(red[0] > 240 && red[1] < 10 && red[2] < 10, "{red:?}");
    }

    #[test]
    fn yuv_respects_pixel_stride_and_rejects_bad_input() {
        // 色差が interleaved（pixel_stride 2）の 2x2。
        let y = [128u8; 4];
        let uv = [128u8, 0, 128, 0];
        assert!(downscale_yuv420(2, 2, &y, 2, &uv, &uv, 2, 2).is_some());
        assert!(downscale_yuv420(2, 2, &y, 1, &uv, &uv, 2, 2).is_none());
        assert!(downscale_yuv420(2, 2, &y[..3], 2, &uv, &uv, 2, 2).is_none());
        assert!(downscale_yuv420(2, 2, &y, 2, &uv[..0], &uv, 2, 2).is_none());
        assert!(downscale_yuv420(2, 2, &y, 2, &uv, &uv, 2, 0).is_none());
        // 4x2 で色差の行幅（3）が row_stride（2）に収まらない。
        let big = [128u8; 16];
        assert!(downscale_yuv420(4, 2, &[0u8; 8], 4, &big, &big, 2, 2).is_none());
        assert!(downscale_yuv420(0, 2, &y, 2, &uv, &uv, 2, 2).is_none());
    }

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
