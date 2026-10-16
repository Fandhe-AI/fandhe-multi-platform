//! Android（self-managed ConnectionService） 向け通話 UI 実装のスタブ（Issue #276 (a)。#278 が埋める）。
//! `device/call_ui.rs::platform_call_ui` の android 経路から呼ばれる。実体ができるまでは
//! `UnsupportedCallUi` を返し、成功値へ倒さない。dispatch はドライバ側で最終形のため、
//! 後続は本ファイルの `implementation()` の中身だけを置き換える。

use std::sync::Arc;

use fandhe_device::call_ui::{CallUi, UnsupportedCallUi};

/// この OS の `CallUi` 実装。
pub(crate) fn implementation() -> Arc<dyn CallUi> {
    Arc::new(UnsupportedCallUi)
}
