//! Windows 向け画面キャプチャの防止実装（Issue #582）。`device/capture_protection.rs` の Windows
//! 経路から呼ばれ、自プロセスのトップレベルウィンドウに `SetWindowDisplayAffinity`
//! （`WDA_EXCLUDEFROMCAPTURE`）を掛け / 外し、`GetWindowDisplayAffinity` で読み戻す
//! （`windows_share.rs` と同じく FFI・`unsafe` を持てる本クレート側から `windows` クレートを直接叩く）。
//!
//! `unsafe` は Set / Get の 2 呼び出しだけに閉じ、`// SAFETY:` を付ける。
//! 録画・スクリーンショットの検知は Windows に公開 API が無いので `support()` で非対応と返し、
//! `observe()` は全て `None`。防止のみの OS（対応表 `capture-protection` の Windows セル）。
//!
//! # 制約
//!
//! - UI スレッド（ウィンドウを作ったスレッド）から呼ぶ。effect は egui の update で同期実行される
//! - 対象は自プロセスのトップレベルウィンドウで、DWM が合成しているときだけ効く。DRM ではなく、
//!   写真での撮影は防げない
//! - Windows 10 2004 より前では `WDA_EXCLUDEFROMCAPTURE` が使えず黒塗りになる
//! - プロセス終了時の後片付けは OS がする（ウィンドウと一緒に属性が消える）
//! - OS のエラーコードをログへ出さない。`DeviceError::Internal` は固定ラベルのみ（security.md A09）

use fandhe_device::capture_protection::{CaptureObservation, CaptureProtection, CaptureSupport};
use fandhe_device::DeviceError;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowDisplayAffinity, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
};

use super::capture_protection::affinity_to_protected;

/// 状態を持たない実装（毎回 OS から読み戻す）。
pub(crate) struct WindowsCaptureProtection;

/// Windows の `CaptureProtection` 実装を返す。
pub(crate) fn implementation() -> std::sync::Arc<dyn CaptureProtection> {
    std::sync::Arc::new(WindowsCaptureProtection)
}

impl CaptureProtection for WindowsCaptureProtection {
    fn support(&self) -> CaptureSupport {
        CaptureSupport {
            prevent: true,
            detect_recording: false,
            detect_screenshot: false,
        }
    }

    #[allow(unsafe_code)]
    fn set_protected(&self, on: bool) -> Result<(), DeviceError> {
        let hwnd = super::windows_share::window()
            .ok_or(DeviceError::Internal("capture-protection window"))?;
        let affinity = if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
        // SAFETY: `hwnd` は `windows_share::window()`（`remember_window` が控えた HWND、
        // `ProxyApp` の生存中はウィンドウも生存する）を経由した有効なハンドルで、
        // ウィンドウを作った UI スレッドから呼ぶ（モジュール doc）。
        unsafe { SetWindowDisplayAffinity(hwnd, affinity) }
            .map_err(|_| DeviceError::Internal("capture-protection"))
    }

    #[allow(unsafe_code)]
    fn is_protected(&self) -> Option<bool> {
        let hwnd = super::windows_share::window()?;
        let mut raw: u32 = 0;
        // SAFETY: `hwnd` は有効なウィンドウハンドル（`set_protected` と同じ根拠）。出力先 `raw` は
        // この呼び出しの間だけ有効なスタック上の `u32` への可変参照。
        unsafe { GetWindowDisplayAffinity(hwnd, &mut raw) }.ok()?;
        affinity_to_protected(raw)
    }

    fn observe(&self) -> CaptureObservation {
        CaptureObservation::default()
    }
}
