//! iOS 向け端末内翻訳実装（Issue #536 b-ios）。
//! `device/translation.rs` の iOS 経路から呼ばれ、macOS と共用の Translation 実装
//! （`apple_translation::AppleTextTranslator`）を返す。dispatch・画面・文言は触らない。
//!
//! 契約表 4（再描画）: 結果は専用ワーカーが Swift の同期呼び出し（セマフォ待ち）から戻ったあと
//! `RequestSlot::deliver` で置く pull 型で、Rust の共有状態へ書き込む OS コールバックは無い。
//! 待機中はドライバの `poll_and_finish` が毎フレーム `request_repaint_after` を予約するため、
//! 入力が無くても結果を拾うまで次フレームが起きる。`crate::ios::request_repaint` を別に呼ぶ必要はない。
//!
//! 契約表 1（ライフサイクル）: 背景遷移で suspend をはさんで期限を過ぎた場合は、復帰後にドライバが
//! `Timeout` で確定し、遅れて届いた結果は世代照合で捨てる。OS 側の失敗は `Failed` に写し、
//! 嘘の成功にしない。iOS 26 未満は `Unsupported`（Swift の `#available` で判定）。
//! 守る範囲・守らない範囲は `fandhe_device::translation` の trait doc を正とする。

use std::sync::Arc;

use fandhe_device::translation::TextTranslator;

/// iOS の `TextTranslator` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn TextTranslator> {
    Arc::new(super::apple_translation::AppleTextTranslator)
}
