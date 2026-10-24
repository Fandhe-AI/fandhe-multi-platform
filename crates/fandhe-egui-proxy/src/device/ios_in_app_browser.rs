//! iOS 向けアプリ内ブラウザ（`SFSafariViewController`）の骨格（親 #135、Issue #701）。
//! `device/in_app_browser.rs` の iOS 経路（dispatch）から呼ばれる。実体は #702 が本ファイルと
//! `ios/fandhe_ios_in_app_browser.m`・`build.rs` に足す。現状は常に `Unsupported` を返し、
//! 成功を装わない。
//!
//! # #702 が埋めるときの約束
//!
//! - dispatch・`device.rs` の列・共通コア画面・`registry.rs` は触らない（本ファイルの
//!   `implementation()` の中身だけを差し替える）
//! - 表示の直前に `super::in_app_browser::try_begin_present(request_id)` を呼び、`false` なら表示しない
//! - 完了は 1 回だけ `super::in_app_browser::complete_from_os` で送る。
//!   `safariViewControllerDidFinish` は `Ok(Closed)` に写す
//! - main thread で `@autoreleasepool` の内側から present する。最前面の View Controller が
//!   取れなければ `NoPresenter`。背景遷移で未表示なら `Interrupted`
//! - 完了の直後に `crate::ios::request_repaint` を呼ぶ（入力がないと egui が描画せず止まる）
//! - OS 層でも scheme が http / https かを再確認する。未知のコードは `Internal` に倒す
//! - URL と OS のエラー文をログに出さない。新しい権限・依存を足さない

use std::sync::Arc;

use fandhe_device::in_app_browser::{InAppBrowser, UnsupportedInAppBrowser};

/// iOS 用の実装を返す。#702 までは `Unsupported` 固定。
pub(crate) fn implementation() -> Arc<dyn InAppBrowser> {
    Arc::new(UnsupportedInAppBrowser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::in_app_browser::{validate_url, InAppBrowserFailure};

    #[test]
    fn stub_never_succeeds() {
        let u = validate_url("https://example.com/").unwrap();
        assert_eq!(
            implementation().open(1, &u),
            Err(InAppBrowserFailure::Unsupported)
        );
    }
}
