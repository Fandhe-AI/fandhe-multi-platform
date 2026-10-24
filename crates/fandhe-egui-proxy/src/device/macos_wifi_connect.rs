//! macOS 向け「指定した Wi-Fi への接続」実装（Issue #791、親 #165）。
//! `macos/fandhe_macos_wifi_connect.m`（CoreWLAN の directed scan と
//! `associateToNetwork:password:error:`、CoreLocation の許可要求。`build.rs` が macOS 分岐で
//! feature `device` 有効時にコンパイルし CoreWLAN をリンクする）を `unsafe extern "C"` 1 関数で呼ぶ。
//! `unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。`device/wifi_connect.rs` の macOS
//! 経路からワーカースレッドで呼ばれ、契約は `fandhe_device::wifi_connect`。
//!
//! 戻りコードの写像 [`decode_code`] は純関数で、ホストの `cargo test` で検査する。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: SSID のバイト一致を確かめてから接続する（別のネットワークへつながない）、見つからない・
//!   許可なし・電源オフを成功にしない、未知の戻りコードは成功にせず `Internal`、SSID・パスワード・
//!   `NSError` をログに出さない
//! - 守らない: OS 呼び出しが戻らない場合、管理者パスワードのダイアログの待ち時間、接続した後の変化、
//!   メモリ上のパスワードの消去
//!
//! # 未確認事項（Mac 目視 #782 で確認）
//!
//! - 素の実行体（`Info.plist` 埋め込み・非バンドル）で directed scan の `ssidData` が取れるか
//! - 管理者パスワードのダイアログが出る条件と、取り消したときの `NSError`（識別できれば三値に分ける）
//! - WPA3 での動作
//!
//! 実物の CoreWLAN はホストのテストから呼ばない（接続状態を変えてしまうため）。

#[cfg(target_os = "macos")]
use std::sync::Arc;

#[cfg(target_os = "macos")]
use fandhe_device::wifi_connect::WifiConnect;
use fandhe_device::wifi_connect::{
    CONNECT_FAILED, FEATURE, NOT_FOUND, RADIO_OFF, UNSUPPORTED_SECURITY,
};
use fandhe_device::DeviceError;

/// `.m` の戻りコードを契約の結果へ写す。未知の値は `Internal`（成功へ畳まない）。
fn decode_code(rc: i32) -> Result<(), DeviceError> {
    match rc {
        0 => Ok(()),
        1 => Err(DeviceError::Unsupported(FEATURE)),
        2 => Err(DeviceError::PermissionDenied(FEATURE)),
        3 => Err(DeviceError::Disabled(RADIO_OFF)),
        4 => Err(DeviceError::Internal(NOT_FOUND)),
        6 => Err(DeviceError::Internal(UNSUPPORTED_SECURITY)),
        _ => Err(DeviceError::Internal(CONNECT_FAILED)),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_wifi_connect.m`（`build.rs` が macOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_wifi_connect(
            ssid: *const u8,
            ssid_len: usize,
            password: *const u8,
            password_len: usize,
        ) -> i32;
    }

    /// 接続を要求して戻りコードを返す。`password` が空なら暗号なし。
    #[allow(unsafe_code)]
    pub(super) fn connect(ssid: &[u8], password: &[u8]) -> i32 {
        // SAFETY: `ssid`・`password` は呼び出しの間有効な読み取り専用スライスで、長さを一緒に渡す。
        // `.m` はこれらを呼び出しの外へ保持せず、書き込まない。空スライスのポインタも非 NULL。
        unsafe {
            fandhe_macos_wifi_connect(ssid.as_ptr(), ssid.len(), password.as_ptr(), password.len())
        }
    }
}

#[cfg(target_os = "macos")]
struct MacWifiConnect;

#[cfg(target_os = "macos")]
impl WifiConnect for MacWifiConnect {
    fn connect(
        &self,
        request: &fandhe_device::wifi_connect::ConnectRequest,
    ) -> Result<(), DeviceError> {
        let password = request
            .password
            .as_ref()
            .map_or(&[][..], |p| p.expose().as_bytes());
        decode_code(ffi::connect(request.ssid.as_bytes(), password))
    }
}

/// macOS の `WifiConnect` 実装を返す（`device/wifi_connect.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Arc<dyn WifiConnect> {
    Arc::new(MacWifiConnect)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_maps_and_unknown_is_internal() {
        assert!(decode_code(0).is_ok());
        assert!(matches!(decode_code(1), Err(DeviceError::Unsupported(_))));
        assert!(matches!(
            decode_code(2),
            Err(DeviceError::PermissionDenied(_))
        ));
        assert!(matches!(
            decode_code(3),
            Err(DeviceError::Disabled(RADIO_OFF))
        ));
        assert!(matches!(
            decode_code(4),
            Err(DeviceError::Internal(NOT_FOUND))
        ));
        assert!(matches!(
            decode_code(5),
            Err(DeviceError::Internal(CONNECT_FAILED))
        ));
        assert!(matches!(
            decode_code(6),
            Err(DeviceError::Internal(UNSUPPORTED_SECURITY))
        ));
        assert!(matches!(
            decode_code(7),
            Err(DeviceError::Internal(CONNECT_FAILED))
        ));
        for rc in [-1, 8, 99, i32::MIN] {
            assert!(matches!(decode_code(rc), Err(DeviceError::Internal(_))));
        }
    }
}
