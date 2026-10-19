//! Android 向け集中モードの状態実装のスタブ（Issue #438、b-android が埋める）。
//! `device/focus_mode_status.rs::implementation` の Android 経路から呼ばれる。
//!
//! b-android は本ファイルだけを実装に置き換える。JNI の署名は `()I`、メソッド名は
//! `focusModeStatusRead`、コード表は `fandhe_device::focus_mode_status::focus_from_code` が SSOT。
//! Activity 未確立は `Internal`。それまでは非対応を返す（成功を偽らない）。

use std::sync::Arc;

use fandhe_device::focus_mode_status::{FocusStatusSource, UnsupportedFocusStatus};

/// Android の実装を返す（現状は非対応のスタブ）。
pub(crate) fn implementation() -> Arc<dyn FocusStatusSource> {
    Arc::new(UnsupportedFocusStatus)
}
