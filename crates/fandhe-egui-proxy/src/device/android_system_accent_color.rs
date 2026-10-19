//! Android 向けシステムのアクセント色実装のスタブ（Issue #421、#422 が埋める）。
//! `device/system_accent_color.rs::implementation` の Android 経路から呼ばれる。
//!
//! #422（b-android）は本ファイルだけを実装に置き換える。JNI の署名は `()I`、コード表は
//! `fandhe_device::system_accent_color::rgb_from_code` が SSOT（`-2` = API 30 以下で
//! `Unsupported`）。Activity 未確立は `Internal`。それまでは非対応を返す（成功を偽らない）。

use std::sync::Arc;

use fandhe_device::system_accent_color::{AccentColorSource, UnsupportedAccentColor};

/// Android の実装を返す（現状は非対応のスタブ）。
pub(crate) fn implementation() -> Arc<dyn AccentColorSource> {
    Arc::new(UnsupportedAccentColor)
}
