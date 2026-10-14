//! Windows 向けネットワーク接続状態実装（Issue #1282、親 #1130）。
//! `device/network_status.rs::implementation` の Windows 経路から呼ばれ、WinRT
//! `Windows.Networking.Connectivity.NetworkInformation` を UI スレッドで同期に読む
//! （1 秒に 1 回。変化通知 `NetworkStatusChanged` は購読しない）。
//!
//! 写像は `windows` クレートの型に依存しない純関数に切り出し、ホストの `cargo test` で
//! 検査する（WinRT 呼び出しだけ内側の `cfg(target_os = "windows")` に閉じる。
//! `windows_url_scheme.rs` と同じ方針）。`unsafe` と `RoInitialize` は使わない
//! （`windows_text_scale.rs` と同じ前提）。HRESULT・インターフェース情報はログにも画面にも
//! 出さず固定文言へ畳む（security.md A09）。
//!
//! - プロファイル無し: windows-core 0.61 は null を `Err(Error::empty())`（code = 0）で返す。
//!   code 0 は「接続なし」、それ以外の失敗は `Internal`（null と実失敗を混同しない）
//! - 接続: `LocalAccess` / `ConstrainedInternetAccess` / `InternetAccess` は接続あり（契約は
//!   「通信経路がある」でインターネット到達性は保証しない）、`None` のみ接続なし。
//!   インターネット用プロファイルが null のときは `GetConnectionProfiles` のうち
//!   経路を持つもの（LocalAccess 以上）を探し、無ければ接続なし
//! - 種別: Wlan → Wi-Fi、Wwan → 携帯、アダプタの IANA 型 6 → 有線。判定に要る API が
//!   1 つでも失敗したら不明、全て成功して該当無しのときだけその他
//! - 従量制: `NetworkCostType` が Unrestricted → いいえ、Fixed / Variable → はい、Unknown → 不明
//!
//! 既知の天井: unpackaged exe での動作は実機未確認（#1273）。ローカルに MSVC が無く
//! 型検査は `x86_64-pc-windows-gnu` のみ。

use fandhe_device::network_status::ConnectionKind;
#[cfg(target_os = "windows")]
use fandhe_device::network_status::{NetworkSnapshot, NetworkStatus};
#[cfg(target_os = "windows")]
use fandhe_device::DeviceError;

/// `GetInternetConnectionProfile` の失敗が「プロファイル無し（null）」か。
fn profile_absent(code: i32) -> bool {
    code == 0
}

/// `NetworkConnectivityLevel` の値から通信経路ありかを決める。LocalAccess(1) /
/// ConstrainedInternetAccess(2) / InternetAccess(3) は経路あり（LAN のみでも接続あり）。
/// None(0) と未知の値は接続なし。
fn connection_from_level(level: i32) -> bool {
    matches!(level, 1..=3)
}

/// IANA ifType 6 = ethernetCsmacd。
const IANA_ETHERNET: u32 = 6;

/// 種別を決める。各引数は API 失敗を `None` で表す。Wlan / Wwan が優先、次にアダプタの
/// IANA 型。真と判った種別を返せなかった場合に、失敗が 1 つでもあれば不明、無ければその他。
fn kind_from(is_wlan: Option<bool>, is_wwan: Option<bool>, iana: Option<u32>) -> ConnectionKind {
    if is_wlan == Some(true) {
        ConnectionKind::Wifi
    } else if is_wwan == Some(true) {
        ConnectionKind::Cellular
    } else if iana == Some(IANA_ETHERNET) {
        ConnectionKind::Ethernet
    } else if is_wlan.is_none() || is_wwan.is_none() || iana.is_none() {
        ConnectionKind::Unknown
    } else {
        ConnectionKind::Other
    }
}

/// `NetworkCostType` の値（Unknown = 0 / Unrestricted = 1 / Fixed = 2 / Variable = 3）から
/// 従量制かを決める。
fn metered_from_cost(cost: i32) -> Option<bool> {
    match cost {
        1 => Some(false),
        2 | 3 => Some(true),
        _ => None,
    }
}

/// [`NetworkStatus`] の Windows 実装。状態を持たない。
#[cfg(target_os = "windows")]
pub(crate) struct WindowsNetworkStatus;

#[cfg(target_os = "windows")]
impl NetworkStatus for WindowsNetworkStatus {
    fn read(&self) -> Result<NetworkSnapshot, DeviceError> {
        use windows::Networking::Connectivity::NetworkInformation;

        let profile = match NetworkInformation::GetInternetConnectionProfile() {
            Ok(p) => p,
            Err(e) if profile_absent(e.code().0) => {
                // インターネット用が無くても、LAN のみの経路があれば接続あり。
                // 接続レベルを読めなかったプロファイルがあれば、経路が無いとは言えないので
                // 「接続なし」と断定せず失敗にする（取得失敗を接続なしへ畳まない）。
                let list = NetworkInformation::GetConnectionProfiles()
                    .map_err(|_| DeviceError::Internal("network_status profiles"))?;
                let mut level_failed = false;
                let mut found = None;
                for p in list {
                    match p.GetNetworkConnectivityLevel() {
                        Ok(l) if connection_from_level(l.0) => {
                            found = Some(p);
                            break;
                        }
                        Ok(_) => {}
                        Err(_) => level_failed = true,
                    }
                }
                match found {
                    Some(p) => p,
                    None if level_failed => {
                        return Err(DeviceError::Internal("network_status level"))
                    }
                    None => return Ok(NetworkSnapshot::Offline),
                }
            }
            Err(_) => return Err(DeviceError::Internal("network_status profile")),
        };
        let level = profile
            .GetNetworkConnectivityLevel()
            .map_err(|_| DeviceError::Internal("network_status level"))?;
        if !connection_from_level(level.0) {
            return Ok(NetworkSnapshot::Offline);
        }
        // 種別・従量制は取れなくても接続ありの判定は変えない（不明へ倒す）。
        let is_wlan = profile.IsWlanConnectionProfile().ok();
        let is_wwan = profile.IsWwanConnectionProfile().ok();
        let iana = profile
            .NetworkAdapter()
            .and_then(|a| a.IanaInterfaceType())
            .ok();
        let metered = profile
            .GetConnectionCost()
            .and_then(|c| c.NetworkCostType())
            .map(|t| metered_from_cost(t.0))
            .unwrap_or(None);
        Ok(NetworkSnapshot::Online {
            kind: kind_from(is_wlan, is_wwan, iana),
            metered,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_profile_is_distinguished_from_failure() {
        assert!(profile_absent(0));
        assert!(!profile_absent(0x8007_0005_u32 as i32));
    }

    #[test]
    fn level_none_is_offline() {
        assert!(!connection_from_level(0));
        assert!(connection_from_level(1));
        assert!(connection_from_level(2));
        assert!(connection_from_level(3));
        assert!(!connection_from_level(99));
    }

    #[test]
    fn cost_mapping() {
        assert_eq!(metered_from_cost(0), None);
        assert_eq!(metered_from_cost(1), Some(false));
        assert_eq!(metered_from_cost(2), Some(true));
        assert_eq!(metered_from_cost(3), Some(true));
        assert_eq!(metered_from_cost(99), None);
    }

    #[test]
    fn kind_priority() {
        let (t, f) = (Some(true), Some(false));
        assert_eq!(kind_from(t, t, Some(6)), ConnectionKind::Wifi);
        assert_eq!(kind_from(f, t, Some(6)), ConnectionKind::Cellular);
        assert_eq!(kind_from(f, f, Some(6)), ConnectionKind::Ethernet);
        assert_eq!(kind_from(f, f, Some(71)), ConnectionKind::Other);
        // API 失敗は「その他」ではなく不明。
        assert_eq!(kind_from(f, f, None), ConnectionKind::Unknown);
        assert_eq!(kind_from(None, f, Some(71)), ConnectionKind::Unknown);
        assert_eq!(kind_from(f, None, Some(71)), ConnectionKind::Unknown);
    }
}
