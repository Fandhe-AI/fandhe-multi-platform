//! Android 向け Web 認証セッション実装のスタブ（Issue #530 が骨格を置き、#534 が埋める）。
//! `device/web_auth_session.rs::implementation` の Android 経路から呼ばれる。実装は常に
//! `Unsupported` を返し、ダミーの成功へは倒さない。
//!
//! #534 はこのファイルの `implementation()` だけを埋める（完了は
//! `web_auth_session::bridge::complete_from_os` へ渡す）。dispatch・画面・文言・
//! `device.rs` の `pub mod` 列は触らない。契約の SSOT は `fandhe_device::web_auth_session`。

use std::sync::Arc;

use fandhe_device::web_auth_session::{UnsupportedWebAuthSession, WebAuthSession};

/// Android の `WebAuthSession` 実装を返す（#534 が置き換える）。
pub(crate) fn implementation() -> Arc<dyn WebAuthSession> {
    Arc::new(UnsupportedWebAuthSession)
}
