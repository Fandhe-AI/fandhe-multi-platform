//! spell-check の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。短い文のスペルチェック。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`spell_check_effect`）。macOS（`NSSpellChecker`）・iOS（`UITextChecker`）・
//! Android（`TextServicesManager`、Activity ごとの `SpellCheckHelper` を `SpellCheckLifecycle` が束ねる）・Windows（`ISpellChecker`）が実装する。
//! Activity の作り直しでは `fandhe-gallery` の `android_main` が `reset_pending_tracking` を呼ぶ。
//! 守る範囲は 1 プロセス内の要求順序・嘘の成功の禁止（辞書なしを「誤り 0 件」にしない）・session の解放・入力と誤り語のログ非出力。
//! 守らない範囲は期限後も OS 側の検査が走り切ること、OS（OEM・キーボードアプリ）による結果の違い。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_spell_check()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    )),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    ),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::SpellCheckError;
