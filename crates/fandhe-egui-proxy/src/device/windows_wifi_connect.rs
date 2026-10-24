//! Windows 向け「指定した Wi-Fi への接続」実装（Issue #792、親 #165）。`device/wifi_connect.rs` の
//! dispatch（Windows 経路）からワーカースレッドで呼ばれる。契約の SSOT は
//! `fandhe_device::wifi_connect`（「OS ごとの写像」節）。
//!
//! # 流れ（Win32 wlanapi。COM・WinRT なし）
//!
//! 1. `WlanOpenHandle`（`ERROR_SERVICE_NOT_ACTIVE` は `Unsupported`）→ `WlanEnumInterfaces`
//!    （0 件は `Unsupported`。複数あれば接続中の最初、無ければ先頭だけを使う）
//! 2. 待ち合わせの context（対象の interface GUID・SSID・結果の 1 スロット）を作り、
//!    `WlanRegisterNotification` で ACM の通知だけを登録する（MSM は `wiFiControl` が要るため登録しない）
//! 3. 一時プロファイル（`wlan_connection_mode_temporary_profile`）で `WlanConnect`。戻り値 0 は
//!    「要求を受け付けた」だけで、成功にしない。`connection_complete` の通知を待つ
//! 4. 通知を [`match_notification`] で照合する。`connection_complete` かつ reason が成功のときだけ成功
//! 5. 片付けは全経路で「登録解除（実行中のコールバックの終了を待つ）→ context の解放 →
//!    ハンドルを閉じる」の順（`ConnectGuard` の `Drop`）
//!
//! 失敗したときだけ電源状態を読み直し、オフなら `Disabled(RADIO_OFF)` に分ける（電源の判定は
//! `windows_wifi_info::radio_off` を共有）。読み直しに失敗したら元の失敗をそのまま返す。
//!
//! # プロファイル
//!
//! SSID は 16 進で書き（注入できない）、パスワードは XML エスケープして `keyMaterial` に入れる
//! （`protected=false` の平文は OS 仕様で、一時プロファイルは保存されない）。扱うのは WPA2-Personal
//! （AES）と暗号なしだけ。XML・パスワードはログにも `Debug` にも出さない。
//!
//! # 守る範囲と守らない範囲
//!
//! 照合は interface GUID・SSID・プロファイル種別（一時）まで。他のアプリが同じアダプターへ同じ
//! SSID を同時に要求した場合の取り違え、複数アダプター、複数プロセスは守らない（契約のとおり）。
//! 期限は持たない。ただしアダプターが外れて完了通知が来なくなる経路は `interface_removal`
//! の通知で失敗として終え、ワーカーと登録を残さない。
//!
//! Windows では `UNSUPPORTED_SECURITY` を返さない。WPA3 だけのネットワークなど扱わない方式は
//! OS の reason code では区別せず `CONNECT_FAILED` に入る。reason code・HRESULT は写像にだけ使い、
//! ログ・エラーに出さない。位置情報の同意は不要の見込み（24H2 の同意対象の一覧に `WlanConnect` が無い）。
//!
//! 写像の純関数はホストの `cargo test` で検査する。型検査は `x86_64-pc-windows-gnu` のみで、
//! 実機（WPA2・暗号なし・パスワード違い・存在しない SSID・Wi-Fi オフ・WPA3 のみ・アダプター取り外し）は
//! #782 で確かめる。

use std::sync::{Condvar, Mutex};

use fandhe_device::wifi_connect::{
    ConnectRequest, CONNECT_FAILED, FEATURE, NOT_FOUND, RADIO_OFF, SSID_MAX_BYTES,
};
use fandhe_device::DeviceError;

/// `ERROR_ACCESS_DENIED`。
const ACCESS_DENIED: u32 = 5;
/// `ERROR_SERVICE_NOT_ACTIVE`（WLAN AutoConfig サービスが無い・止まっている）。
const SERVICE_NOT_ACTIVE: u32 = 1062;
/// `WLAN_NOTIFICATION_SOURCE_ACM`。
const SOURCE_ACM: u32 = 8;
/// `wlan_notification_acm_connection_complete`。
const ACM_CONNECTION_COMPLETE: u32 = 10;
/// `wlan_notification_acm_connection_attempt_fail`。
const ACM_ATTEMPT_FAIL: u32 = 11;
/// `wlan_notification_acm_interface_removal`。
const ACM_INTERFACE_REMOVAL: u32 = 14;
/// `wlan_connection_mode_temporary_profile`。
const MODE_TEMPORARY_PROFILE: i32 = 1;
/// `WLAN_REASON_CODE_SUCCESS`。
const REASON_SUCCESS: u32 = 0;
/// `WLAN_REASON_CODE_NETWORK_NOT_AVAILABLE`。
const REASON_NETWORK_NOT_AVAILABLE: u32 = 0x2800B;
/// `WLAN_REASON_CODE_NOT_VISIBLE`。
const REASON_NOT_VISIBLE: u32 = 0x28002;
/// 一時プロファイルの名前。SSID を含めない固定名。
const PROFILE_NAME: &str = "fandhe-wifi-connect";

/// 通知から決まる 1 回の要求の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// `connection_complete` で reason が成功。
    Success,
    /// 指定の SSID が見えない・利用できない。
    NotFound,
    /// 上記以外の失敗（電源オフかは呼び出し側が読み直して分ける）。
    Failed,
}

/// 同期エラーを含む失敗の分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    Denied,
    NotFound,
    Other,
}

/// 失敗を契約の `DeviceError` へ写す（全経路で共通）。`radio_off` は失敗後に読み直した電源状態。
fn to_error(failure: Failure, radio_off: bool) -> DeviceError {
    match failure {
        Failure::Denied => DeviceError::PermissionDenied(FEATURE),
        Failure::NotFound => DeviceError::Internal(NOT_FOUND),
        Failure::Other if radio_off => DeviceError::Disabled(RADIO_OFF),
        Failure::Other => DeviceError::Internal(CONNECT_FAILED),
    }
}

/// `WlanConnect` / `WlanOpenHandle` などが同期で返したエラーコードの分類。
fn classify_sync(code: u32) -> Failure {
    match code {
        ACCESS_DENIED => Failure::Denied,
        _ => Failure::Other,
    }
}

/// 通知の reason code から結果を決める。
fn outcome_from_reason(reason: u32) -> Outcome {
    match reason {
        REASON_SUCCESS => Outcome::Success,
        REASON_NETWORK_NOT_AVAILABLE | REASON_NOT_VISIBLE => Outcome::NotFound,
        _ => Outcome::Failed,
    }
}

/// `WLAN_CONNECTION_NOTIFICATION_DATA` のうち照合に使う部分の複写。
#[derive(Clone, Copy)]
struct ConnData {
    mode: i32,
    ssid_len: u32,
    ssid: [u8; SSID_MAX_BYTES],
    reason: u32,
}

/// 通知 1 件がこの要求の終端なら結果を返す。対象外・不正な通知は `None`。
fn match_notification(
    source: u32,
    code: u32,
    same_interface: bool,
    data: Option<&ConnData>,
    want_ssid: &[u8],
) -> Option<Outcome> {
    if source != SOURCE_ACM || !same_interface {
        return None;
    }
    if code == ACM_INTERFACE_REMOVAL {
        return Some(Outcome::Failed);
    }
    if code != ACM_CONNECTION_COMPLETE && code != ACM_ATTEMPT_FAIL {
        return None;
    }
    let d = data?;
    let len = usize::try_from(d.ssid_len)
        .ok()
        .filter(|n| *n <= SSID_MAX_BYTES)?;
    if d.mode != MODE_TEMPORARY_PROFILE || d.ssid[..len] != *want_ssid {
        return None;
    }
    let outcome = outcome_from_reason(d.reason);
    // 成功は connection_complete のみ。attempt_fail が成功 reason を持っても成功にしない。
    if outcome == Outcome::Success && code != ACM_CONNECTION_COMPLETE {
        return Some(Outcome::Failed);
    }
    Some(outcome)
}

/// 終端を一度だけ受ける 1 スロット。コールバック（OS スレッド）が確定し、ワーカーが待つ。
struct Slot {
    state: Mutex<Option<Outcome>>,
    cv: Condvar,
}

impl Slot {
    fn new() -> Self {
        Slot {
            state: Mutex::new(None),
            cv: Condvar::new(),
        }
    }

    /// 最初の終端だけを採用する（2 回目以降は捨てる）。確定させてから起こす。
    fn finish(&self, outcome: Outcome) {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Some(outcome);
        }
        drop(g);
        self.cv.notify_all();
    }

    /// 終端が確定するまでブロックする（期限なし。契約どおり）。
    fn wait(&self) -> Outcome {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(o) = *g {
                return o;
            }
            g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// `& < > " '` を実体参照にする。trim しない。
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// 大文字の 16 進。
fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// 一時プロファイルの XML。SSID は 16 進、パスワードは XML エスケープ。出力・ログに使わない。
fn profile_xml(req: &ConnectRequest) -> String {
    let security = match &req.password {
        None => "<authEncryption><authentication>open</authentication>\
                 <encryption>none</encryption><useOneX>false</useOneX></authEncryption>"
            .to_string(),
        Some(pw) => {
            let key = pw.expose();
            let hex_key = key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit());
            let key_type = if hex_key { "networkKey" } else { "passPhrase" };
            format!(
                "<authEncryption><authentication>WPA2PSK</authentication>\
                 <encryption>AES</encryption><useOneX>false</useOneX></authEncryption>\
                 <sharedKey><keyType>{key_type}</keyType><protected>false</protected>\
                 <keyMaterial>{}</keyMaterial></sharedKey>",
                xml_escape(key)
            )
        }
    };
    format!(
        "<?xml version=\"1.0\"?>\
         <WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\">\
         <name>{PROFILE_NAME}</name>\
         <SSIDConfig><SSID><hex>{}</hex></SSID></SSIDConfig>\
         <connectionType>ESS</connectionType><connectionMode>manual</connectionMode>\
         <MSM><security>{security}</security></MSM></WLANProfile>",
        hex_upper(req.ssid.as_bytes())
    )
}

#[cfg(target_os = "windows")]
pub(crate) use ffi::implementation;

#[cfg(target_os = "windows")]
mod ffi {
    use std::ffi::c_void;
    use std::mem::{offset_of, size_of};
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::Arc;

    use fandhe_device::wifi_connect::{ConnectRequest, WifiConnect, FEATURE, SSID_MAX_BYTES};
    use fandhe_device::DeviceError;
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::NetworkManagement::WiFi::{
        dot11_BSS_type_infrastructure, wlan_connection_mode_temporary_profile,
        wlan_interface_state_connected, wlan_intf_opcode_radio_state,
        wlan_notification_acm_connection_attempt_fail, wlan_notification_acm_connection_complete,
        wlan_notification_acm_interface_removal, WlanCloseHandle, WlanConnect, WlanEnumInterfaces,
        WlanFreeMemory, WlanOpenHandle, WlanQueryInterface, WlanRegisterNotification, DOT11_SSID,
        L2_NOTIFICATION_DATA, WLAN_CONNECTION_NOTIFICATION_DATA, WLAN_CONNECTION_PARAMETERS,
        WLAN_INTERFACE_INFO, WLAN_INTERFACE_INFO_LIST, WLAN_NOTIFICATION_SOURCE_ACM,
        WLAN_NOTIFICATION_SOURCE_NONE, WLAN_RADIO_STATE, WLAN_REASON_CODE_NETWORK_NOT_AVAILABLE,
        WLAN_REASON_CODE_NOT_VISIBLE, WLAN_REASON_CODE_SUCCESS,
    };

    use super::super::windows_wifi_info::radio_off;
    use super::{
        classify_sync, match_notification, profile_xml, to_error, ConnData, Failure, Outcome, Slot,
        ACM_ATTEMPT_FAIL, ACM_CONNECTION_COMPLETE, ACM_INTERFACE_REMOVAL, MODE_TEMPORARY_PROFILE,
        REASON_NETWORK_NOT_AVAILABLE, REASON_NOT_VISIBLE, REASON_SUCCESS, SERVICE_NOT_ACTIVE,
        SOURCE_ACM,
    };

    // 純関数側の定数が windows クレートの値と一致していることをコンパイル時に固定する。
    const _: () = {
        assert!(WLAN_NOTIFICATION_SOURCE_ACM.0 == SOURCE_ACM);
        assert!(wlan_notification_acm_connection_complete.0 as u32 == ACM_CONNECTION_COMPLETE);
        assert!(wlan_notification_acm_connection_attempt_fail.0 as u32 == ACM_ATTEMPT_FAIL);
        assert!(wlan_notification_acm_interface_removal.0 as u32 == ACM_INTERFACE_REMOVAL);
        assert!(wlan_connection_mode_temporary_profile.0 == MODE_TEMPORARY_PROFILE);
        assert!(WLAN_REASON_CODE_SUCCESS == REASON_SUCCESS);
        assert!(WLAN_REASON_CODE_NETWORK_NOT_AVAILABLE == REASON_NETWORK_NOT_AVAILABLE);
        assert!(WLAN_REASON_CODE_NOT_VISIBLE == REASON_NOT_VISIBLE);
    };

    /// 受け入れるインターフェース数の上限。超える値は壊れたヘッダーとみなす。
    const MAX_INTERFACES: usize = 64;

    /// Windows の `WifiConnect` 実装を返す。
    pub(crate) fn implementation() -> Arc<dyn WifiConnect> {
        Arc::new(WindowsWifiConnect)
    }

    struct WindowsWifiConnect;

    impl WifiConnect for WindowsWifiConnect {
        fn connect(&self, request: &ConnectRequest) -> Result<(), DeviceError> {
            connect(request)
        }
    }

    /// `WlanOpenHandle` のクライアントハンドル。`Drop` で閉じる。
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

    /// 通知コールバックが参照する context。登録の解除が済むまで解放しない。
    struct Shared {
        guid: GUID,
        ssid: Vec<u8>,
        slot: Slot,
    }

    /// 片付けの順序を固定する RAII。`Drop` で「登録解除 → context の解放」を行い、
    /// その後にフィールド `client` が drop されてハンドルが閉じる。
    struct ConnectGuard {
        ctx: *const Shared,
        registered: bool,
        client: Client,
    }

    impl Drop for ConnectGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            if self.registered {
                // SAFETY: 有効なハンドル。解除は実行中のコールバックの終了を待つ（公式）ため、
                // これ以降 `ctx` を参照するコールバックは走らない。
                unsafe {
                    WlanRegisterNotification(
                        self.client.0,
                        WLAN_NOTIFICATION_SOURCE_NONE,
                        true,
                        None,
                        None,
                        None,
                        None,
                    )
                };
            }
            if !self.ctx.is_null() {
                // SAFETY: `Arc::into_raw` で作った strong を、解除後にここで 1 度だけ戻して解放する。
                drop(unsafe { Arc::from_raw(self.ctx) });
            }
        }
    }

    /// OS スレッドから呼ばれる ACM 通知コールバック。パニックを境界の外へ出さない。
    #[allow(unsafe_code)]
    unsafe extern "system" fn callback(data: *mut L2_NOTIFICATION_DATA, ctx: *mut c_void) {
        if data.is_null() || ctx.is_null() {
            return;
        }
        let _ = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: `ctx` は登録時の `Arc::into_raw` で、解除が完了するまで解放されない。
            let shared = unsafe { &*(ctx as *const Shared) };
            // SAFETY: OS がコールバックの間有効な通知データを渡す。ヘッダーは `Copy` で複写する。
            let n = unsafe { std::ptr::read_unaligned(data) };
            let conn = parse_conn_data(&n);
            if let Some(o) = match_notification(
                n.NotificationSource.0,
                n.NotificationCode,
                n.InterfaceGuid == shared.guid,
                conn.as_ref(),
                &shared.ssid,
            ) {
                shared.slot.finish(o);
            }
        }));
    }

    /// 通知の `pData` を長さ検証のうえ複写する。足りなければ `None`。
    #[allow(unsafe_code)]
    fn parse_conn_data(n: &L2_NOTIFICATION_DATA) -> Option<ConnData> {
        let need = offset_of!(WLAN_CONNECTION_NOTIFICATION_DATA, strProfileXml);
        if n.pData.is_null() || (n.dwDataSize as usize) < need {
            return None;
        }
        let base = n.pData as *const u8;
        // SAFETY: `pData` は `dwDataSize >= need` バイトの有効な領域で、`strProfileXml` より前の
        // フィールドだけを読む。配置は不定なので各フィールドを `read_unaligned` で複写する。
        unsafe {
            let mode = std::ptr::read_unaligned(base.add(offset_of!(
                WLAN_CONNECTION_NOTIFICATION_DATA,
                wlanConnectionMode
            )) as *const i32);
            let ssid = std::ptr::read_unaligned(
                base.add(offset_of!(WLAN_CONNECTION_NOTIFICATION_DATA, dot11Ssid))
                    as *const DOT11_SSID,
            );
            let reason = std::ptr::read_unaligned(base.add(offset_of!(
                WLAN_CONNECTION_NOTIFICATION_DATA,
                wlanReasonCode
            )) as *const u32);
            Some(ConnData {
                mode,
                ssid_len: ssid.uSSIDLength,
                ssid: ssid.ucSSID,
                reason,
            })
        }
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
        if n > MAX_INTERFACES {
            return Err(u32::MAX);
        }
        // SAFETY: wlanapi が `dwNumberOfItems` 件分の `WLAN_INTERFACE_INFO` を連続して確保した領域。
        // 件数は上で検証済み。`_mem` が生きている間だけ読み、`Copy` の値として複写する。
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

    /// 失敗後の電源状態の読み直し。読めなければ `false`（元の失敗をそのまま返す）。
    #[allow(unsafe_code)]
    fn radio_is_off(c: &Client, guid: &GUID) -> bool {
        let mut size = 0u32;
        let mut data: *mut c_void = std::ptr::null_mut();
        // SAFETY: すべて呼び出しの間有効なローカル変数・引数への排他ポインタ。
        let rc = unsafe {
            WlanQueryInterface(
                c.0,
                guid,
                wlan_intf_opcode_radio_state,
                None,
                &mut size,
                &mut data,
                None,
            )
        };
        let mem = Mem(data);
        if rc != 0 || mem.0.is_null() || (size as usize) < size_of::<WLAN_RADIO_STATE>() {
            return false;
        }
        // SAFETY: `mem.0` は `size >= size_of::<WLAN_RADIO_STATE>()` バイトの有効な領域。
        let radio: WLAN_RADIO_STATE =
            unsafe { std::ptr::read_unaligned(mem.0 as *const WLAN_RADIO_STATE) };
        let n = (radio.dwNumberOfPhys as usize).min(radio.PhyRadioState.len());
        let states: Vec<(i32, i32)> = radio.PhyRadioState[..n]
            .iter()
            .map(|p| (p.dot11SoftwareRadioState.0, p.dot11HardwareRadioState.0))
            .collect();
        radio_off(&states)
    }

    #[allow(unsafe_code)]
    fn connect(req: &ConnectRequest) -> Result<(), DeviceError> {
        let client = Client::open().map_err(|rc| match rc {
            SERVICE_NOT_ACTIVE => DeviceError::Unsupported(FEATURE),
            _ => to_error(classify_sync(rc), false),
        })?;
        let ifaces = interfaces(&client).map_err(|rc| to_error(classify_sync(rc), false))?;
        // ponytail: 複数アダプターは 1 つだけ使う（接続中の最初、無ければ先頭）。
        let &(guid, _) = ifaces
            .iter()
            .find(|i| i.1)
            .or_else(|| ifaces.first())
            .ok_or(DeviceError::Unsupported(FEATURE))?;

        let ssid_bytes = req.ssid.as_bytes();
        if ssid_bytes.len() > SSID_MAX_BYTES {
            return Err(DeviceError::InvalidInput(FEATURE));
        }
        let shared = Arc::new(Shared {
            guid,
            ssid: ssid_bytes.to_vec(),
            slot: Slot::new(),
        });
        let mut guard = ConnectGuard {
            ctx: Arc::into_raw(Arc::clone(&shared)),
            registered: false,
            client,
        };

        // SAFETY: ハンドルは有効。`ctx` は `guard` が解除後に解放するまで生きる。通知は ACM のみ。
        let rc = unsafe {
            WlanRegisterNotification(
                guard.client.0,
                WLAN_NOTIFICATION_SOURCE_ACM,
                true,
                Some(callback),
                Some(guard.ctx as *const c_void),
                None,
                None,
            )
        };
        if rc != 0 {
            return Err(to_error(classify_sync(rc), false));
        }
        guard.registered = true;

        let xml: Vec<u16> = profile_xml(req)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut dot11 = DOT11_SSID {
            uSSIDLength: ssid_bytes.len() as u32,
            ucSSID: [0u8; 32],
        };
        dot11.ucSSID[..ssid_bytes.len()].copy_from_slice(ssid_bytes);
        let params = WLAN_CONNECTION_PARAMETERS {
            wlanConnectionMode: wlan_connection_mode_temporary_profile,
            strProfile: PCWSTR(xml.as_ptr()),
            pDot11Ssid: &mut dot11,
            pDesiredBssidList: std::ptr::null_mut(),
            dot11BssType: dot11_BSS_type_infrastructure,
            dwFlags: 0,
        };
        // SAFETY: `xml`（NUL 終端）・`dot11`・`params` は戻るまで生きるローカル。BSSID 一覧は null。
        let rc = unsafe { WlanConnect(guard.client.0, &guid, &params, None) };
        if rc != 0 {
            let failure = classify_sync(rc);
            let off = failure == Failure::Other && radio_is_off(&guard.client, &guid);
            return Err(to_error(failure, off));
        }

        // 戻り値 0 は受け付けただけ。終端の通知を待つ。
        match shared.slot.wait() {
            Outcome::Success => Ok(()),
            Outcome::NotFound => Err(to_error(Failure::NotFound, false)),
            Outcome::Failed => {
                let off = radio_is_off(&guard.client, &guid);
                Err(to_error(Failure::Other, off))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::wifi_connect::validate;

    fn data(mode: i32, ssid: &[u8], reason: u32) -> ConnData {
        let mut b = [0u8; SSID_MAX_BYTES];
        b[..ssid.len()].copy_from_slice(ssid);
        ConnData {
            mode,
            ssid_len: ssid.len() as u32,
            ssid: b,
            reason,
        }
    }

    fn m(code: u32, d: Option<&ConnData>) -> Option<Outcome> {
        match_notification(SOURCE_ACM, code, true, d, b"home")
    }

    #[test]
    fn escape_covers_all_specials() {
        assert_eq!(
            xml_escape("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
        assert_eq!(xml_escape(" pass word "), " pass word ");
    }

    #[test]
    fn profile_open() {
        let x = profile_xml(&validate("home", "").unwrap());
        assert!(x.contains("<hex>686F6D65</hex>"));
        assert!(x.contains("<authentication>open</authentication>"));
        assert!(x.contains("<encryption>none</encryption>"));
        assert!(!x.contains("sharedKey"));
        assert!(x.contains("<name>fandhe-wifi-connect</name>"));
        assert!(!x.contains(">home<"));
    }

    #[test]
    fn profile_passphrase_and_escape() {
        let x = profile_xml(&validate("あ b", " p&<>\"'w ").unwrap());
        assert!(x.contains("<hex>E381822062</hex>"));
        assert!(x.contains("WPA2PSK"));
        assert!(x.contains("<encryption>AES</encryption>"));
        assert!(x.contains("<keyType>passPhrase</keyType>"));
        assert!(x.contains("<protected>false</protected>"));
        assert!(x.contains("<keyMaterial> p&amp;&lt;&gt;&quot;&apos;w </keyMaterial>"));
    }

    #[test]
    fn profile_network_key() {
        let key = "ab".repeat(32);
        let x = profile_xml(&validate("s", &key).unwrap());
        assert!(x.contains("<keyType>networkKey</keyType>"));
    }

    #[test]
    fn error_mapping() {
        assert!(matches!(
            to_error(classify_sync(5), false),
            DeviceError::PermissionDenied(FEATURE)
        ));
        assert!(matches!(
            to_error(classify_sync(87), false),
            DeviceError::Internal(CONNECT_FAILED)
        ));
        assert!(matches!(
            to_error(classify_sync(87), true),
            DeviceError::Disabled(RADIO_OFF)
        ));
        assert!(matches!(
            to_error(Failure::NotFound, true),
            DeviceError::Internal(NOT_FOUND)
        ));
        assert_eq!(SERVICE_NOT_ACTIVE, 1062);
    }

    #[test]
    fn only_complete_with_success_reason_succeeds() {
        let ok = data(1, b"home", 0);
        assert_eq!(m(10, Some(&ok)), Some(Outcome::Success));
        assert_eq!(m(11, Some(&ok)), Some(Outcome::Failed));
        assert_eq!(
            m(10, Some(&data(1, b"home", 0x28001))),
            Some(Outcome::Failed)
        );
        assert_eq!(
            m(10, Some(&data(1, b"home", REASON_NOT_VISIBLE))),
            Some(Outcome::NotFound)
        );
        assert_eq!(
            m(11, Some(&data(1, b"home", REASON_NETWORK_NOT_AVAILABLE))),
            Some(Outcome::NotFound)
        );
    }

    #[test]
    fn mismatches_are_ignored() {
        let ok = data(1, b"home", 0);
        assert_eq!(match_notification(1, 10, true, Some(&ok), b"home"), None);
        assert_eq!(
            match_notification(SOURCE_ACM, 10, false, Some(&ok), b"home"),
            None
        );
        assert_eq!(m(9, Some(&ok)), None);
        assert_eq!(m(10, None), None);
        assert_eq!(m(10, Some(&data(0, b"home", 0))), None);
        assert_eq!(m(10, Some(&data(1, b"other", 0))), None);
        for len in [33, u32::MAX] {
            let mut d = ok;
            d.ssid_len = len;
            assert_eq!(m(10, Some(&d)), None, "{len}");
        }
    }

    #[test]
    fn interface_removal_ends_wait_as_failure() {
        assert_eq!(m(14, None), Some(Outcome::Failed));
        assert_eq!(
            match_notification(SOURCE_ACM, 14, false, None, b"home"),
            None
        );
    }

    #[test]
    fn slot_keeps_first_terminal() {
        let s = Slot::new();
        s.finish(Outcome::Failed);
        s.finish(Outcome::Success);
        assert_eq!(s.wait(), Outcome::Failed);
    }
}
