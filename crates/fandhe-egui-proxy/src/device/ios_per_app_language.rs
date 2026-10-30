//! iOS 向けアプリ別の言語設定実装（Issue #1074。親 #137 の b-ios、骨格は #1073）。
//! `device/per_app_language.rs::platform_per_app_language` の iOS 経路から呼ばれ、macOS と共用の
//! `apple_per_app_language`（`apple/fandhe_apple_per_app_language.m`、`Bundle.main.preferredLocalizations` の先頭）を返す。
//! Linux ホストの `cargo test` では共用実装が存在しないため、関数と import は iOS 限定にしている。
//!
//! # iOS 固有の挙動
//!
//! - 設定アプリのこのアプリの「言語」は、端末の優先言語が 2 つ以上のときだけ出る。選べる言語は
//!   `platforms/apple/FandheIosApp/Resources/Info.plist` の `CFBundleLocalizations`（`ja`・`en`）で決まる。
//! - 言語を変えるとアプリは再起動する。読み取りは押下ごとに OS へ聞く。
//! - `Locale.preferredLanguages` は対応言語に関係なく利用者の希望一覧を返すため使わない。
//!
//! # 再描画（契約表 4）
//!
//! OS コールバックが無い（UI スレッドの effect で同期に 1 回読む）。完了は `CompletionSender::send` が
//! egui の `request_repaint` を呼んで届くため、`crate::ios::request_repaint` は足さない（該当なし）。
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! `fandhe_device::per_app_language` の trait doc に従う。

#[cfg(target_os = "ios")]
use fandhe_device::per_app_language::PerAppLanguage;

/// iOS の実装を返す（macOS と共用）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Box<dyn PerAppLanguage> {
    super::apple_per_app_language::implementation()
}
