//! macOS 向け MDM 管理アプリ設定実装（Issue #944）。`macos/fandhe_macos_managed_app_config.m`
//! （`CFPreferences` で固定 3 キーを読み、強制された値だけをワイヤ文字列にして返す。`build.rs` が
//! macOS ターゲットの feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。
//! `unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。`device/managed_app_config.rs` の
//! macOS 経路から呼ばれ、契約は `fandhe_device::managed_app_config`。
//!
//! # 既知の天井
//!
//! - プロファイルが重なったときの勝敗は OS に任せる
//! - 素の実行体は `-sectcreate` で埋め込んだ `macos/Info.plist` の `CFBundleIdentifier` を domain にする。
//!   テスト実行体には埋め込まれず、識別子が取れないため `Internal` になる（「設定なし」と嘘を言わない）
//! - 実機の目視確認は #937

use std::ffi::{c_char, CStr};
use std::sync::Arc;

use fandhe_device::managed_app_config::{parse_wire, ManagedAppConfig, Report, READ_FAILED};
use fandhe_device::DeviceError;

/// ワイヤ文字列の受け取りバッファ長（`parse_wire` の上限 128 バイト + NUL）。
const BUF_LEN: usize = 129;

mod ffi {
    use super::{c_char, BUF_LEN};

    // SAFETY: `macos/fandhe_macos_managed_app_config.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_managed_app_config_read(out: *mut c_char, cap: usize) -> i32;
    }

    /// 0 = 成功（`out` は NUL 終端のワイヤ文字列）。
    #[allow(unsafe_code)]
    pub(super) fn read(out: &mut [u8; BUF_LEN]) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で長さは `BUF_LEN`、`.m` は書き込み上限に
        // `cap` を使い、ポインタを保持しない。
        unsafe { fandhe_macos_managed_app_config_read(out.as_mut_ptr().cast(), BUF_LEN) }
    }
}

struct MacManagedAppConfig;

impl ManagedAppConfig for MacManagedAppConfig {
    fn read(&self) -> Result<Report, DeviceError> {
        let mut buf = [0u8; BUF_LEN];
        if ffi::read(&mut buf) != 0 {
            return Err(DeviceError::Internal(READ_FAILED));
        }
        let raw = CStr::from_bytes_until_nul(&buf)
            .ok()
            .and_then(|c| c.to_str().ok())
            .ok_or(DeviceError::Internal(READ_FAILED))?;
        parse_wire(raw)
    }
}

/// macOS の `ManagedAppConfig` 実装を返す（`device/managed_app_config.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn ManagedAppConfig> {
    Arc::new(MacManagedAppConfig)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実 OS を 1 回通す。テスト実行体には Info.plist が埋め込まれず識別子が取れないため
    /// `Internal` になる見込み。取れる環境なら強制値が無く `Ok`。`Unsupported` は決して返さず、
    /// 失敗を `NoConfig` へ畳まない。
    #[test]
    fn real_read_is_honest() {
        match MacManagedAppConfig.read() {
            Err(DeviceError::Internal(READ_FAILED)) | Ok(_) => {}
            other => panic!("想定外: {other:?}"),
        }
    }
}
