//! Android 向け共有の受け取り（Issue #606 で骨格、実体は #607）。`device/share_receive.rs::
//! platform_share_receive` の Android 経路から呼ばれる。
//!
//! 現時点は `UnsupportedShareReceive` を返す薄いスタブ（対応表は `possible` のまま）。
//! #607 が埋める範囲: `ShareReceiveHelper.java`、`EXTRA_TEXT` のみの受信、JNI 署名の
//! Java 側・no-op スタブ・実体の 3 か所一致、`onCreate` の既存ガード、`onNewIntent` の
//! `deliverUrlIntent` の隣での配送、static `ShareInbox`、feature `device` 無効時の no-op
//! （`crate::android`）。dispatch・画面・`registry.rs`・`device.rs` の `pub mod` 列は
//! #606 で最終形のため #607 は触らない。

use fandhe_device::share_receive::{ShareReceive, UnsupportedShareReceive};

/// Android の実装を返す。#607 で static `ShareInbox` を読む実体に差し替える。
pub(crate) fn implementation() -> &'static dyn ShareReceive {
    &UnsupportedShareReceive
}
