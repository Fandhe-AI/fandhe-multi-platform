//! geocoding-search の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。住所 → 座標の変換（順ジオコーディング）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`geocoding_search_show_effect`）。iOS・macOS は MapKit、Android は
//! `android.location.Geocoder`（Activity ごとの `GeocodingHelper` を `GeocodingLifecycle` が束ねる）が実装し、他の OS は `Unsupported`。
//! Activity の作り直しでは `fandhe-gallery` の `android_main` が `reset_pending_tracking` を呼ぶ。
//! 守る範囲は 1 プロセス内の要求順序・嘘の成功の禁止（0 件・座標なしを成功にしない）・住所と座標のログ非出力。
//! 守らない範囲は OS 側の処理が期限後も走り切ること、通信不可と回数上限の区別（Android）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_geocoding_search()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_os = "macos")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "ios", target_os = "macos"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;

pub use contract::*;
pub use driver::*;
pub use error::GeocodingSearchError;
