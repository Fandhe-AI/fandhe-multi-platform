//! Android 向けアプリ別の言語設定（Issue #1073 の骨格。実装は #1075）。
//! `device/per_app_language.rs::platform_per_app_language` の Android 経路から呼ばれる。
//! 現状は `Unsupported` を返すスタブ（ダミーの成功へ倒さない）。
//!
//! 後続 b-android（#1075）への申し送り: `PerAppLanguageHelper.java` を足し、JNI の署名
//! `perAppLanguageRead()Ljava/lang/String;` を Java 側・no-op スタブ・実体の 3 か所で一致させる。
//! `res/xml/locale_config.xml`（`ja`・`en`）と `<application android:localeConfig>` を宣言する。
//! API 33 未満は `Unsupported`。`getApplicationLocales()` が空なら `System`（成功）、
//! `getSystemLocales()` の空は失敗（向きは `fandhe_device::per_app_language` の doc で固定済み）。
//! dispatch・画面には触らない。

use fandhe_device::per_app_language::{PerAppLanguage, UnsupportedPerAppLanguage};

/// Android 用の実装を返す（#1075 で差し替え）。
pub(crate) fn implementation() -> Box<dyn PerAppLanguage> {
    Box::new(UnsupportedPerAppLanguage)
}
