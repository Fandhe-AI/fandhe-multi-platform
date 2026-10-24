//! macOS 向け「既定アプリで開く・Finder で表示」の実装（Issue #749）。
//! `macos/fandhe_macos_open_in_shell.m`（`NSWorkspace`、`build.rs` が macOS ターゲットの
//! feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` はこの
//! 呼び出しに閉じ `// SAFETY:` で契約を明記する。`device/open_in_shell.rs` の macOS 経路から
//! 呼ばれ、契約は `fandhe_device::open_in_shell`。パスは `ShellPath`（検査・正規化済み）だけを渡す。
//!
//! 結果コードの写像 [`outcome_from_code`] はホストでもテストできるようモジュールは
//! `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 成功は `HandedOff`（OS が受理した）に留める。アプリ・Finder が表示したかは不明
//! - 検査から OS 呼び出しまでの間のファイル差し替え（TOCTOU）は守らない

use fandhe_device::open_in_shell::ShellOutcome;
use fandhe_device::DeviceError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = 受理 / 1 = 扱うアプリ無し / 2 = 拒否した型 / 3 = OS の失敗。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn outcome_from_code(code: i32) -> Result<ShellOutcome, DeviceError> {
    match code {
        0 => Ok(ShellOutcome::HandedOff),
        1 => Ok(ShellOutcome::NoHandler),
        2 => Err(DeviceError::InvalidInput("shell type")),
        3 => Err(DeviceError::Internal("shell os")),
        _ => Err(DeviceError::Internal("shell outcome")),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::CString;

    // SAFETY: `macos/fandhe_macos_open_in_shell.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_open_in_shell_open(path: *const std::ffi::c_char) -> i32;
        fn fandhe_macos_open_in_shell_reveal(path: *const std::ffi::c_char) -> i32;
    }

    /// 結果コードを返す。`CString` を作れなければ 3（文字列変換の失敗と同じ扱い）。
    #[allow(unsafe_code)]
    pub(super) fn call(path: &str, reveal: bool) -> i32 {
        let Ok(c) = CString::new(path) else {
            return 3;
        };
        // SAFETY: `c` はこの呼び出しの間だけ生きる NUL 終端文字列。`.m` はポインタを呼び出し中に
        // だけ読み、保持しない。
        unsafe {
            if reveal {
                fandhe_macos_open_in_shell_reveal(c.as_ptr())
            } else {
                fandhe_macos_open_in_shell_open(c.as_ptr())
            }
        }
    }
}

#[cfg(target_os = "macos")]
struct MacOpenInShell;

#[cfg(target_os = "macos")]
impl fandhe_device::open_in_shell::OpenInShell for MacOpenInShell {
    fn open(
        &self,
        path: &fandhe_device::open_in_shell::ShellPath,
    ) -> Result<ShellOutcome, DeviceError> {
        outcome_from_code(ffi::call(path.as_str(), false))
    }

    fn reveal(
        &self,
        path: &fandhe_device::open_in_shell::ShellPath,
    ) -> Result<ShellOutcome, DeviceError> {
        outcome_from_code(ffi::call(path.as_str(), true))
    }
}

/// macOS の `OpenInShell` 実装を返す（`device/open_in_shell.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::open_in_shell::OpenInShell> {
    Box::new(MacOpenInShell)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_table() {
        assert!(matches!(outcome_from_code(0), Ok(ShellOutcome::HandedOff)));
        assert!(matches!(outcome_from_code(1), Ok(ShellOutcome::NoHandler)));
        assert!(matches!(
            outcome_from_code(2),
            Err(DeviceError::InvalidInput("shell type"))
        ));
        assert!(matches!(
            outcome_from_code(3),
            Err(DeviceError::Internal("shell os"))
        ));
        for code in [4, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_code(code),
                Err(DeviceError::Internal("shell outcome"))
            ));
        }
    }
}
