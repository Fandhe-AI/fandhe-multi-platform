//! Android 向け音楽ライブラリ実装のスタブ（Issue #482 が骨格を置き、#485 が埋める）。
//! `device/music_library.rs::implementation` の Android 経路から呼ばれる。実装は常に
//! `Unsupported` を返し、ダミーの成功へは倒さない。
//!
//! #485 はこのファイルの `implementation()` だけを埋める。dispatch・画面・文言・
//! `device.rs` の `pub mod` 列は触らない。契約の SSOT は `fandhe_device::music_library`。

use std::sync::Arc;

use fandhe_device::music_library::{MusicLibrary, UnsupportedMusicLibrary};

/// Android の `MusicLibrary` 実装を返す（#485 が置き換える）。
pub(crate) fn implementation() -> Arc<dyn MusicLibrary> {
    Arc::new(UnsupportedMusicLibrary)
}
