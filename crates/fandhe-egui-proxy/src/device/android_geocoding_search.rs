//! Android 向け住所の座標変換実装の骨格（Issue #691 a、実体は #693 b-android）。
//! `device/geocoding_search.rs` の Android 経路から呼ばれる。現時点は `Unsupported` を返す
//! スタブで、ダミーの成功へは倒さない。後続の子がホストテストを足すときに `device.rs` の cfg を
//! 広げずに済むよう test でもコンパイルする。
//!
//! b-android への約束:
//!
//! - `android.location.Geocoder`: API 33 以上は `GeocodeListener` 版、24〜32 は同期版を executor で
//!   呼ぶ。`Geocoder.isPresent()` が false は `Unsupported`
//! - `maxResults` は `fandhe_device::geocoding_search::MAX_CANDIDATES`。空リストは `NOT_FOUND`、
//!   `hasLatitude/hasLongitude` が偽は座標なし（`check_result` の `has_coordinate = false`）
//! - `IOException` と `onError` は通信不可・回数上限を区別できないため `Internal("geocoding_search")`
//! - JNI の署名は Java・no-op スタブ・実体の 3 か所で一致させる。`ACCESS_*_LOCATION` は足さない
//!   （住所 → 座標は位置情報を使わない）
//! - Activity 再生成での保留破棄は `android.rs` から `geocoding_search::reset_pending_tracking` へ
//!   配線済み
//!
//! 守る範囲・守らない範囲は `fandhe_device::geocoding_search` の doc を正とする。

use std::sync::Arc;

use fandhe_device::geocoding_search::{ForwardGeocoder, UnsupportedForwardGeocoder};

/// Android の `ForwardGeocoder` 実装を返す（骨格: `Unsupported` 固定）。
pub(crate) fn implementation() -> Arc<dyn ForwardGeocoder> {
    Arc::new(UnsupportedForwardGeocoder)
}
