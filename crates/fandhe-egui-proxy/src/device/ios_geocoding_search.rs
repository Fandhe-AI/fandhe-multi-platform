//! iOS 向け住所の座標変換実装（Issue #692、骨格は #691）。
//! `device/geocoding_search.rs` の iOS 経路から呼ばれ、macOS と共用の
//! `apple_geocoding_search::AppleForwardGeocoder`（MapKit `MKGeocodingRequest`、
//! `apple/fandhe_apple_geocoding_search.m`）へ委譲する薄いラッパーを返す。
//!
//! - iOS 26 未満は ObjC 側の `@available` で `Unsupported`（非推奨の `CLGeocoder` へは倒さない）
//! - 背景にいる間は completion の配送が遅れ得る。その場合は Rust 側の期限（`Timeout`）で失敗が
//!   確定し、遅れて届いた結果は世代照合で捨てられる
//! - 権限・Info.plist のキーは不要（住所 → 座標は現在地を使わない）
//!
//! 契約表 4（再描画）: 結果は `RequestSlot::deliver` で置く pull 型で、Rust の共有状態へ書き込む
//! OS コールバックは無い。待機中はドライバの `poll_and_finish` が毎フレーム
//! `request_repaint_after` を予約するので画面は止まらない。補強として FFI から戻った直後にも
//! `crate::ios::request_repaint` を呼ぶ（`ios_spell_check.rs` と同じ形）。この呼び出しは
//! `deliver` より前に走るため、結果の反映の保証は `poll_and_finish` の予約が担う。
//! 守る範囲・守らない範囲は `fandhe_device::geocoding_search` の doc を正とする。

use std::sync::Arc;
use std::time::Duration;

use fandhe_device::geocoding_search::{ForwardGeocoder, GeocodeResult};
use fandhe_device::DeviceError;

use super::apple_geocoding_search::AppleForwardGeocoder;

/// `AppleForwardGeocoder` へ委譲し、戻った直後に次フレームを起こす iOS 用ラッパー。
/// 戻り値・失敗の写像は委譲先と同一。
struct IosForwardGeocoder;

impl ForwardGeocoder for IosForwardGeocoder {
    fn geocode(&self, address: &str, remaining: Duration) -> Result<GeocodeResult, DeviceError> {
        let result = AppleForwardGeocoder.geocode(address, remaining);
        crate::ios::request_repaint();
        result
    }
}

/// iOS の `ForwardGeocoder` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn ForwardGeocoder> {
    Arc::new(IosForwardGeocoder)
}
