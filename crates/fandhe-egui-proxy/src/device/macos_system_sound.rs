//! macOS 向けシステムの効果音実装（Issue #939）。`macos/fandhe_macos_system_sound.m`
//! （`NSBeep`、`build.rs` が macOS ターゲットの feature `device` 有効時にコンパイルする）を
//! `extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/system_sound.rs` の macOS 経路から main スレッドで呼ばれ、契約は
//! `fandhe_device::system_sound`。
//!
//! 結果コードの写像 [`outcome_from_code`] はホストでもテストできるようモジュールは
//! `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - `NSBeep` は戻り値がなく、鳴ったかは検知できない。成功は `Requested`（OS に渡した）まで
//! - 実際に聞こえるかの Mac 目視は #935

use fandhe_device::system_sound::BeepOutcome;
use fandhe_device::DeviceError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = OS に渡した、2 = main スレッド以外。
pub(crate) fn outcome_from_code(code: i32) -> Result<BeepOutcome, DeviceError> {
    match code {
        0 => Ok(BeepOutcome::Requested),
        2 => Err(DeviceError::Internal("system_sound not on main thread")),
        _ => Err(DeviceError::Internal("system_sound")),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_system_sound.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_system_sound_beep() -> i32;
    }

    /// 0 = OS に渡した。
    #[allow(unsafe_code)]
    pub(super) fn beep() -> i32 {
        // SAFETY: 引数もポインタも持たず、`.m` は何も保持しない。main 以外なら `.m` が
        // AppKit に触らず 2 を返す。
        unsafe { fandhe_macos_system_sound_beep() }
    }
}

#[cfg(target_os = "macos")]
struct MacosSystemSound;

#[cfg(target_os = "macos")]
impl fandhe_device::system_sound::SystemSound for MacosSystemSound {
    fn beep(&self) -> Result<BeepOutcome, DeviceError> {
        outcome_from_code(ffi::beep())
    }
}

/// macOS の `SystemSound` 実装を返す（`device/system_sound.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::system_sound::SystemSound> {
    std::sync::Arc::new(MacosSystemSound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_table() {
        assert!(matches!(outcome_from_code(0), Ok(BeepOutcome::Requested)));
        assert!(matches!(
            outcome_from_code(2),
            Err(DeviceError::Internal("system_sound not on main thread"))
        ));
        for code in [1, -1, 99] {
            assert!(matches!(
                outcome_from_code(code),
                Err(DeviceError::Internal("system_sound"))
            ));
        }
    }
}
