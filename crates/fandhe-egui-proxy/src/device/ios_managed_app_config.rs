//! iOS 向け MDM 管理アプリ設定実装（Issue #945）。`ios/fandhe_ios_managed_app_config.m`
//! （`NSUserDefaults` の `com.apple.configuration.managed` 辞書から固定 3 キーを読み、ワイヤ文字列にして
//! 返す。`build.rs` が iOS ターゲットの feature `device` 有効時にコンパイルする）を
//! `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/managed_app_config.rs` の iOS 経路から呼ばれ、契約は `fandhe_device::managed_app_config`。
//!
//! # 再描画
//!
//! 同期 getter で、OS コールバックが共有状態へ結果を書く経路が無い。結果は effect ドライバの
//! `CompletionSender::send` が `ctx.request_repaint()` で届けるため、`crate::ios::request_repaint`
//! は呼ばない（`ios_device_info.rs` と同じ）。
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 辞書でない値は失敗（`Internal`）にし「設定なし」へ畳まない。型違いは `?` で見せる。
//!   保留中の要求・購読・オブザーバーを持たないため、ライフサイクルで破棄するものは無い
//! - 守らない: 読み取り後の変化の反映（再押下まで。`didChangeNotification` は自プロセスの変更しか
//!   拾えない）、期限（`NSUserDefaults` の同期読み取りで戻らない場合）、未管理端末と設定ゼロの区別
//! - MDM 経路の実機確認は #937

use std::ffi::{c_char, CStr};
use std::sync::Arc;

use fandhe_device::managed_app_config::{parse_wire, ManagedAppConfig, Report, READ_FAILED};
use fandhe_device::DeviceError;

/// ワイヤ文字列の受け取りバッファ長（`parse_wire` の上限 128 バイト + NUL）。
const BUF_LEN: usize = 129;

mod ffi {
    use super::{c_char, BUF_LEN};

    // SAFETY: `ios/fandhe_ios_managed_app_config.m`（`build.rs` が iOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_managed_app_config_read(out: *mut c_char, cap: usize) -> i32;
    }

    /// 0 = 成功（`out` は NUL 終端のワイヤ文字列）。
    #[allow(unsafe_code)]
    pub(super) fn read(out: &mut [u8; BUF_LEN]) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で長さは `BUF_LEN`、`.m` は書き込み上限に
        // `cap` を使い、ポインタを保持しない。
        unsafe { fandhe_ios_managed_app_config_read(out.as_mut_ptr().cast(), BUF_LEN) }
    }
}

struct IosManagedAppConfig;

impl ManagedAppConfig for IosManagedAppConfig {
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

/// iOS の `ManagedAppConfig` 実装を返す（`device/managed_app_config.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn ManagedAppConfig> {
    Arc::new(IosManagedAppConfig)
}
