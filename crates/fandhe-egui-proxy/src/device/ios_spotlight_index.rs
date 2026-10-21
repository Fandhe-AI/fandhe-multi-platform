//! iOS 向け OS の検索（Core Spotlight）への登録（Issue #561）。`device/spotlight_index.rs` の
//! iOS 経路から呼ばれ、登録・削除は macOS と共用の `apple_spotlight_index` へ委譲する。
//!
//! 受信: 検索結果から開かれると、scene の `scene:continueUserActivity:`（起動中）または
//! `connectionOptions.userActivities`（未起動からの起動）が通知
//! `FandheSceneContinueUserActivityNotification` を投げ（`ios/fandhe_ios_scene.m`）、
//! `apple_spotlight_index::register` が足した observer（`apple/fandhe_apple_spotlight_index.m`）が
//! `fandhe://spotlight/<id>` にして `fandhe_rust_url_scheme_received` へ渡す。同関数が受信直後に
//! `crate::ios::request_repaint` を呼ぶため、受信側で再描画を足す必要はない（契約表 4）。
//!
//! 登録・削除は pull 型。ワーカーが `.m` の同期待ちから戻った後にスロットへ結果を入れ、待機中は
//! ドライバの `poll_and_finish` が次回のポーリングを予約するため、`request_repaint` は不要。
//! 期限は `remaining` を `.m` の待機へそのまま渡す。識別子・題名・本文・OS のエラー文はログへ出さない。

use std::sync::Arc;

use fandhe_device::spotlight_index::SpotlightIndex;

/// iOS の `SpotlightIndex` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn SpotlightIndex> {
    super::apple_spotlight_index::implementation()
}
