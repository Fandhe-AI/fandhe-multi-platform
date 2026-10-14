//! macOS 向け端末・OS 情報実装（Issue #1304）。`macos/fandhe_macos_device_info.m`
//! （`NSProcessInfo` + `sysctl` + `NSURL` のボリューム容量、`build.rs` が macOS ターゲットの
//! feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` は
//! この呼び出しに閉じ `// SAFETY:` で契約を明記する。`device/device_info.rs` の macOS 経路から
//! 呼ばれ、契約は `fandhe_device::device_info`。
//!
//! `.m` が返す生値（負値の番兵、固定長の機種バッファ）は [`from_raw`] で契約の値へ写す。
//! 写像はホストでもテストできるようモジュールは `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 機種は `hw.model` の型番識別子（`MacBookPro18,3` 等）で、製品名ではない
//! - 空き容量はホームのボリュームの OS の見積り（`volumeAvailableCapacityForImportantUsage`）
//! - 個体識別子（`hw.uuid`・シリアル）は読まない
//! - 実機の目視確認は #1301

use std::ffi::CStr;

#[cfg(target_os = "macos")]
use fandhe_device::device_info::require_any;
use fandhe_device::device_info::{clean_text, DeviceInfoSnapshot};

/// `.m` の `FandheDeviceInfoRaw` と同じレイアウト。
#[repr(C)]
#[derive(Clone, Copy)]
struct Raw {
    os_major: i64,
    os_minor: i64,
    os_patch: i64,
    memory_bytes: i64,
    free_bytes: i64,
    model: [u8; 64],
}

/// `.m` の生値を契約の値へ写す。負値は不明（`None`）。
fn from_raw(raw: Raw) -> DeviceInfoSnapshot {
    let os_version = match (raw.os_major, raw.os_minor, raw.os_patch) {
        (a, b, c) if a >= 0 && b >= 0 && c >= 0 => Some(format!("{a}.{b}.{c}")),
        _ => None,
    };
    let model = CStr::from_bytes_until_nul(&raw.model)
        .ok()
        .and_then(|c| c.to_str().ok())
        .and_then(clean_text);
    DeviceInfoSnapshot {
        os_name: Some("macOS".to_string()),
        os_version,
        model,
        memory_bytes: u64::try_from(raw.memory_bytes).ok(),
        free_storage_bytes: u64::try_from(raw.free_bytes).ok(),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use super::Raw;

    // SAFETY: `macos/fandhe_macos_device_info.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_device_info_read(out: *mut Raw) -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn read(out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持しない。
        unsafe { fandhe_macos_device_info_read(out) }
    }
}

#[cfg(target_os = "macos")]
struct MacDeviceInfo;

#[cfg(target_os = "macos")]
impl fandhe_device::device_info::DeviceInfo for MacDeviceInfo {
    fn read(&self) -> Result<DeviceInfoSnapshot, fandhe_device::DeviceError> {
        let mut raw = Raw {
            os_major: -1,
            os_minor: -1,
            os_patch: -1,
            memory_bytes: -1,
            free_bytes: -1,
            model: [0; 64],
        };
        if ffi::read(&mut raw) != 0 {
            return Err(fandhe_device::DeviceError::Internal("device_info read"));
        }
        require_any(from_raw(raw))
    }
}

/// macOS の `DeviceInfo` 実装を返す（`device/device_info.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::device_info::DeviceInfo> {
    Box::new(MacDeviceInfo)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(model: &[u8]) -> Raw {
        let mut buf = [0u8; 64];
        buf[..model.len()].copy_from_slice(model);
        Raw {
            os_major: 14,
            os_minor: 5,
            os_patch: 0,
            memory_bytes: 16 << 30,
            free_bytes: 100,
            model: buf,
        }
    }

    /// 実 OS を 1 回読めること（リンクと FFI の疎通。値は機種依存のため成功だけ見る）。
    #[cfg(target_os = "macos")]
    #[test]
    fn real_read_succeeds() {
        let s = implementation().read().expect("成功");
        assert_eq!(s.os_name.as_deref(), Some("macOS"));
        assert!(s.memory_bytes.is_some());
    }

    #[test]
    fn maps_normal_values() {
        let s = from_raw(raw(b"Mac14,2"));
        assert_eq!(s.os_version.as_deref(), Some("14.5.0"));
        assert_eq!(s.model.as_deref(), Some("Mac14,2"));
        assert_eq!(s.memory_bytes, Some(16 << 30));
        assert_eq!(s.free_storage_bytes, Some(100));
    }

    #[test]
    fn negative_sentinels_are_none_and_zero_free_is_kept() {
        let mut r = raw(b"");
        r.memory_bytes = -1;
        r.free_bytes = 0;
        let s = from_raw(r);
        assert_eq!(s.memory_bytes, None);
        assert_eq!(s.free_storage_bytes, Some(0));
        assert_eq!(s.model, None);
    }

    #[test]
    fn partially_negative_version_is_none() {
        let mut r = raw(b"x");
        r.os_patch = -1;
        assert_eq!(from_raw(r).os_version, None);
    }

    #[test]
    fn model_without_nul_or_non_utf8_is_none() {
        let mut r = raw(b"");
        r.model = [b'a'; 64];
        assert_eq!(from_raw(r).model, None);
        assert_eq!(from_raw(raw(&[0xff, 0xfe])).model, None);
    }

    #[test]
    fn model_control_chars_are_removed() {
        assert_eq!(from_raw(raw(b"Mac\x01\n1")).model.as_deref(), Some("Mac1"));
    }
}
