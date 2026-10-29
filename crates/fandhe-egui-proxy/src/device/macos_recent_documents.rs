//! macOS 向け「最近使った項目」の実装（Issue #988）。
//! `macos/fandhe_macos_recent_documents.m`（`NSDocumentController`、`build.rs` が macOS ターゲットの
//! feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` はこの
//! 呼び出しに閉じ `// SAFETY:` で契約を明記する。`device/recent_documents.rs` の macOS 経路から
//! 呼ばれ、契約は `fandhe_device::recent_documents`。パスは `ShellPath`（検査・正規化済み）だけを渡す。
//!
//! 結果コードの写像 [`outcome_from_code`] はホストでもテストできるようモジュールは
//! `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 登録後の OS の反映の時間差は守らない（一時的に `NotListed` になりうる）
//! - 素の実行体で一覧に載るかは Mac 目視（#968）で確かめる

use fandhe_device::recent_documents::RecentOutcome;
use fandhe_device::DeviceError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = 一覧に載っている / 1 = 載っていない / 2 = main thread ではない / 3 = 文字列変換の失敗・nil。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn outcome_from_code(code: i32) -> Result<RecentOutcome, DeviceError> {
    match code {
        0 => Ok(RecentOutcome::Listed),
        1 => Ok(RecentOutcome::NotListed),
        2 => Err(DeviceError::Internal("recent documents thread")),
        3 => Err(DeviceError::Internal("recent documents os")),
        _ => Err(DeviceError::Internal("recent documents outcome")),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::CString;

    // SAFETY: `macos/fandhe_macos_recent_documents.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_recent_documents_register(path: *const std::ffi::c_char) -> i32;
        fn fandhe_macos_recent_documents_check(path: *const std::ffi::c_char) -> i32;
    }

    /// 結果コードを返す。`CString` を作れなければ 3（文字列変換の失敗と同じ扱い）。
    #[allow(unsafe_code)]
    pub(super) fn call(path: &str, register: bool) -> i32 {
        let Ok(c) = CString::new(path) else {
            return 3;
        };
        // SAFETY: `c` はこの呼び出しの間だけ生きる NUL 終端文字列。`.m` はポインタを呼び出し中に
        // だけ読み、保持しない。
        unsafe {
            if register {
                fandhe_macos_recent_documents_register(c.as_ptr())
            } else {
                fandhe_macos_recent_documents_check(c.as_ptr())
            }
        }
    }
}

#[cfg(target_os = "macos")]
struct MacRecentDocuments;

#[cfg(target_os = "macos")]
impl fandhe_device::recent_documents::RecentDocuments for MacRecentDocuments {
    fn register(
        &self,
        path: &fandhe_device::recent_documents::ShellPath,
    ) -> Result<RecentOutcome, DeviceError> {
        outcome_from_code(ffi::call(path.as_str(), true))
    }

    fn check(
        &self,
        path: &fandhe_device::recent_documents::ShellPath,
    ) -> Result<RecentOutcome, DeviceError> {
        outcome_from_code(ffi::call(path.as_str(), false))
    }
}

/// macOS の `RecentDocuments` 実装を返す（`device/recent_documents.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::recent_documents::RecentDocuments> {
    Box::new(MacRecentDocuments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_table() {
        assert!(matches!(outcome_from_code(0), Ok(RecentOutcome::Listed)));
        assert!(matches!(outcome_from_code(1), Ok(RecentOutcome::NotListed)));
        assert!(matches!(
            outcome_from_code(2),
            Err(DeviceError::Internal("recent documents thread"))
        ));
        assert!(matches!(
            outcome_from_code(3),
            Err(DeviceError::Internal("recent documents os"))
        ));
        for code in [4, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_code(code),
                Err(DeviceError::Internal("recent documents outcome"))
            ));
        }
    }
}
