//! deep-link（ディープリンク: Universal Links・App Links）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1333）。
//! 扱う範囲は「検証済みドメインの https URL を検査し、最新 1 件を保持して画面が読めるようにする」ところまで（パース・ルーティングはしない）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・Android 実装（`android` と `android/java`）を 1 つに集める。
//! 保持と検査の部品（`UrlInbox`）は url-scheme と共有するため `fandhe-device` に残っており、本 crate は url-scheme の crate に依存しない
//! （選んでいないアプリへ `fandhe` scheme の宣言を持ち込まない）。画面の状態型を参照してよいのは `driver` だけ。
//!
//! # Android の受け口と利用者の責務
//!
//! `DeepLinkLifecycle`（非公開 provider）が共通層の `deliverIntents` で起動時と `onNewIntent` を `DeepLinkHelper.deliver` へ配る（#1325）。
//! `autoVerify` 付きの https の intent-filter（ホストは確認用ドメインに依存）は宣言では表せないため、利用者が
//! `FANDHE_ANDROID_DEEP_LINK_HOST` を渡して flavor の Manifest（`platforms/android/app/src/deeplink/AndroidManifest.xml`）に足す。
//! 許可ホストはビルド時の同じ環境変数で、未設定なら届いた URL を全件拒否する（成功を装わない）。
//!
//! 守る範囲: 1 プロセス内の受信順、不正 URL の非表示、コールドスタート分の保持、判定と更新を同じ lock の内側で行うこと。
//! 守らない範囲: ポーリング間隔内の連続受信で最後の 1 件だけ表示されること、複数プロセス間、OS 側のリンク検証状態の表示。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
mod error;

pub use contract::*;
pub use driver::platform_deep_link;
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::DeepLinkError;
