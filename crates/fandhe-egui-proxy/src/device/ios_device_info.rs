//! iOS 向け端末・OS 情報実装（Issue #1305）。`ios/fandhe_ios_device_info.m`
//! （`NSProcessInfo` + `sysctl` + `NSURL` のボリューム容量、`build.rs` が iOS ターゲットの
//! feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。#1304 の
//! スタブを置き換えたもので、dispatch（`device/device_info.rs`）と共通コア画面は変えない。
//! 契約は `fandhe_device::device_info`。
//!
//! `.m` の生値（負値の番兵、固定長の機種バッファ）は [`from_raw`] で契約の値へ写す
//! （ホストでテストできるようモジュールは `cfg(any(ios, test))`）。OS 名は機種が `iPad` で
//! 始まれば `iPadOS`、それ以外は `iOS`。
//!
//! # 再描画
//!
//! OS コールバックを持たず結果を共有状態へ書く経路が無いため、`crate::ios::request_repaint` は
//! 呼ばない。再描画は `drive` の完了通知（`CompletionSender::send`）が起こす。
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 1 プロセス内の取得、全項目不明を成功へ畳まないこと（`require_any`）、シミュレータの
//!   ホスト機種（`arm64`）を機種として出さないこと、0 バイトの空きと不明の区別
//! - 守らない: OS 間の値の一致、読み取り後の変化（空き容量は読んだ時点の値）、複数プロセス
//!
//! # 既知の天井
//!
//! - 機種は `hw.machine` の型番識別子（`iPhone16,1` 等）で、製品名ではない
//! - 空き容量はホームのボリュームの OS の見積り（`volumeAvailableCapacityForImportantUsage`）。
//!   required reason API のため privacy manifest（DiskSpace `85F4.1`）を同梱する
//! - 個体識別子（UDID・シリアル）と端末名は読まない
//! - 実機の目視確認は #1301

use std::ffi::CStr;

#[cfg(target_os = "ios")]
use fandhe_device::device_info::require_any;
use fandhe_device::device_info::{clean_text, DeviceInfoSnapshot};

/// `.m` の `FandheIosDeviceInfoRaw` と同じレイアウト。
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

/// `.m` の生値を契約の値へ写す。負値は不明（`None`）、総メモリ 0 は不正値として不明。
fn from_raw(raw: Raw) -> DeviceInfoSnapshot {
    let os_version = match (raw.os_major, raw.os_minor, raw.os_patch) {
        (a, b, c) if a >= 0 && b >= 0 && c >= 0 => Some(format!("{a}.{b}.{c}")),
        _ => None,
    };
    let model = CStr::from_bytes_until_nul(&raw.model)
        .ok()
        .and_then(|c| c.to_str().ok())
        .and_then(clean_text);
    let os_name = if model.as_deref().is_some_and(|m| m.starts_with("iPad")) {
        "iPadOS"
    } else {
        "iOS"
    };
    DeviceInfoSnapshot {
        os_name: Some(os_name.to_string()),
        os_version,
        model,
        memory_bytes: u64::try_from(raw.memory_bytes).ok().filter(|&b| b > 0),
        free_storage_bytes: u64::try_from(raw.free_bytes).ok(),
    }
}

#[cfg(target_os = "ios")]
mod ffi {
    use super::Raw;

    // SAFETY: `ios/fandhe_ios_device_info.m`（`build.rs` が iOS ターゲットの feature `device`
    // 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_device_info_read(out: *mut Raw) -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn read(out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持しない。
        unsafe { fandhe_ios_device_info_read(out) }
    }
}

#[cfg(target_os = "ios")]
struct IosDeviceInfo;

#[cfg(target_os = "ios")]
impl fandhe_device::device_info::DeviceInfo for IosDeviceInfo {
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

/// iOS の `DeviceInfo` 実装を返す（`device/device_info.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::device_info::DeviceInfo> {
    Box::new(IosDeviceInfo)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(model: &[u8]) -> Raw {
        let mut buf = [0u8; 64];
        buf[..model.len()].copy_from_slice(model);
        Raw {
            os_major: 17,
            os_minor: 5,
            os_patch: 1,
            memory_bytes: 6 << 30,
            free_bytes: 100,
            model: buf,
        }
    }

    #[test]
    fn maps_normal_values() {
        let s = from_raw(raw(b"iPhone16,1"));
        assert_eq!(s.os_name.as_deref(), Some("iOS"));
        assert_eq!(s.os_version.as_deref(), Some("17.5.1"));
        assert_eq!(s.model.as_deref(), Some("iPhone16,1"));
        assert_eq!(s.memory_bytes, Some(6 << 30));
        assert_eq!(s.free_storage_bytes, Some(100));
    }

    #[test]
    fn ipad_is_ipados() {
        assert_eq!(
            from_raw(raw(b"iPad13,4")).os_name.as_deref(),
            Some("iPadOS")
        );
        assert_eq!(from_raw(raw(b"")).os_name.as_deref(), Some("iOS"));
    }

    #[test]
    fn sentinels_and_zero() {
        let mut r = raw(b"");
        r.memory_bytes = -1;
        r.free_bytes = 0;
        let s = from_raw(r);
        assert_eq!(s.memory_bytes, None);
        assert_eq!(s.free_storage_bytes, Some(0));
        assert_eq!(s.model, None);
        r.memory_bytes = 0;
        r.free_bytes = -1;
        let s = from_raw(r);
        assert_eq!(s.memory_bytes, None);
        assert_eq!(s.free_storage_bytes, None);
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
        assert_eq!(
            from_raw(raw(b"iPhone\x01\n1")).model.as_deref(),
            Some("iPhone1")
        );
    }
}
