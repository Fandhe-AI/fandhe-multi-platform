//! macOS / iOS 向け住所の座標変換実装（Issue #691 macOS、iOS は #692）。
//! `device/geocoding_search.rs` の macOS 経路（と #692 以降の iOS 経路）から呼ばれ、
//! `apple/fandhe_apple_geocoding_search.m`（MapKit `MKGeocodingRequest`。`build.rs` が feature
//! `device` 有効時に `cc` でコンパイル）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの
//! 呼び出し 1 か所に閉じる（coding-rust.md の FFI 例外規定準拠）。`build.rs` のコンパイル条件・
//! `device.rs` の mod cfg と同じ OS 集合に保つ。
//!
//! 契約の SSOT は `fandhe_device::geocoding_search`。戻りコード → `DeviceError` の写像の SSOT は
//! 本ファイルの `match`。呼び出しはブロックするため、呼び出し元はワーカーで呼ぶ。
// iOS では #692 が使い始めるまで未使用になる。使い始めたら本行を外してよい。
#![cfg_attr(target_os = "ios", allow(dead_code))]

use std::time::Duration;

use fandhe_device::geocoding_search::{
    check_result, validate_address, ForwardGeocoder, GeocodeResult, NOT_FOUND, OFFLINE, THROTTLED,
    TIMEOUT,
};
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_geocoding_search.m` が定義する C ABI（`build.rs` が macOS・iOS
// ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は .m 冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_geocoding_search_forward(
        addr: *const u8,
        len: usize,
        deadline_ms: u64,
        out_lat: *mut f64,
        out_lon: *mut f64,
        out_has_coord: *mut u8,
        out_count: *mut u32,
    ) -> i32;
}

/// MapKit `MKGeocodingRequest` で住所を座標へ変換する実装。
pub(crate) struct AppleForwardGeocoder;

impl ForwardGeocoder for AppleForwardGeocoder {
    #[allow(unsafe_code)]
    fn geocode(&self, address: &str, remaining: Duration) -> Result<GeocodeResult, DeviceError> {
        validate_address(address)?;
        let deadline_ms = u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX);
        let (mut lat, mut lon, mut has, mut count) = (0.0f64, 0.0f64, 0u8, 0u32);
        // SAFETY: `address`・`lat`・`lon`・`has`・`count` は呼び出しの間生存する領域で、ObjC は
        // 呼び出しの間だけ参照し保持しない（住所は NSString へコピーしてから main queue へ渡す）。
        // out 引数への書き込みは戻り値 0 のときだけ、待機明けに呼び出しスレッド自身が行い、
        // 期限切れ（5）で戻った後に走る処理は ObjC の箱だけを触って Rust の領域へは書かない。
        let code = unsafe {
            fandhe_apple_geocoding_search_forward(
                address.as_ptr(),
                address.len(),
                deadline_ms,
                &mut lat,
                &mut lon,
                &mut has,
                &mut count,
            )
        };
        match code {
            0 => check_result(lat, lon, has != 0, count),
            1 => Err(DeviceError::Unsupported("geocoding_search")),
            2 => Err(DeviceError::Internal(OFFLINE)),
            3 => Err(DeviceError::Internal(THROTTLED)),
            4 => Err(DeviceError::Internal(NOT_FOUND)),
            5 => Err(DeviceError::Internal(TIMEOUT)),
            6 => Err(DeviceError::InvalidInput("geocoding_search address")),
            // 7（main スレッドから呼ばれた）・8（OS の失敗）・未知のコード。
            _ => Err(DeviceError::Internal("geocoding_search")),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use fandhe_core::screens::geocoding_search_show::ADDRESS;
    use fandhe_device::geocoding_search::{GEOCODE_DEADLINE, NO_COORDINATE};

    /// 実呼び出しテストの期限。テスト実行体の main thread は runloop を回さず、MapKit の
    /// 問い合わせが返らないことがあるため（実アプリでは winit の runloop が回る）、短くして
    /// 期限切れ（Timeout）を許容する。
    const TEST_DEADLINE: Duration = Duration::from_secs(3);

    /// 実呼び出し。Ok なら有効座標と 1 件以上、Err なら契約上の失敗のどれか。
    fn assert_honest(r: Result<GeocodeResult, DeviceError>) {
        match r {
            Ok(g) => {
                assert!((-90.0..=90.0).contains(&g.latitude));
                assert!((-180.0..=180.0).contains(&g.longitude));
                assert!(g.candidates >= 1);
            }
            Err(DeviceError::Unsupported(_)) => {}
            Err(DeviceError::Internal(id)) => {
                assert!(
                    [
                        OFFLINE,
                        THROTTLED,
                        NOT_FOUND,
                        NO_COORDINATE,
                        TIMEOUT,
                        "geocoding_search"
                    ]
                    .contains(&id),
                    "{id}"
                )
            }
            Err(e) => panic!("想定外の失敗: {e:?}"),
        }
    }

    #[test]
    fn fixed_address_never_fake_success() {
        assert_honest(AppleForwardGeocoder.geocode(ADDRESS, TEST_DEADLINE));
    }

    #[test]
    #[allow(unsafe_code)]
    fn ffi_entry_rejects_oversized_address() {
        let addr = vec![b'a'; 4097];
        let (mut lat, mut lon, mut has, mut count) = (0.0f64, 0.0f64, 0u8, 0u32);
        // SAFETY: 各領域は呼び出しの間生存する。長さ上限の guard は availability 判定より前で、
        // MapKit には触れず 6 を返す。
        let code = unsafe {
            fandhe_apple_geocoding_search_forward(
                addr.as_ptr(),
                addr.len(),
                1000,
                &mut lat,
                &mut lon,
                &mut has,
                &mut count,
            )
        };
        assert_eq!(code, 6);
    }

    #[test]
    fn bad_input_is_rejected_before_ffi() {
        for bad in ["", "  ", "a\0b"] {
            assert!(matches!(
                AppleForwardGeocoder.geocode(bad, GEOCODE_DEADLINE),
                Err(DeviceError::InvalidInput(_))
            ));
        }
        let long = "a".repeat(300);
        assert!(matches!(
            AppleForwardGeocoder.geocode(&long, GEOCODE_DEADLINE),
            Err(DeviceError::InvalidInput(_))
        ));
    }
}
