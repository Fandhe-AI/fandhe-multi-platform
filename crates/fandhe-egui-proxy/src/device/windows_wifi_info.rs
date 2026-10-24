//! Windows 向け Wi-Fi 情報実装（Issue #515、親 #111）。`device/wifi_info.rs::implementation` の
//! Windows 経路からワーカースレッドで呼ばれる。契約の SSOT は `fandhe_device::wifi_info`。
//!
//! # 流れ（Win32 wlanapi。COM・WinRT なし）
//!
//! 1. `WlanOpenHandle`（`ERROR_SERVICE_NOT_ACTIVE` は `Unsupported`）→ `WlanEnumInterfaces`
//!    （0 件は `Unsupported`。複数あれば接続中の最初の 1 つ、無ければ先頭だけを読む）
//! 2. `wlan_intf_opcode_radio_state` で全 PHY のソフト・ハード電源を見て、1 つもオンが無ければ
//!    `Disabled(RADIO_OFF)`（電源オフで空の一覧が返る嘘の成功を防ぐ。PHY 0 件は判定不能として読み進める）
//! 3. 接続中なら `current_connection`（SSID）と `rssi`（dBm）を読む
//! 4. `WlanGetNetworkBssList`（OS のキャッシュ。`WlanScan` は呼ばず自分からスキャンしない）→
//!    `normalize_nearby`
//!
//! # 位置情報
//!
//! Windows 11 24H2 以降は、同意なしだと 3・4 が `ERROR_ACCESS_DENIED` を返す。どちらで出ても
//! それまでの結果を捨てて `PermissionDenied` にする（未接続でも 4 で判定がかかるため、許可なしの
//! 「未接続・0 件」は出ない）。位置情報サービスのオフとアプリ単位の拒否は wlanapi 上で区別できず、
//! どちらも `PermissionDenied`（`Disabled(LOCATION_OFF)` は返さない。案内先の設定画面は同じ）。
//!
//! # 取らないもの
//!
//! BSSID はコピーも出力もしない。SSID・OS のエラー値はログ・エラーに出さない（固定の識別子へ写す）。
//! 部分成功は返さず、想定外のエラーは `Internal`。例外は切断直後の `ERROR_INVALID_STATE` で、
//! 未接続（`current = None`）として扱う。
//!
//! 写像の純関数はホストの `cargo test` で検査する。型検査は `x86_64-pc-windows-gnu` のみで、
//! 実機（24H2 の正常系・同意拒否・位置情報オフ・Wi-Fi オフ・未接続）は #510 で確かめる。

use fandhe_device::wifi_info::{rssi_from_dbm, Ssid, WifiNetwork, FEATURE, READ_FAILED};
use fandhe_device::DeviceError;

/// `ERROR_ACCESS_DENIED`。
const ACCESS_DENIED: u32 = 5;
/// `ERROR_SERVICE_NOT_ACTIVE`（WLAN AutoConfig サービスが無い・止まっている）。
const SERVICE_NOT_ACTIVE: u32 = 1062;
/// `ERROR_INVALID_STATE`（接続状態の読み出しが切断と競合した）。
const INVALID_STATE: u32 = 5023;
/// `dot11_radio_state_on`。
const RADIO_ON: i32 = 1;
/// 受け入れるインターフェース数の上限。超える値は壊れたヘッダーとみなす。
const MAX_INTERFACES: usize = 64;
/// `dot11_radio_state_off`（ソフト・ハードどちらかが明示的にオフ）。0 は unknown で、オフではない。
const RADIO_STATE_OFF: i32 = 2;
/// SSID の最大バイト数（IEEE 802.11）。
const SSID_MAX: usize = 32;

/// OS のエラーコードを契約の失敗へ写す（全呼び出し箇所で共通）。
fn classify(code: u32) -> DeviceError {
    match code {
        ACCESS_DENIED => DeviceError::PermissionDenied(FEATURE),
        _ => DeviceError::Internal(READ_FAILED),
    }
}

/// `WlanOpenHandle` のエラーを写す。サービスが無ければ非対応。
fn open_error(code: u32) -> DeviceError {
    match code {
        SERVICE_NOT_ACTIVE => DeviceError::Unsupported(FEATURE),
        _ => classify(code),
    }
}

/// `(ソフト, ハード)` の電源状態で、明示的にオフの PHY が 1 つ以上あり、かつ両方オンの PHY が
/// 無いときだけ true。unknown（0）だけ・空は判定不能として false（偽の失敗を避ける）。
pub(super) fn radio_off(states: &[(i32, i32)]) -> bool {
    let on = states.iter().any(|&(s, h)| s == RADIO_ON && h == RADIO_ON);
    let off = states
        .iter()
        .any(|&(s, h)| s == RADIO_STATE_OFF || h == RADIO_STATE_OFF);
    off && !on
}

/// 生の SSID 長・バイト列・dBm から 1 件を作る。長さの検証をスライスより先に行う。
fn net_from_parts(len: u32, ssid: &[u8; SSID_MAX], rssi: i32) -> WifiNetwork {
    let ssid = usize::try_from(len)
        .ok()
        .filter(|n| *n <= SSID_MAX)
        .and_then(|n| Ssid::from_bytes(&ssid[..n]));
    WifiNetwork {
        ssid,
        rssi_dbm: rssi_from_dbm(rssi),
    }
}

/// BSS 一覧の件数分のエントリがヘッダー込みで `dwTotalSize` に収まるか（オーバーフローは false）。
fn bss_items_fit(total_size: u32, items: u32, header: usize, entry: usize) -> bool {
    (items as usize)
        .checked_mul(entry)
        .and_then(|n| n.checked_add(header))
        .is_some_and(|need| need <= total_size as usize)
}

#[cfg(target_os = "windows")]
pub(crate) use ffi::implementation;

#[cfg(target_os = "windows")]
mod ffi {
    use std::ffi::c_void;
    use std::mem::{offset_of, size_of};
    use std::sync::Arc;

    use fandhe_device::wifi_info::{
        normalize_nearby, WifiInfo, WifiNetwork, WifiReport, FEATURE, RADIO_OFF,
    };
    use fandhe_device::DeviceError;
    use windows::core::GUID;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::NetworkManagement::WiFi::{
        dot11_BSS_type_any, wlan_interface_state_connected, wlan_intf_opcode_current_connection,
        wlan_intf_opcode_radio_state, wlan_intf_opcode_rssi, WlanCloseHandle, WlanEnumInterfaces,
        WlanFreeMemory, WlanGetNetworkBssList, WlanOpenHandle, WlanQueryInterface, WLAN_BSS_ENTRY,
        WLAN_BSS_LIST, WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO, WLAN_INTERFACE_INFO_LIST,
        WLAN_INTF_OPCODE, WLAN_RADIO_STATE,
    };

    use super::{
        bss_items_fit, classify, net_from_parts, open_error, radio_off, INVALID_STATE,
        MAX_INTERFACES,
    };

    /// Windows の `WifiInfo` 実装を返す。
    pub(crate) fn implementation() -> Arc<dyn WifiInfo> {
        Arc::new(WindowsWifiInfo)
    }

    struct WindowsWifiInfo;

    impl WifiInfo for WindowsWifiInfo {
        fn read(&self) -> Result<WifiReport, DeviceError> {
            read()
        }
    }

    /// `WlanOpenHandle` のクライアントハンドル。`Drop` で閉じ、早期 return でも漏らさない。
    struct Client(HANDLE);

    impl Client {
        #[allow(unsafe_code)]
        fn open() -> Result<Client, u32> {
            let mut ver = 0u32;
            let mut h = HANDLE::default();
            // SAFETY: `ver`・`h` は呼び出しの間有効なローカル変数への排他ポインタ。
            let rc = unsafe { WlanOpenHandle(2, None, &mut ver, &mut h) };
            if rc == 0 {
                Ok(Client(h))
            } else {
                Err(rc)
            }
        }
    }

    impl Drop for Client {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: `self.0` は `WlanOpenHandle` が返した有効なハンドルで、ここで 1 度だけ閉じる。
            unsafe { WlanCloseHandle(self.0, None) };
        }
    }

    /// wlanapi が確保したメモリ。`Drop` で `WlanFreeMemory` する。
    struct Mem(*mut c_void);

    impl Drop for Mem {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: wlanapi が返したポインタで、ここで 1 度だけ解放する。
                unsafe { WlanFreeMemory(self.0) };
            }
        }
    }

    /// `WlanQueryInterface` の戻りを `T` として複写する。返却サイズが `T` に満たなければ失敗。
    #[allow(unsafe_code)]
    fn query<T: Copy>(c: &Client, guid: &GUID, op: WLAN_INTF_OPCODE) -> Result<T, u32> {
        let mut size = 0u32;
        let mut data: *mut c_void = std::ptr::null_mut();
        // SAFETY: すべて呼び出しの間有効なローカル変数・引数への排他ポインタ。
        let rc = unsafe { WlanQueryInterface(c.0, guid, op, None, &mut size, &mut data, None) };
        let mem = Mem(data);
        if rc != 0 {
            return Err(rc);
        }
        if mem.0.is_null() || (size as usize) < size_of::<T>() {
            return Err(u32::MAX);
        }
        // SAFETY: `mem.0` は `size >= size_of::<T>()` バイトの有効な領域。配置は不定なので
        // `read_unaligned` で複写する。
        Ok(unsafe { std::ptr::read_unaligned(mem.0 as *const T) })
    }

    /// `(GUID, 接続中か)` の一覧。
    #[allow(unsafe_code)]
    fn interfaces(c: &Client) -> Result<Vec<(GUID, bool)>, u32> {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        // SAFETY: `list` は有効なローカル変数への排他ポインタ。
        let rc = unsafe { WlanEnumInterfaces(c.0, None, &mut list) };
        let _mem = Mem(list as *mut c_void);
        if rc != 0 {
            return Err(rc);
        }
        if list.is_null() {
            return Err(u32::MAX);
        }
        // SAFETY: ヘッダーの `dwNumberOfItems` は常に有効。
        let n = unsafe { (*list).dwNumberOfItems } as usize;
        // `WLAN_INTERFACE_INFO_LIST` はバイト長を持たないので、件数の上限で守る。
        if n > MAX_INTERFACES {
            return Err(u32::MAX);
        }
        // SAFETY: wlanapi が `dwNumberOfItems` 件分の `WLAN_INTERFACE_INFO` を連続して確保した領域。
        // 件数は上で `MAX_INTERFACES` 以下に検証済み。`_mem` が生きている間だけ読み、`Copy` の値として複写する。
        let infos = unsafe {
            std::slice::from_raw_parts(
                std::ptr::addr_of!((*list).InterfaceInfo) as *const WLAN_INTERFACE_INFO,
                n,
            )
        };
        Ok(infos
            .iter()
            .map(|i| (i.InterfaceGuid, i.isState == wlan_interface_state_connected))
            .collect())
    }

    /// OS キャッシュの周辺 BSS を `WifiNetwork`（BSSID なし）へ変換して返す。
    #[allow(unsafe_code)]
    fn bss_list(c: &Client, guid: &GUID) -> Result<Vec<WifiNetwork>, u32> {
        let mut list: *mut WLAN_BSS_LIST = std::ptr::null_mut();
        // SAFETY: `guid`・`list` は有効なポインタ。SSID 指定なし・BSS 種別 any でキャッシュを読むだけ。
        let rc = unsafe {
            WlanGetNetworkBssList(c.0, guid, None, dot11_BSS_type_any, false, None, &mut list)
        };
        let _mem = Mem(list as *mut c_void);
        if rc != 0 {
            return Err(rc);
        }
        if list.is_null() {
            return Err(u32::MAX);
        }
        // SAFETY: ヘッダー（`dwTotalSize`・`dwNumberOfItems`）は常に有効。
        let (total, items) = unsafe { ((*list).dwTotalSize, (*list).dwNumberOfItems) };
        if !bss_items_fit(
            total,
            items,
            offset_of!(WLAN_BSS_LIST, wlanBssEntries),
            size_of::<WLAN_BSS_ENTRY>(),
        ) {
            return Err(u32::MAX);
        }
        // SAFETY: `bss_items_fit` で `items` 件のエントリが確保領域内に収まると確認済み。`_mem` が
        // 生きている間だけ読む。
        let entries = unsafe {
            std::slice::from_raw_parts(
                std::ptr::addr_of!((*list).wlanBssEntries) as *const WLAN_BSS_ENTRY,
                items as usize,
            )
        };
        Ok(entries
            .iter()
            .map(|e| net_from_parts(e.dot11Ssid.uSSIDLength, &e.dot11Ssid.ucSSID, e.lRssi))
            .collect())
    }

    fn read() -> Result<WifiReport, DeviceError> {
        let c = Client::open().map_err(open_error)?;
        let ifaces = interfaces(&c).map_err(classify)?;
        // ponytail: 複数アダプターは 1 つだけ読む（接続中の最初、無ければ先頭）。全アダプターの合成は未対応。
        let &(guid, connected) = ifaces
            .iter()
            .find(|i| i.1)
            .or_else(|| ifaces.first())
            .ok_or(DeviceError::Unsupported(FEATURE))?;

        let radio: WLAN_RADIO_STATE =
            query(&c, &guid, wlan_intf_opcode_radio_state).map_err(classify)?;
        let n = (radio.dwNumberOfPhys as usize).min(radio.PhyRadioState.len());
        let states: Vec<(i32, i32)> = radio.PhyRadioState[..n]
            .iter()
            .map(|p| (p.dot11SoftwareRadioState.0, p.dot11HardwareRadioState.0))
            .collect();
        if radio_off(&states) {
            return Err(DeviceError::Disabled(RADIO_OFF));
        }

        let current = if connected {
            read_current(&c, &guid)?
        } else {
            None
        };
        let nearby = normalize_nearby(bss_list(&c, &guid).map_err(classify)?);
        Ok(WifiReport { current, nearby })
    }

    /// 接続中の SSID と dBm。切断と競合して `ERROR_INVALID_STATE` なら未接続（`None`）。
    fn read_current(c: &Client, guid: &GUID) -> Result<Option<WifiNetwork>, DeviceError> {
        let conn: WLAN_CONNECTION_ATTRIBUTES =
            match query(c, guid, wlan_intf_opcode_current_connection) {
                Ok(v) => v,
                Err(INVALID_STATE) => return Ok(None),
                Err(e) => return Err(classify(e)),
            };
        let rssi: i32 = match query(c, guid, wlan_intf_opcode_rssi) {
            Ok(v) => v,
            Err(INVALID_STATE) => return Ok(None),
            Err(e) => return Err(classify(e)),
        };
        let s = &conn.wlanAssociationAttributes.dot11Ssid;
        Ok(Some(net_from_parts(s.uSSIDLength, &s.ucSSID, rssi)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_mapping() {
        assert!(matches!(
            classify(5),
            DeviceError::PermissionDenied(FEATURE)
        ));
        assert!(matches!(classify(1062), DeviceError::Internal(READ_FAILED)));
        assert!(matches!(classify(0), DeviceError::Internal(READ_FAILED)));
        assert!(matches!(
            open_error(1062),
            DeviceError::Unsupported(FEATURE)
        ));
        assert!(matches!(
            open_error(5),
            DeviceError::PermissionDenied(FEATURE)
        ));
        assert!(matches!(open_error(87), DeviceError::Internal(READ_FAILED)));
    }

    #[test]
    fn radio_states() {
        assert!(radio_off(&[(2, 1), (1, 2)]));
        assert!(!radio_off(&[(0, 1), (1, 0)]));
        assert!(!radio_off(&[(0, 0)]));
        assert!(radio_off(&[(2, 1), (0, 0)]));
        assert!(radio_off(&[(1, 2)]));
        assert!(!radio_off(&[(0, 0), (1, 1)]));
        assert!(!radio_off(&[(2, 2), (1, 1)]));
        assert!(!radio_off(&[]));
    }

    #[test]
    fn net_parts_validates_len_and_rssi() {
        let mut b = [0u8; SSID_MAX];
        b[..4].copy_from_slice(b"home");
        let n = net_from_parts(4, &b, -50);
        assert_eq!(n.ssid.unwrap().expose(), "home");
        assert_eq!(n.rssi_dbm, Some(-50));
        for len in [0, 33, u32::MAX] {
            assert!(net_from_parts(len, &b, -50).ssid.is_none(), "{len}");
        }
        assert_eq!(net_from_parts(4, &b, 0).rssi_dbm, None);
        assert_eq!(net_from_parts(4, &b, -127).rssi_dbm, None);
        let mut bad = [0u8; SSID_MAX];
        bad[0] = 0xff;
        assert!(net_from_parts(1, &bad, -50).ssid.is_none());
    }

    #[test]
    fn bss_fit_bounds() {
        assert!(bss_items_fit(100, 0, 8, 10));
        assert!(bss_items_fit(98, 9, 8, 10));
        assert!(!bss_items_fit(97, 9, 8, 10));
        assert!(!bss_items_fit(u32::MAX, u32::MAX, 8, usize::MAX));
    }
}
