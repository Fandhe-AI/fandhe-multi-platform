//! iOS 向け行動認識実装のスタブ（Issue #613、親 #124）。`device/activity_recognition.rs` の
//! iOS 経路が [`implementation`] を呼ぶ。dispatch・共通コア画面・ドライバ・trait 契約は #613 (a) で
//! 最終形を置いてあるため、実体を入れる (b-ios) #614 は本ファイルと iOS 側のネイティブコード
//! （`.m`・`build.rs`・Info.plist の文言）だけを埋め、dispatch・画面・ドライバ・`device.rs` には触らない。
//!
//! 現状は [`UnsupportedActivityRecognizer`] を返すだけで、ダミーの成功へは倒さない
//! （画面は「この環境では行動認識を使えません」を出す）。
//!
//! # (b-ios) #614 が埋める範囲
//!
//! - `CMMotionActivityManager` と専用 serial queue による購読（開始直後に現在の行動が 1 件届く）
//! - `read` のたびに `authorizationStatus` を確かめ、拒否コード 2 種
//!   （`CMErrorNotAuthorized` と `CMErrorMotionActivityNotAuthorized`）を `PermissionDenied` にする。
//!   許可ダイアログの応答待ちは `AwaitingPermission`
//! - 分類フラグは `fandhe_device::activity_recognition::classify_flags` で種別にまとめる
//! - 背景遷移の受け口から `device/activity_recognition.rs` の `note_backgrounded` / `note_resumed` を呼ぶ
//! - 値の確定直後に `crate::ios::request_repaint` で次フレームを起こす

use std::sync::Arc;

use fandhe_device::activity_recognition::{ActivityRecognizer, UnsupportedActivityRecognizer};

/// プロセスで共有する iOS 実装を返す（#614 が実体に置き換えるまでは非対応を返す）。
pub(crate) fn implementation() -> Arc<dyn ActivityRecognizer> {
    Arc::new(UnsupportedActivityRecognizer)
}
