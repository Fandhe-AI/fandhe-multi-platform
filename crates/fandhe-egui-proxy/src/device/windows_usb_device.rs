//! Windows 向け USB 機器一覧実装（Issue #506、親 #112）。`device/usb_device.rs::
//! platform_usb_devices` の Windows 経路から UI スレッドで同期に 1 回呼ばれる。契約の SSOT は
//! `fandhe_device::usb_device`。
//!
//! # 流れ（COM なし、ワーカーなし）
//!
//! 1. `SetupDiGetClassDevsW(GUID_DEVINTERFACE_USB_DEVICE, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE)` で
//!    接続中の USB 機器の一覧を得る（機器は開かない）。
//! 2. `SetupDiEnumDeviceInfo` で回し、`SPDRP_HARDWAREID` から VID/PID を
//!    [`parse_hardware_id`] で取り出す。名前は `DEVPKEY_Device_BusReportedDeviceDesc`（機器が報告する
//!    製品名。macOS の `USB Product Name` 相当）。取れなければ `None`（汎用名へは倒さない）。
//! 3. [`normalize`] に通して返す。
//!
//! 列挙の開始失敗・途中の想定外エラーは `Internal`（部分結果を成功にしない）。`ERROR_NO_MORE_ITEMS`
//! による終端は正常で、0 件も成功。一覧ハンドルは RAII で全経路で解放する。
//!
//! # 取らないもの
//!
//! シリアル・インスタンス ID・デバイスパスは読まない（`SetupDiGetDeviceInstanceIdW` 等を呼ばない）。
//! hardware ID は VID/PID を取り出したら捨てる。エラーコード・名前はログに出さない。
//!
//! WinRT `Windows.Devices.Usb` は package identity が必要なため使わない。WinUSB は機器を開く API で
//! 範囲外。型検査は `x86_64-pc-windows-gnu` のみで、実機確認は #504。

use fandhe_device::usb_device::{
    normalize, parse_hardware_id, UsbDeviceEntry, UsbDevices, MAX_DEVICES,
};
use fandhe_device::DeviceError;
use windows::core::HRESULT;
use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDevicePropertyW, SetupDiGetDeviceRegistryPropertyW, DIGCF_DEVICEINTERFACE,
    DIGCF_PRESENT, HDEVINFO, SPDRP_HARDWAREID, SP_DEVINFO_DATA,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_Device_BusReportedDeviceDesc, DEVPROPTYPE, DEVPROP_TYPE_STRING,
};
use windows::Win32::Devices::Usb::GUID_DEVINTERFACE_USB_DEVICE;
use windows::Win32::Foundation::{ERROR_INVALID_DATA, ERROR_NO_MORE_ITEMS};

/// Windows の `UsbDevices` 実装を返す。
pub(crate) fn implementation() -> Box<dyn UsbDevices> {
    Box::new(WindowsUsbDevices)
}

struct WindowsUsbDevices;

/// 機器情報セットのハンドル。`Drop` で解放し、早期 return でも漏らさない。
struct DevInfo(HDEVINFO);

impl Drop for DevInfo {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: `self.0` は `SetupDiGetClassDevsW` が返した有効なハンドルで、ここで 1 度だけ解放する。
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

/// UTF-16 バイト列（LE）を NUL までの文字列にする。不正な UTF-16 は `None`。
fn utf16_until_nul(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16(&units).ok()
}

/// hardware ID 取得失敗の分類。プロパティが無い（`ERROR_INVALID_DATA`）のは想定内で機器を飛ばし、
/// それ以外（アクセス拒否等）は一覧を不完全にするため `Internal` として伝播する。
fn classify_registry_error(code: HRESULT) -> Result<Option<(u16, u16)>, DeviceError> {
    if code == ERROR_INVALID_DATA.to_hresult() {
        Ok(None)
    } else {
        Err(DeviceError::Internal("usb_device list"))
    }
}

/// 1 件の VID/PID。プロパティ無し・解析不能は `Ok(None)`（捨てる）、想定外の OS エラーは `Err`。
#[allow(unsafe_code)]
fn vid_pid(set: HDEVINFO, data: &SP_DEVINFO_DATA) -> Result<Option<(u16, u16)>, DeviceError> {
    let mut buf = [0u8; 1024];
    // SAFETY: `set` は有効なハンドル、`data` は `cbSize` 初期化済みで列挙が埋めたもの。
    // バッファは slice で長さを OS に伝える。
    if let Err(e) = unsafe {
        SetupDiGetDeviceRegistryPropertyW(set, data, SPDRP_HARDWAREID, None, Some(&mut buf), None)
    } {
        return classify_registry_error(e.code());
    }
    // REG_MULTI_SZ の先頭の文字列だけを使う。
    Ok(utf16_until_nul(&buf).and_then(|s| parse_hardware_id(&s)))
}

/// 機器が報告する製品名。型違い・失敗・不正な UTF-16 は `None`。
#[allow(unsafe_code)]
fn product_name(set: HDEVINFO, data: &SP_DEVINFO_DATA) -> Option<String> {
    let mut buf = [0u8; 512];
    let mut ty = DEVPROPTYPE(0);
    // SAFETY: `set`・`data` は上と同じ。キーはスタック上の定数、`ty` と `buf` は有効な書き込み先で、
    // バッファ長は slice で渡している。
    unsafe {
        SetupDiGetDevicePropertyW(
            set,
            data,
            &DEVPKEY_Device_BusReportedDeviceDesc,
            &mut ty,
            Some(&mut buf),
            None,
            0,
        )
    }
    .ok()?;
    (ty == DEVPROP_TYPE_STRING)
        .then(|| utf16_until_nul(&buf))
        .flatten()
}

impl UsbDevices for WindowsUsbDevices {
    #[allow(unsafe_code)]
    fn list(&self) -> Result<Vec<UsbDeviceEntry>, DeviceError> {
        let fail = || DeviceError::Internal("usb_device list");
        // SAFETY: GUID は静的な定数への有効な参照。他の引数は未指定（NULL）で良い組み合わせ。
        let set = unsafe {
            SetupDiGetClassDevsW(
                Some(&GUID_DEVINTERFACE_USB_DEVICE),
                PCWSTR::null(),
                None,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
        }
        .map_err(|_| fail())?;
        let set = DevInfo(set);

        let mut out = Vec::new();
        let mut index = 0u32;
        while out.len() < MAX_DEVICES {
            let mut data = SP_DEVINFO_DATA {
                cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            // SAFETY: `set.0` は有効なハンドル、`data` は `cbSize` 初期化済みの書き込み先。
            match unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut data) } {
                Ok(()) => {}
                Err(e) if e.code() == ERROR_NO_MORE_ITEMS.to_hresult() => break,
                Err(_) => return Err(fail()),
            }
            index += 1;
            if let Some((vendor_id, product_id)) = vid_pid(set.0, &data)? {
                out.push(UsbDeviceEntry {
                    vendor_id,
                    product_id,
                    name: product_name(set.0, &data),
                });
            }
        }
        Ok(normalize(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::ERROR_ACCESS_DENIED;

    #[test]
    fn 取得失敗は握りつぶさず_プロパティ無しだけ飛ばす() {
        assert!(matches!(
            classify_registry_error(ERROR_INVALID_DATA.to_hresult()),
            Ok(None)
        ));
        assert!(classify_registry_error(ERROR_ACCESS_DENIED.to_hresult()).is_err());
    }
}
