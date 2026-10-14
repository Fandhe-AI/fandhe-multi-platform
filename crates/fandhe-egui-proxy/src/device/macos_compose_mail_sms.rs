//! macOS 向けメール・SMS 作成画面の実装（Issue #1384）。`macos/fandhe_macos_compose_mail_sms.m`
//! （`NSWorkspace openURL:`、`build.rs` が macOS ターゲットの feature `device` 有効時にコンパイル
//! する）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を
//! 明記する。`device/compose_mail_sms.rs` の macOS 経路から呼ばれ、契約は
//! `fandhe_device::compose_mail_sms`。URL は `ComposeRequest::to_url`（scheme は `mailto:` /
//! `sms:` 固定）で組み立て済みのものだけを渡す。
//!
//! 結果コードの写像 [`outcome_from_code`] はホストでもテストできるようモジュールは
//! `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 成功は `HandedOff`（OS が URL を受理した）に留める。メールアプリが開いたか・送ったかは不明
//! - `sms:`（宛先なし含む）を Messages が受けるかは Mac 目視で確認する（#1383）

use fandhe_device::compose_mail_sms::ComposeOutcome;
use fandhe_device::DeviceError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = 受理 / 1 = 扱うアプリ無し（`openURL:` が NO）/ 2 = NSURL を作れない・main thread 外。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn outcome_from_code(code: i32) -> Result<ComposeOutcome, DeviceError> {
    match code {
        0 => Ok(ComposeOutcome::HandedOff),
        1 => Ok(ComposeOutcome::NoHandler),
        2 => Err(DeviceError::Internal("compose present")),
        _ => Err(DeviceError::Internal("compose outcome")),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::CString;

    // SAFETY: `macos/fandhe_macos_compose_mail_sms.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_compose_open_url(url: *const std::ffi::c_char) -> i32;
    }

    /// 結果コードを返す。`CString` を作れなければ 2（NSURL を作れないのと同じ扱い）。
    #[allow(unsafe_code)]
    pub(super) fn open_url(url: &str) -> i32 {
        let Ok(c) = CString::new(url) else {
            return 2;
        };
        // SAFETY: `c` はこの呼び出しの間だけ生きる NUL 終端文字列。`.m` はポインタを呼び出し中に
        // だけ読み、保持しない。
        unsafe { fandhe_macos_compose_open_url(c.as_ptr()) }
    }
}

#[cfg(target_os = "macos")]
struct MacComposeMailSms;

#[cfg(target_os = "macos")]
impl fandhe_device::compose_mail_sms::ComposeMailSms for MacComposeMailSms {
    fn compose(
        &self,
        req: &fandhe_device::compose_mail_sms::ComposeRequest,
    ) -> Result<ComposeOutcome, DeviceError> {
        outcome_from_code(ffi::open_url(&req.to_url()))
    }
}

/// macOS の `ComposeMailSms` 実装を返す（`device/compose_mail_sms.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::compose_mail_sms::ComposeMailSms> {
    Box::new(MacComposeMailSms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_table() {
        assert_eq!(outcome_from_code(0).unwrap(), ComposeOutcome::HandedOff);
        assert_eq!(outcome_from_code(1).unwrap(), ComposeOutcome::NoHandler);
        for code in [2, 3, -1, i32::MAX] {
            assert!(outcome_from_code(code).is_err());
        }
    }
}
