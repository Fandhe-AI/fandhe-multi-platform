//! iOS（ActivityKit）向けライブアクティビティ実装のスタブ（Issue #294 (a)。b-ios が埋める）。
//! `device/live_activity.rs::platform_live_activity` の ios 経路から呼ばれる。実体（ActivityKit +
//! Swift 薄層 + Widget Extension）ができるまでは `UnsupportedLiveActivity` を返し、成功値へ倒さない。
//! dispatch はドライバ側で最終形のため、後続は本ファイルの `implementation()` の中身だけを置き換える。

use std::sync::Arc;

use fandhe_device::live_activity::{LiveActivity, UnsupportedLiveActivity};

/// この OS の `LiveActivity` 実装。
pub(crate) fn implementation() -> Arc<dyn LiveActivity> {
    Arc::new(UnsupportedLiveActivity)
}
