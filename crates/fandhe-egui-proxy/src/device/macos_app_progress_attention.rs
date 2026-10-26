//! macOS 向け進捗表示・注意喚起実装（Issue #860）。`macos/fandhe_macos_app_progress_attention.m`
//! （`NSDockTile.contentView` + `display`、`requestUserAttention:NSCriticalRequest`、`build.rs` が
//! macOS ターゲットの feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 3 関数で呼ぶ。
//! `unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。`device/app_progress_attention.rs`
//! の macOS 経路から呼ばれ、契約は `fandhe_device::app_progress_attention`。
//!
//! 結果コードの写像 [`outcome_from_code`] はホストでもテストできるようモジュールは
//! `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 未バンドル実行（`cargo run`）で Dock に出るかは未確認（Mac 目視は #852）
//! - 表示は確かめられないため成功は「OS へ渡した」に留める

use fandhe_device::DeviceError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = OS へ渡した、それ以外（NSApp 無し・main thread 外・未知）は `Internal(label)`。
pub(crate) fn outcome_from_code(code: i32, label: &'static str) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        _ => Err(DeviceError::Internal(label)),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_app_progress_attention.m`（`build.rs` が macOS ターゲットの
    // feature `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_app_progress_set(percent: u32) -> i32;
        fn fandhe_macos_app_progress_clear() -> i32;
        fn fandhe_macos_app_attention_request() -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn progress_set(percent: u32) -> i32 {
        // SAFETY: 値渡しの `u32` だけで、`.m` はポインタを受け取らず保持しない。
        unsafe { fandhe_macos_app_progress_set(percent) }
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn progress_clear() -> i32 {
        // SAFETY: 引数なし。`.m` は main thread 以外なら何もせず 2 を返す。
        unsafe { fandhe_macos_app_progress_clear() }
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn attention_request() -> i32 {
        // SAFETY: 引数なし。`.m` は main thread 以外なら何もせず 2 を返す。
        unsafe { fandhe_macos_app_attention_request() }
    }
}

#[cfg(target_os = "macos")]
struct MacAppProgressAttention;

#[cfg(target_os = "macos")]
impl fandhe_device::app_progress_attention::AppProgressAttention for MacAppProgressAttention {
    fn set_progress(
        &self,
        value: fandhe_device::app_progress_attention::ProgressValue,
    ) -> Result<(), DeviceError> {
        outcome_from_code(
            ffi::progress_set(value.get()),
            "app_progress_attention progress",
        )
    }

    fn clear_progress(&self) -> Result<(), DeviceError> {
        outcome_from_code(ffi::progress_clear(), "app_progress_attention clear")
    }

    fn request_attention(&self) -> Result<(), DeviceError> {
        outcome_from_code(ffi::attention_request(), "app_progress_attention attention")
    }
}

/// macOS の `AppProgressAttention` 実装を返す（`device/app_progress_attention.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation(
) -> Box<dyn fandhe_device::app_progress_attention::AppProgressAttention> {
    Box::new(MacAppProgressAttention)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_table() {
        assert!(outcome_from_code(0, "l").is_ok());
        for code in [1, 2, 3, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_code(code, "l"),
                Err(DeviceError::Internal("l"))
            ));
        }
    }
}
