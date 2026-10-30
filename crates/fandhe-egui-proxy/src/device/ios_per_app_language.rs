//! iOS 向けアプリ別の言語設定（Issue #1073 の骨格。実装は #1074）。
//! `device/per_app_language.rs::platform_per_app_language` の iOS 経路から呼ばれる。
//! 現状は `Unsupported` を返すスタブ（ダミーの成功へ倒さない）。
//!
//! 後続 b-ios（#1074）への申し送り: `implementation()` を `apple_per_app_language::implementation()`
//! へ差し替え、`platforms/apple/FandheIosApp/Resources/Info.plist` に `CFBundleLocalizations`
//! （`ja`・`en`）を足す。build.rs（`.m` のコンパイルは #1073 で済み）・dispatch・画面には触らない。

use fandhe_device::per_app_language::{PerAppLanguage, UnsupportedPerAppLanguage};

/// iOS 用の実装を返す（#1074 で差し替え）。
pub(crate) fn implementation() -> Box<dyn PerAppLanguage> {
    Box::new(UnsupportedPerAppLanguage)
}
