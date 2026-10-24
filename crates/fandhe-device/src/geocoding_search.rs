//! 住所の座標変換（順ジオコーディング）の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/geocoding-search.json`、親 #133、Issue #691）。
//!
//! 扱う範囲は「画面に埋め込んだ固定の公開住所 1 件を OS の地理コードで座標へ変換し、先頭候補の
//! 緯度・経度と候補数を返す」ことだけ。経路（`MKDirections`）・地点検索（`MKLocalSearch`）・
//! 逆ジオコーディング・利用者の入力・現在地は扱わない。対象は macOS / iOS（MapKit
//! `MKGeocodingRequest`、26 以上）と Android（`android.location.Geocoder`）。Windows・Web・Linux は
//! `Unsupported` 固定。一発呼び出し型の同期 API で、呼び出し側（`fandhe-egui-proxy` の effect
//! ドライバ）は UI を止めないようワーカースレッドから呼ぶ。
//!
//! # 契約
//!
//! - **成功**: 座標が有効な先頭候補と候補数だけ（[`check_result`]）。0 件・座標なしは成功にしない
//!   （`device-bridge-contracts.md`「6. 嘘の成功」）。未取得は番兵値ではなく `has_coordinate`
//!   フラグで表す（契約表 8）
//! - **失敗の区別**（写像表）:
//!
//! | 状況 | Apple | Android | 返す値 |
//! | --- | --- | --- | --- |
//! | OS が古い・provider 無し | macOS / iOS 26 未満 | `Geocoder.isPresent()` が false | `Unsupported` |
//! | 通信不可 | `NSURLError` の通信不可 4 種・`MKErrorServerFailure` の underlying | 区別できない | `Internal(`[`OFFLINE`]`)` |
//! | 回数上限 | `MKErrorLoadingThrottled` | 区別できない | `Internal(`[`THROTTLED`]`)` |
//! | 該当なし | `MKErrorPlacemarkNotFound`・空配列 | 空リスト | `Internal(`[`NOT_FOUND`]`)` |
//! | 座標なし・範囲外 | `MKMapItem.location` が nil・無効 | `hasLatitude/hasLongitude` が偽 | `Internal(`[`NO_COORDINATE`]`)` |
//! | 期限切れ | Rust の期限 | Rust の期限 | `Internal(`[`TIMEOUT`]`)` |
//! | 入力不正 | — | — | `InvalidInput`（[`validate_address`]） |
//! | それ以外（OS 側の取り消し・`IOException`・`onError` を含む） | 他の `NSError` | 他の例外 | `Internal("geocoding_search")` |
//!
//! Android は通信不可と回数上限を区別できないため、どちらも `Internal("geocoding_search")` に入る。
//!
//! - **三値**（契約表 10）: 利用者の取り消し操作が無いため成功 / 失敗の 2 値。OS が取り消しを
//!   報告した場合も失敗
//! - **期限**（契約表 3）: 期限は Rust のドライバだけが持つ（[`GEOCODE_DEADLINE`]）。起点は要求を
//!   観測した時の `Instant`（単調時計）。期限後に届いた成功は捨てる。Apple は残り時間を ms で渡し、
//!   ObjC 側も `dispatch_time`（単調）で待つ
//! - **並行性**（契約表 2）: 世代は画面が要求ごとに進め、完了は「処理中かつ同じ世代」のときだけ
//!   適用する。処理中の再押下は無視する
//! - **ライフサイクル**（契約表 1）: Android は Activity 再生成でドライバの記録を捨てる
//! - **資源の解放**（契約表 9）: Apple の要求オブジェクトは完了・期限切れ（`cancel`）・生成失敗の
//!   全経路で参照を外す
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス・1 画面の順序、結果は 1 回きり、0 件・座標なしを成功にしない、期限後の成功を
//! 捨てる、住所・座標をログへ出さない。守らない範囲: 結果の精度と候補の選択、回数上限、期限後も
//! OS 側の処理が走り切ること、背景にいる間の要求、複数プロセス。

use std::time::Duration;

use crate::DeviceError;

/// 要求を受け付けてから結果を確定するまでの期限（Rust のドライバだけが持つ）。
pub const GEOCODE_DEADLINE: Duration = Duration::from_secs(15);
/// 住所の最大バイト数。
pub const MAX_ADDRESS_BYTES: usize = 256;
/// 数える候補数の上限（Android の `maxResults` と表示上の上限を揃える）。
pub const MAX_CANDIDATES: u32 = 5;

/// 通信できないときの `Internal` の識別子（SSOT）。
pub const OFFLINE: &str = "geocoding_search offline";
/// 回数上限に達したときの `Internal` の識別子（SSOT）。
pub const THROTTLED: &str = "geocoding_search throttled";
/// 該当する場所が無いときの `Internal` の識別子（SSOT）。
pub const NOT_FOUND: &str = "geocoding_search not_found";
/// 先頭候補に有効な座標が無いときの `Internal` の識別子（SSOT）。
pub const NO_COORDINATE: &str = "geocoding_search no_coordinate";
/// 期限切れのときの `Internal` の識別子（SSOT）。
pub const TIMEOUT: &str = "geocoding_search timeout";

/// 変換結果（先頭候補の座標と、数えた候補数）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeocodeResult {
    /// 緯度（度。[-90, 90]）。
    pub latitude: f64,
    /// 経度（度。[-180, 180]）。
    pub longitude: f64,
    /// 候補数（1 以上 [`MAX_CANDIDATES`] 以下）。
    pub candidates: u32,
}

/// 住所の座標変換の契約（モジュール doc の「契約」節）。
pub trait ForwardGeocoder: Send + Sync {
    /// `address` を座標へ 1 回変換する。`remaining` は期限までの残り時間。
    fn geocode(&self, address: &str, remaining: Duration) -> Result<GeocodeResult, DeviceError>;
}

/// 対象外 OS・未実装 OS のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedForwardGeocoder;

impl ForwardGeocoder for UnsupportedForwardGeocoder {
    fn geocode(&self, _: &str, _: Duration) -> Result<GeocodeResult, DeviceError> {
        Err(DeviceError::Unsupported("geocoding_search"))
    }
}

/// 住所を OS へ渡す前に検査する。空・空白だけ・NUL を含む・[`MAX_ADDRESS_BYTES`] 超過は
/// `InvalidInput`。
pub fn validate_address(address: &str) -> Result<(), DeviceError> {
    if address.trim().is_empty() || address.contains('\0') || address.len() > MAX_ADDRESS_BYTES {
        return Err(DeviceError::InvalidInput("geocoding_search address"));
    }
    Ok(())
}

/// OS の結果を検査して [`GeocodeResult`] にする。範囲の検証は丸め・表示整形の前に行う（契約表 5）。
/// `count == 0` は `NOT_FOUND`、座標なし・非有限・範囲外は `NO_COORDINATE`。(0, 0) は有効な座標。
/// 候補数は [`MAX_CANDIDATES`] で頭打ちにする。
pub fn check_result(
    latitude: f64,
    longitude: f64,
    has_coordinate: bool,
    count: u32,
) -> Result<GeocodeResult, DeviceError> {
    if count == 0 {
        return Err(DeviceError::Internal(NOT_FOUND));
    }
    let valid = has_coordinate
        && latitude.is_finite()
        && longitude.is_finite()
        && (-90.0..=90.0).contains(&latitude)
        && (-180.0..=180.0).contains(&longitude);
    if !valid {
        return Err(DeviceError::Internal(NO_COORDINATE));
    }
    Ok(GeocodeResult {
        latitude,
        longitude,
        candidates: count.min(MAX_CANDIDATES),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_address_boundaries() {
        assert!(validate_address("東京都千代田区丸の内一丁目").is_ok());
        assert!(validate_address(&"a".repeat(MAX_ADDRESS_BYTES)).is_ok());
        for bad in [
            "".to_string(),
            " \n\t".to_string(),
            "a\0b".to_string(),
            "a".repeat(MAX_ADDRESS_BYTES + 1),
        ] {
            assert!(
                matches!(
                    validate_address(&bad),
                    Err(DeviceError::InvalidInput("geocoding_search address"))
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn zero_candidates_is_not_found() {
        assert!(matches!(
            check_result(35.0, 139.0, true, 0),
            Err(DeviceError::Internal(NOT_FOUND))
        ));
        // 0 件は座標の有無より先に判定する
        assert!(matches!(
            check_result(0.0, 0.0, false, 0),
            Err(DeviceError::Internal(NOT_FOUND))
        ));
    }

    #[test]
    fn missing_or_invalid_coordinate_is_no_coordinate() {
        for (lat, lon, has) in [
            (35.0, 139.0, false),
            (f64::NAN, 139.0, true),
            (35.0, f64::INFINITY, true),
            (90.1, 0.0, true),
            (-90.1, 0.0, true),
            (0.0, 180.1, true),
            (0.0, -180.1, true),
        ] {
            assert!(
                matches!(
                    check_result(lat, lon, has, 1),
                    Err(DeviceError::Internal(NO_COORDINATE))
                ),
                "{lat} {lon} {has}"
            );
        }
    }

    #[test]
    fn boundary_and_origin_coordinates_succeed() {
        for (lat, lon) in [(90.0, 180.0), (-90.0, -180.0), (0.0, 0.0)] {
            let r = check_result(lat, lon, true, 1).unwrap();
            assert_eq!((r.latitude, r.longitude, r.candidates), (lat, lon, 1));
        }
    }

    #[test]
    fn candidates_are_capped() {
        assert_eq!(check_result(1.0, 1.0, true, 3).unwrap().candidates, 3);
        assert_eq!(
            check_result(1.0, 1.0, true, 99).unwrap().candidates,
            MAX_CANDIDATES
        );
    }

    #[test]
    fn unsupported_is_not_a_fake_success() {
        let e = UnsupportedForwardGeocoder
            .geocode("x", GEOCODE_DEADLINE)
            .unwrap_err();
        assert!(matches!(e, DeviceError::Unsupported("geocoding_search")));
    }
}
