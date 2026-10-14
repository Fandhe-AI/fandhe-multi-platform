//! macOS / iOS 向けネットワーク接続状態実装（Issue #1282、iOS は #1283）。
//! `device/network_status.rs::implementation` の macOS 経路と、iOS 経路
//! （`ios_network_status.rs` が返す）から呼ばれ、`apple/fandhe_apple_network_status.m`
//! （`NWPathMonitor`、`build.rs` が feature `device` 有効時にコンパイル）を C ABI で呼ぶ。
//! `unsafe` は引数なし・戻り値 int のみの FFI 1 箇所と、`.m` から呼ばれる受け口 1 つに閉じる。
//!
//! # 再描画（契約表 4）
//!
//! `NWPathMonitor` の update handler は値（atomic）を更新した直後に
//! [`fandhe_rust_network_status_changed`] を呼ぶ。iOS ではここで `crate::ios::request_repaint`
//! を呼び次フレームを起こす（入力が無いと egui が描画せず表示が止まるため）。呼び出しは path
//! 変化時だけで、再描画の嵐にはならない。macOS は event loop 側に再描画の経路があるため何もしない。
//! 読み取りは `dispatch_once` と atomic load だけでスレッド安全かつ UI を要しないので、
//! `is_main_thread()` ガードは置かない（置くと偽の失敗を生む経路が増える）。
//!
//! 6 の嘘の成功: 未知のビット値・範囲外・負値は `Unknown`。接続なし・不明のとき種別・
//! 従量制のビットは無視する。値をログへ出さない。
//! 既知の天井: 起動直後の 1 回目は handler 未到達で「不明」になりうる。

// ホストの test ビルドでは純関数がテストからのみ使われる。
#![cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]

#[cfg(any(target_os = "macos", target_os = "ios"))]
use fandhe_device::network_status::NetworkStatus;
use fandhe_device::network_status::{ConnectionKind, NetworkSnapshot};
#[cfg(any(target_os = "macos", target_os = "ios"))]
use fandhe_device::DeviceError;

/// `.m` が詰めた値を契約の値へ戻す純関数（ホストの `cargo test` で検査する）。
fn snapshot_from_packed(packed: i32) -> NetworkSnapshot {
    if packed < 0 {
        return NetworkSnapshot::Unknown;
    }
    match packed & 0b11 {
        1 => {}
        2 => return NetworkSnapshot::Offline,
        _ => return NetworkSnapshot::Unknown,
    }
    let kind = match (packed >> 2) & 0b111 {
        0 => ConnectionKind::Unknown,
        1 => ConnectionKind::Wifi,
        2 => ConnectionKind::Cellular,
        3 => ConnectionKind::Ethernet,
        4 => ConnectionKind::Other,
        _ => return NetworkSnapshot::Unknown,
    };
    let metered = match (packed >> 5) & 0b11 {
        0 => None,
        1 => Some(false),
        2 => Some(true),
        _ => return NetworkSnapshot::Unknown,
    };
    if packed >> 7 != 0 {
        return NetworkSnapshot::Unknown;
    }
    NetworkSnapshot::Online { kind, metered }
}

// SAFETY: `apple/fandhe_apple_network_status.m` が定義する C ABI。引数なし・戻り値は int のみで
// 所有権の移転もポインタの受け渡しも無い。
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_network_status_read() -> i32;
}

/// `NWPathMonitor` による実装。状態は `.m` 側の static が持つ。
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) struct AppleNetworkStatus;

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl NetworkStatus for AppleNetworkStatus {
    #[allow(unsafe_code)]
    fn read(&self) -> Result<NetworkSnapshot, DeviceError> {
        // SAFETY: 上記 extern 宣言のとおり、副作用は `.m` 内の static 初期化と atomic 読み取りのみ。
        let packed = unsafe { fandhe_apple_network_status_read() };
        Ok(snapshot_from_packed(packed))
    }
}

/// `.m` の update handler が値を確定（atomic store）した直後に呼ぶ受け口。引数なし・
/// ポインタなしで、ObjC の serial queue から呼ばれる。`request_repaint` は Mutex を取るだけで
/// `dispatch_sync` はしない（デッドロックしない、契約表 11）。panic しない。
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_network_status_changed() {
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_values() {
        assert_eq!(snapshot_from_packed(0), NetworkSnapshot::Unknown);
        assert_eq!(
            snapshot_from_packed(1 | (1 << 2) | (2 << 5)),
            NetworkSnapshot::Online {
                kind: ConnectionKind::Wifi,
                metered: Some(true)
            }
        );
        // 接続なしでは種別ビットを無視する。
        assert_eq!(
            snapshot_from_packed(2 | (1 << 2) | (2 << 5)),
            NetworkSnapshot::Offline
        );
    }

    #[test]
    fn out_of_range_is_unknown() {
        assert_eq!(snapshot_from_packed(-1), NetworkSnapshot::Unknown);
        assert_eq!(snapshot_from_packed(3), NetworkSnapshot::Unknown);
        assert_eq!(snapshot_from_packed(1 | (5 << 2)), NetworkSnapshot::Unknown);
        assert_eq!(snapshot_from_packed(1 | (3 << 5)), NetworkSnapshot::Unknown);
        assert_eq!(snapshot_from_packed(1 | (1 << 7)), NetworkSnapshot::Unknown);
    }
}
