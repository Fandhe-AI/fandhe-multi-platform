//! drag-drop（ファイルのドラッグ & ドロップ受け取り）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1302）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`drag_drop_effect`）。契約（読み取りの検証・上限）は
//! `fandhe_device::drag_drop` に残る（file-association が未移行で共有するため。Web 実装本体も `fandhe-web-dom`）。
//! デスクトップ（macOS・Windows）はウィンドウへのドロップを `fandhe_runtime::HostView::file_drag` 越しに観測し
//! （egui を知らない）、iOS（`UIDropInteraction`、`ios/fandhe_ios_drag_drop.m`）と Android（DecorView の
//! `View.OnDragListener`、`DragDropHelper` を `DragDropLifecycle` が束ねる）は OS のコールバックを集約器
//! （`drop_session`、セッション id が世代を兼ねる）へ push する。受け取り側のみで、送り側は範囲外。
//! 守る範囲は 1 プロセス内のセッション順序・画面外イベントの破棄・読み取りの上限（件数・サイズ・スレッド数）・
//! 失敗を成功と見せないこと。守らない範囲はドロップ処理中のプロセス死・複数ウィンドウ・OS 側のドロップ許可の差。
//!
//! 契約・effect ドライバ・OS 実装を 1 つに集める。画面の状態型を参照してよいのは `driver` だけ。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(target_os = "android")]
mod android;
mod driver;
pub mod drop_session;
#[cfg(target_os = "ios")]
mod ios;

#[cfg(target_os = "android")]
pub use android::{reset_sessions, set_screen_active};
pub use driver::*;
// 契約（読み取りの検証・上限）の正本は fandhe-device に残し、利用者が直接依存しなくて済むよう再公開する。
pub use fandhe_device::drag_drop::{
    DropError, MAX_DROP_FILES, MAX_FILE_BYTES, PickedFile, read_dropped, read_dropped_from,
};

/// 画面の状態型に依存しない入口（iOS・Android）。OS が積んだドロップの出来事を取り出す。
///
/// 受け取り側は [`drop_session::DropEvent`] を順に適用し、ファイルの読み取りは OS 実装が済ませた
/// [`drop_session::DroppedItem`] を使う。デスクトップ（macOS・Windows）は OS 実装が無く常に空。
/// ウィンドウへのドロップは `fandhe_runtime::HostView::file_drag` と [`read_dropped`] で受け取る。
/// iOS は取り付け、iOS・Android は受け付けの有効化を呼び出しごとに行うため、毎フレーム呼ぶこと
/// （有効期間は 500ms で、呼び出しが止まると受け付けを止める）。
/// iOS で取り付けに失敗したら [`DropSetupError`] を返す（呼び出し側が利用者へ見せる。取り付けは間隔を置いて再試行される）。
/// 画面ドライバ（[`drag_drop_effect`]）と同時に使わない（出来事を取り合う）。
pub fn platform_drag_drop() -> Result<Vec<drop_session::DropEvent>, DropSetupError> {
    #[cfg(target_os = "ios")]
    {
        ios::mark_active();
        // 再試行待ちの間は ensure_installed が None を返すため、保持した失敗で装着成功まで Err を返し続ける。
        ios::ensure_installed();
        if ios::install_failed() {
            return Err(DropSetupError);
        }
        Ok(drop_session::take_events())
    }
    #[cfg(target_os = "android")]
    {
        android::mark_active();
        Ok(drop_session::take_events())
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        Ok(Vec::new())
    }
}

/// [`platform_drag_drop`] の受け取り準備（iOS の `UIDropInteraction` 取り付け）に失敗した。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropSetupError;

impl std::fmt::Display for DropSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ドロップの受け取りを準備できませんでした")
    }
}

impl std::error::Error for DropSetupError {}
