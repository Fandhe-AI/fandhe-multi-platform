//! macOS / iOS 向け触覚フィードバック実装（Issue #1091・#1092）。`device/haptics.rs` の
//! Apple 経路から呼ばれ、`apple/fandhe_apple_haptics.m`（macOS は `NSHapticFeedbackManager`、
//! iOS は `UIImpactFeedbackGenerator`。`build.rs` が feature `device` 有効時にコンパイル）を `unsafe extern "C"` 1 関数で
//! 呼ぶ。`unsafe` はこの呼び出し 1 箇所に閉じる（`apple_share.rs` と同型、coding-rust.md の
//! FFI 例外規定準拠）。
//!
//! macOS は非搭載を判定できない（Force Touch トラックパッド以外では何も起きない）ため
//! `Unsupported` は返さない。iOS は `CHHapticEngine` で非搭載を判定でき、`.m` が 3 を返す。
//! `Ok` は「要求を渡した」まで（`fandhe_device::haptics` の契約）。
//! `build.rs` のコンパイル条件・`device.rs` の mod cfg と同じ OS 集合に保つ。

use fandhe_device::haptics::Haptics;
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_haptics.m` が定義する C ABI（`build.rs` が macOS / iOS ターゲットの
// feature `device` 有効時のみコンパイル・リンクする）。引数なし・ポインタなし・
// 所有権の移転なし。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_haptics_tap() -> i32;
}

/// `NSHapticFeedbackManager`（macOS）/ `UIImpactFeedbackGenerator`（iOS）で軽い振動を 1 回要求する実装。
pub(crate) struct AppleHaptics;

impl Haptics for AppleHaptics {
    #[allow(unsafe_code)]
    fn tap(&self) -> Result<(), DeviceError> {
        // SAFETY: 引数・戻り値とも値渡しの `i32` のみ。`.m` 側は main スレッド以外から
        // 呼ばれると AppKit / UIKit に触れずに 2 を返す（effect は main の update から同期で呼ぶ）。
        let code = unsafe { fandhe_apple_haptics_tap() };
        match code {
            0 => Ok(()),
            2 => Err(DeviceError::Internal("haptics not on main thread")),
            3 => Err(DeviceError::Unsupported("haptics")),
            _ => Err(DeviceError::Internal("haptics")),
        }
    }
}
