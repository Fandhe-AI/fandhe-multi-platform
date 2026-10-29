//! iOS 向け端末内での画像生成実装（Issue #976 b-ios）。
//! `device/image_generation.rs` の iOS 経路から呼ばれ、macOS と共用の Image Playground 実装
//! （`apple_image_generation::AppleImageGenerator`。iOS では Swift 側が UIKit で提示する）を返す。
//! dispatch・画面・文言・契約（`fandhe_device::image_generation`）は触らない。
//!
//! 契約表 4（再描画）: 結果は OS コールバックが共有状態へ入れるため、`apple_image_generation::on_done` が
//! `complete_from_os` の直後に `crate::ios::request_repaint` を呼ぶ（入力が無くても次フレームが起きる）。
//!
//! 契約表 1（ライフサイクル）: background にいる間の要求は `NotForeground`。Inactive（権限ダイアログ等）は
//! 中断扱いにしない。シート表示中に background へ移っても OS がシートを保つので、結果は復帰後に届く。
//! 表示開始前に期限を過ぎたら `TimedOut` で確定し、遅れて届いた結果は世代照合で捨てる。
//! 守る範囲・守らない範囲は `fandhe_device::image_generation` の trait doc を正とする。

use std::sync::Arc;

use fandhe_device::image_generation::ImageGenerator;

/// iOS の `ImageGenerator` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn ImageGenerator> {
    super::apple_image_generation::implementation()
}
