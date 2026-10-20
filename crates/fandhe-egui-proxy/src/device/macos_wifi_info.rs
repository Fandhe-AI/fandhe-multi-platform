//! macOS 向け Wi-Fi 情報実装（Issue #514、親 #111）。`macos/fandhe_macos_wifi_info.m`
//! （CoreWLAN `CWWiFiClient` + CoreLocation の許可判定。`build.rs` が macOS 分岐で feature `device`
//! 有効時にコンパイルし CoreWLAN をリンクする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` は
//! この呼び出しに閉じ `// SAFETY:` で契約を明記する。`device/wifi_info.rs` の macOS 経路から
//! ワーカースレッドで呼ばれ、契約は `fandhe_device::wifi_info`。
//!
//! 戻りコードの写像 [`decode_code`] と生データの変換 [`net_from_raw`] は純関数で、ホストの
//! `cargo test` で検査する。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 読む前に位置情報サービス・許可・Wi-Fi の電源を判定し、空の一覧を成功にしない。
//!   範囲の検証（`ssid_len`・`nearby_len`）は変換より先に行う。未知の戻りコードは成功にせず
//!   `Internal`。SSID をログに出さない。BSSID は読まない
//! - 守らない: 許可ダイアログの応答待ちの期限、`cachedScanResults` の鮮度、読んだ後の変化
//!
//! # 未確認事項（Mac 目視 #510 で確認）
//!
//! - 素の実行体（`Info.plist` 埋め込み・非バンドル）で SSID が nil にならないか（nil なら画面は
//!   「非公開」と出す。確かめるには `.app` 化する。それでも駄目なら対応表を `deferred` に戻す）
//! - `interfaceMode` での接続判定が実機の未接続時に正しいか
//! - `cachedScanResults` が空のときの出方
//!
//! 実物の CoreWLAN・CoreLocation はホストのテストから呼ばない（テスト実行体には `Info.plist` が
//! 埋め込まれず、位置情報の許可が出ないため）。

// ホストのテストでは FFI 経路を載せないため、そこでしか使わない import は未使用になる。
#![cfg_attr(not(target_os = "macos"), allow(unused_imports))]

use std::sync::Arc;

use fandhe_device::wifi_info::{
    normalize_nearby, rssi_from_dbm, Ssid, WifiInfo, WifiNetwork, WifiReport, FEATURE,
    LOCATION_OFF, RADIO_OFF, READ_FAILED,
};
use fandhe_device::DeviceError;

/// SSID の最大バイト数（IEEE 802.11）。
const SSID_MAX: usize = 32;
/// `.m` が書く周辺の最大件数（受け取りバッファの件数）。
const NEARBY_CAP: usize = 256;

/// `.m` の `FandheWifiNet` と同じ配置。
#[repr(C)]
#[derive(Clone, Copy)]
struct RawNet {
    ssid: [u8; SSID_MAX],
    /// `-1` は SSID 無し（nil・32 バイト超）。
    ssid_len: i32,
    /// dBm（`0` は未取得）。
    rssi: i32,
}

const ZERO_NET: RawNet = RawNet {
    ssid: [0; SSID_MAX],
    ssid_len: -1,
    rssi: 0,
};

/// 戻りコードを契約の失敗へ写す。`0`（成功）は `None`。未知の値は `Internal`（成功へ畳まない）。
fn decode_code(rc: i32) -> Option<DeviceError> {
    match rc {
        0 => None,
        1 => Some(DeviceError::Unsupported(FEATURE)),
        2 => Some(DeviceError::PermissionDenied(FEATURE)),
        3 => Some(DeviceError::Disabled(RADIO_OFF)),
        4 => Some(DeviceError::Disabled(LOCATION_OFF)),
        _ => Some(DeviceError::Internal(READ_FAILED)),
    }
}

/// 生データ 1 件を契約の型へ変換する。`ssid_len` が `0..=32` の範囲外なら SSID は `None`
/// （範囲の検証を変換より先に行う）。
fn net_from_raw(raw: &RawNet) -> WifiNetwork {
    let ssid = usize::try_from(raw.ssid_len)
        .ok()
        .filter(|n| *n <= SSID_MAX)
        .and_then(|n| Ssid::from_bytes(&raw.ssid[..n]));
    WifiNetwork {
        ssid,
        rssi_dbm: rssi_from_dbm(raw.rssi),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use super::{RawNet, NEARBY_CAP};

    // SAFETY: `macos/fandhe_macos_wifi_info.m`（`build.rs` が macOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_wifi_info_read(
            current: *mut RawNet,
            has_current: *mut i32,
            nearby: *mut RawNet,
            cap: usize,
            nearby_len: *mut usize,
        ) -> i32;
    }

    /// `(戻りコード, 現在の接続, 周辺の生データ)`。コード 0 以外のとき他の値は無効。
    #[allow(unsafe_code)]
    pub(super) fn read() -> (i32, Option<RawNet>, Vec<RawNet>) {
        let mut current = super::ZERO_NET;
        let mut has_current = 0i32;
        let mut nearby = vec![super::ZERO_NET; NEARBY_CAP];
        let mut nearby_len = 0usize;
        // SAFETY: `current`・`has_current`・`nearby_len` は呼び出しの間有効なローカル変数への排他
        // ポインタ、`nearby` は `NEARBY_CAP` 件分の有効な排他領域。`.m` はこれらを呼び出しの外へ保持
        // せず、`cap` 件を超えて書かない。
        let rc = unsafe {
            fandhe_macos_wifi_info_read(
                &mut current,
                &mut has_current,
                nearby.as_mut_ptr(),
                nearby.len(),
                &mut nearby_len,
            )
        };
        // 範囲の検証は解釈の前に行う。cap を超える件数は壊れた応答として失敗にする。
        if nearby_len > NEARBY_CAP {
            return (5, None, Vec::new());
        }
        nearby.truncate(nearby_len);
        (rc, (has_current != 0).then_some(current), nearby)
    }
}

#[cfg(target_os = "macos")]
struct MacWifiInfo;

#[cfg(target_os = "macos")]
impl WifiInfo for MacWifiInfo {
    fn read(&self) -> Result<WifiReport, DeviceError> {
        let (rc, current, nearby) = ffi::read();
        if let Some(e) = decode_code(rc) {
            return Err(e);
        }
        Ok(WifiReport {
            current: current.as_ref().map(net_from_raw),
            nearby: normalize_nearby(nearby.iter().map(net_from_raw)),
        })
    }
}

/// macOS の `WifiInfo` 実装を返す（`device/wifi_info.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Arc<dyn WifiInfo> {
    Arc::new(MacWifiInfo)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(bytes: &[u8], len: i32, rssi: i32) -> RawNet {
        let mut r = ZERO_NET;
        r.ssid[..bytes.len()].copy_from_slice(bytes);
        r.ssid_len = len;
        r.rssi = rssi;
        r
    }

    #[test]
    fn every_code_maps_and_unknown_is_internal() {
        assert!(decode_code(0).is_none());
        assert!(matches!(decode_code(1), Some(DeviceError::Unsupported(_))));
        assert!(matches!(
            decode_code(2),
            Some(DeviceError::PermissionDenied(_))
        ));
        assert!(matches!(
            decode_code(3),
            Some(DeviceError::Disabled(RADIO_OFF))
        ));
        assert!(matches!(
            decode_code(4),
            Some(DeviceError::Disabled(LOCATION_OFF))
        ));
        for rc in [5, -1, 99, i32::MIN] {
            assert!(matches!(decode_code(rc), Some(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn invalid_length_yields_no_ssid() {
        for len in [-1, -2, 33, i32::MAX] {
            assert_eq!(net_from_raw(&raw(b"abc", len, -50)).ssid, None, "{len}");
        }
        assert_eq!(net_from_raw(&raw(b"abc", 0, -50)).ssid, None);
        let n = net_from_raw(&raw(b"abc", 3, -50));
        assert_eq!(n.ssid.unwrap().expose(), "abc");
        assert_eq!(n.rssi_dbm, Some(-50));
    }

    #[test]
    fn zero_rssi_is_unknown_and_bad_utf8_is_hidden() {
        let n = net_from_raw(&raw(&[0xff, 0xfe], 2, 0));
        assert_eq!(n.ssid, None);
        assert_eq!(n.rssi_dbm, None);
    }
}
