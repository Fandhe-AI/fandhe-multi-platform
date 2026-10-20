//! macOS 向け USB 機器一覧実装（Issue #505）。`macos/fandhe_macos_usb_device.m`（IOKit の
//! `IOUSBHostDevice` 列挙。`build.rs` が macOS ターゲットの feature `device` 有効時にコンパイルし
//! IOKit をリンクする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ
//! `// SAFETY:` で契約を明記する。`device/usb_device.rs` の macOS 経路から呼ばれ、契約は
//! `fandhe_device::usb_device`。
//!
//! `.m` が返す固定長バッファは [`from_raw`] で契約の値へ写す。写像はホストでもテストできるよう
//! モジュールは `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 取るのは `idVendor` / `idProduct` / `USB Product Name` のみ。シリアル・locationID は読まない
//! - 機器は開かないため TCC も entitlement も要らない見込み（実機確認は #504）
//! - ハブや内蔵 USB 機器が含まれるかは OS 任せ

use std::ffi::CStr;

use fandhe_device::usb_device::{normalize, UsbDeviceEntry, MAX_DEVICES};
use fandhe_device::DeviceError;

const NAME_BYTES: usize = 512;

/// `.m` の `FandheUsbDeviceRaw` と同じレイアウト。
#[repr(C)]
struct RawEntry {
    vendor_id: i32,
    product_id: i32,
    name: [u8; NAME_BYTES],
}

/// `.m` の `FandheUsbDeviceListRaw` と同じレイアウト。
#[repr(C)]
struct Raw {
    count: i32,
    entries: [RawEntry; MAX_DEVICES],
}

/// `.m` の生値を契約の値へ写す。件数が 0..=64 の外なら `Internal`（0 件の成功に畳まない）。
/// VID/PID は丸める前に `u16` の範囲を検証し、範囲外の項目は捨てる。名前は NUL 終端と UTF-8 を
/// 検証し、満たさなければ `None`（件数には数える）。
fn from_raw(raw: &Raw) -> Result<Vec<UsbDeviceEntry>, DeviceError> {
    let n = usize::try_from(raw.count)
        .ok()
        .filter(|&n| n <= MAX_DEVICES)
        .ok_or(DeviceError::Internal("usb_device list"))?;
    Ok(normalize(raw.entries[..n].iter().filter_map(|e| {
        Some(UsbDeviceEntry {
            vendor_id: u16::try_from(e.vendor_id).ok()?,
            product_id: u16::try_from(e.product_id).ok()?,
            name: CStr::from_bytes_until_nul(&e.name)
                .ok()
                .and_then(|c| c.to_str().ok())
                .map(str::to_owned),
        })
    })))
}

#[cfg(target_os = "macos")]
mod ffi {
    use super::Raw;

    // SAFETY: `macos/fandhe_macos_usb_device.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_usb_device_list(out: *mut Raw) -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn list(out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持しない。
        unsafe { fandhe_macos_usb_device_list(out) }
    }
}

fn blank() -> Box<Raw> {
    Box::new(Raw {
        count: 0,
        entries: std::array::from_fn(|_| RawEntry {
            vendor_id: 0,
            product_id: 0,
            name: [0; NAME_BYTES],
        }),
    })
}

#[cfg(target_os = "macos")]
struct MacUsbDevices;

#[cfg(target_os = "macos")]
impl fandhe_device::usb_device::UsbDevices for MacUsbDevices {
    fn list(&self) -> Result<Vec<UsbDeviceEntry>, DeviceError> {
        // 約 33KB。ヒープに置く。
        let mut raw = blank();
        if ffi::list(&mut raw) != 0 {
            return Err(DeviceError::Internal("usb_device list"));
        }
        from_raw(&raw)
    }
}

/// macOS の `UsbDevices` 実装を返す（`device/usb_device.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::usb_device::UsbDevices> {
    Box::new(MacUsbDevices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(slot: &mut [u8; NAME_BYTES], s: &[u8]) {
        slot[..s.len()].copy_from_slice(s);
    }

    #[test]
    fn maps_normal_values() {
        let mut r = blank();
        r.count = 1;
        r.entries[0].vendor_id = 0x05ac;
        r.entries[0].product_id = 0x024f;
        put(&mut r.entries[0].name, "キーボード".as_bytes());
        let v = from_raw(&r).unwrap();
        assert_eq!(
            v,
            [UsbDeviceEntry {
                vendor_id: 0x05ac,
                product_id: 0x024f,
                name: Some("キーボード".into())
            }]
        );
    }

    #[test]
    fn out_of_range_count_is_error() {
        for c in [-1, 65] {
            let mut r = blank();
            r.count = c;
            assert!(matches!(
                from_raw(&r),
                Err(DeviceError::Internal("usb_device list"))
            ));
        }
    }

    #[test]
    fn out_of_range_ids_drop_the_entry() {
        let mut r = blank();
        r.count = 3;
        r.entries[0].vendor_id = -1;
        r.entries[1].vendor_id = 65536;
        r.entries[2].vendor_id = 65535;
        let v = from_raw(&r).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].vendor_id, 0xffff);
    }

    #[test]
    fn bad_names_become_none_but_are_counted() {
        let mut r = blank();
        r.count = 2;
        r.entries[0].name = [b'a'; NAME_BYTES];
        put(&mut r.entries[1].name, &[0xff, 0xfe]);
        let v = from_raw(&r).unwrap();
        assert_eq!(v.len(), 2);
        assert!(v.iter().all(|e| e.name.is_none()));
    }

    #[test]
    fn control_chars_are_removed() {
        let mut r = blank();
        r.count = 1;
        put(&mut r.entries[0].name, b"KB\x01\n 2");
        assert_eq!(from_raw(&r).unwrap()[0].name.as_deref(), Some("KB 2"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn real_os_call_succeeds() {
        // 機器が無くても 0 件の成功になる。
        assert!(implementation().list().is_ok());
    }
}
