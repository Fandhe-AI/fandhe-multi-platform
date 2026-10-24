//! Android 向けアプリ内ブラウザ（Custom Tabs）の骨格（親 #135、Issue #701）。
//! `device/in_app_browser.rs` の Android 経路（dispatch）から呼ばれる。実体は #703 が本ファイルと
//! `InAppBrowserHelper.java`・`MainActivity` の最小配線に足す。現状は常に `Unsupported` を返し、
//! 成功を装わない。
//!
//! # #703 が埋めるときの約束
//!
//! - dispatch・`device.rs` の列・`android.rs` の reset 行・共通コア画面・`registry.rs` は触らない
//!   （本ファイルの `implementation()` の中身だけを差し替える）
//! - 機能は `InAppBrowserHelper.java` に分け、要求 id の追跡は `RequestTracker` を使う。
//!   JNI の署名は Java 側・no-op スタブ・実体の 3 か所で一致させる
//! - 表示の直前に `super::in_app_browser::try_begin_present(request_id)` を呼び、`false` なら表示しない
//! - 完了は 1 回だけ `super::in_app_browser::complete_from_os` で送る。
//!   `onPause` → `onResume` は `Ok(Closed)` に写す。`ActivityNotFoundException` は `NoBrowser`、
//!   前面の Activity がなければ `NoPresenter`、再生成・終了は `Interrupted`
//! - OS 層でも scheme が http / https かを再確認する。未知のコードは `Internal` に倒す
//! - URL と OS のエラー文をログに出さない。新しい権限・依存・`<queries>` を足さない

use std::sync::Arc;

use fandhe_device::in_app_browser::{InAppBrowser, UnsupportedInAppBrowser};

/// Android 用の実装を返す。#703 までは `Unsupported` 固定。
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
