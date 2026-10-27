//! iOS 向け端末内での画像生成のスタブ（Issue #975）。`device/image_generation.rs` の iOS 経路
//! （dispatch）から [`implementation`] が呼ばれ、常に `Unsupported` を返す（成功を返さない）。
//! 対応表の iOS は `possible` のままで、`impl` にも書かない。実体は #976 が置く。
//!
//! # #976 への申し送り
//!
//! - UIKit で `ImagePlaygroundViewController` を提示する（Swift 側 `#else` 分岐は現在 2 を返すだけ）
//! - `presentationControllerDidDismiss` を `Cancelled`（コード 1）にする
//! - アプリが前面でないときは `NotForeground`（コード 6）
//! - `build.rs` の iOS 分岐から `compile_apple_image_generation(true)` を呼ぶ
//! - `apple_image_generation.rs` の `allow(dead_code)` を外し、本スタブを `AppleImageGenerator` に置き換える
//! - 受け口 `on_done` が `crate::ios::request_repaint` を呼ぶ（実装済み）
//! - dispatch・画面・文言・契約（`fandhe_device::image_generation`）は触らない

use std::sync::Arc;

use fandhe_device::image_generation::{ImageGenerator, UnsupportedImageGenerator};

/// iOS の `ImageGenerator` 実装を返す（#976 までは非対応固定）。
pub(crate) fn implementation() -> Arc<dyn ImageGenerator> {
    Arc::new(UnsupportedImageGenerator)
}
