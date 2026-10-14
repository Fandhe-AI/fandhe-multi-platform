//! iOS / macOS 向けゲームコントローラ実装（Issue #934、親 #932）。
//! `device/gamepad.rs::platform_gamepad` の Apple 分岐（`any(target_os = "ios",
//! target_os = "macos")`）から呼ばれ、GameController.framework（`GCController`）
//! 実装（`apple/fandhe_apple_gamepad.m`、`build.rs` が iOS・macOS 双方の
//! feature `device` 有効時のみコンパイル・リンクする）を `unsafe extern "C"` 1 関数
//! （[`AppleGamepad::read`]）で呼ぶ。`unsafe` はこの FFI 呼び出し 1 箇所に閉じ
//! `// SAFETY:` で契約を明記する（`ios.rs::IosMotion`/`apple_location.rs::start` と
//! 同型、`lib.rs` の iOS/macOS `deny(unsafe_code)` 緩和・coding-rust.md の FFI
//! 例外規定準拠）。
//!
//! **pull 型**（push 通知ではない）: `device/windows_gamepad.rs::WindowsGamepad` と
//! 同じく状態を一切キャッシュせず、`read()` のたびに `.m` 側の現在値を読み直す
//! （設計判断は `.m` ファイル冒頭コメント参照）。
//!
//! 軸・ボタンビットの対称性は `.m` ファイル冒頭コメントの表と同一。`.m` 側は
//! OS の生値（y 上が正）をそのまま返し、符号反転・値域クランプ・NaN 置換は
//! [`result_from_code`]（本ファイル）でのみ行う（テスト可能な側に責務を寄せる、
//! `windows_gamepad.rs::reading_to_snapshot` と同じ配置）。
//!
//! Apple 実装は `DeviceError::Unsupported` を返さない: iOS 17 / macOS 11 の両方で
//! GameController.framework は常に存在するため（実装計画 §3.3）。
//!
//! 入力値（スティック値・ボタン名）をログへ出力しない（security.md A09）。

use fandhe_device::gamepad::{
    normalize_axis, normalize_trigger, Gamepad, GamepadSnapshot, BUTTON_A, BUTTON_B,
    BUTTON_DPAD_DOWN, BUTTON_DPAD_LEFT, BUTTON_DPAD_RIGHT, BUTTON_DPAD_UP, BUTTON_LEFT_SHOULDER,
    BUTTON_LEFT_STICK, BUTTON_MENU, BUTTON_RIGHT_SHOULDER, BUTTON_RIGHT_STICK, BUTTON_VIEW,
    BUTTON_X, BUTTON_Y,
};
use fandhe_device::DeviceError;

/// `.m` 側が既知ビットとして書きうる全ビットの OR 和。想定外の未定義ビットを
/// AND で落とす（`fandhe_device::gamepad::pressed_labels` が未定義ビットを無視する
/// のと同じ security.md A03 判断。OS から来る値を信頼境界の入力として扱う）。
const KNOWN_BUTTONS_MASK: u16 = BUTTON_A
    | BUTTON_B
    | BUTTON_X
    | BUTTON_Y
    | BUTTON_LEFT_SHOULDER
    | BUTTON_RIGHT_SHOULDER
    | BUTTON_LEFT_STICK
    | BUTTON_RIGHT_STICK
    | BUTTON_DPAD_UP
    | BUTTON_DPAD_DOWN
    | BUTTON_DPAD_LEFT
    | BUTTON_DPAD_RIGHT
    | BUTTON_MENU
    | BUTTON_VIEW;

// SAFETY: `apple/fandhe_apple_gamepad.m` が定義する C ABI（`build.rs` が iOS・macOS
// 両ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は `.m`
// ファイル冒頭コメントおよび下記 `AppleGamepad::read` 実装のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_gamepad_read(out_axes: *mut f32, out_buttons: *mut u16) -> i32;
}

/// [`fandhe_device::gamepad::Gamepad`] の iOS / macOS 実装（`GCController` を
/// pull で読む、モジュール doc 参照）。状態を持たない（`WindowsGamepad` と同じ判断）。
pub(crate) struct AppleGamepad;

impl Gamepad for AppleGamepad {
    #[allow(unsafe_code)]
    fn read(&self) -> Result<Option<GamepadSnapshot>, DeviceError> {
        let mut axes = [0.0f32; 6];
        let mut buttons: u16 = 0;

        // SAFETY: `fandhe_apple_gamepad_read` の契約（`.m` ファイル冒頭コメント）:
        // out 引数はこの関数呼び出し中のみ有効なスタック上の領域（`axes` は 6 要素、
        // `buttons` は 1 個の `u16`）で、`.m` 側が読み書きする長さと一致する。
        // 所有権移転・malloc は無く、呼び出し後に解放処理は不要（`ios.rs::IosMotion`
        // と同型）。戻り値が 0 以外のとき `.m` 側は out 引数を書き換えない契約のため、
        // その場合は呼び出し直前の初期値 `[0.0; 6]`/`0` のまま扱う（実際には
        // `result_from_code` が 0 以外の code では axes/buttons を一切見ないため、
        // 初期値が読まれること自体が無い）。main スレッド以外からの呼び出しは
        // `.m` 側が戻り値 2 を返すだけでメモリ安全性に影響しない。
        let code = unsafe { fandhe_apple_gamepad_read(axes.as_mut_ptr(), &mut buttons) };

        result_from_code(code, axes, buttons)
    }
}

/// `.m` 側の戻りコードから [`fandhe_device::gamepad::Gamepad::read`] の契約
/// （3 分岐: 未接続 `Ok(None)` / 読み取り失敗 `Err(Internal)` / 接続中
/// `Ok(Some(..))`）へ写す純関数（cfg 無しで全ターゲットからテストできる、
/// `windows_gamepad.rs::reading_to_snapshot` と同じ切り出し方針）。
///
/// - `code == 0`: 接続中。`axes`/`buttons` は `.m` 側の生値。y 軸は画面座標系
///   （y 下が正、`fandhe_device::gamepad` モジュール doc 参照）へ符号反転し、
///   `normalize_axis`/`normalize_trigger` で NaN 置換・クランプする。ボタンは
///   [`KNOWN_BUTTONS_MASK`] で未定義ビットを落とす。
/// - `code == 1`: 使えるコントローラが未接続。`axes`/`buttons` の中身は一切見ない
///   （古い値・ゴミ値を接続中として漏らさない）。
/// - それ以外（2 / 3 / 未知）: 偽の成功・偽の未接続を作らず `Err(Internal)` へ畳む
///   （呼び出し元 `device/gamepad.rs::resolve_frame` は `DisconnectedAlt` 表示へ倒す）。
fn result_from_code(
    code: i32,
    axes: [f32; 6],
    buttons: u16,
) -> Result<Option<GamepadSnapshot>, DeviceError> {
    match code {
        0 => Ok(Some(GamepadSnapshot {
            left_stick: (normalize_axis(axes[0]), normalize_axis(-axes[1])),
            right_stick: (normalize_axis(axes[2]), normalize_axis(-axes[3])),
            left_trigger: normalize_trigger(axes[4]),
            right_trigger: normalize_trigger(axes[5]),
            buttons: buttons & KNOWN_BUTTONS_MASK,
        })),
        1 => Ok(None),
        _ => Err(DeviceError::Internal("gamepad read")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_1_is_ok_none_even_if_buffers_hold_values() {
        // 古い値・ゴミ値を接続中として漏らさない契約（未接続との区別は code のみ）。
        let axes = [0.5, 0.5, 0.5, 0.5, 0.5, 0.5];
        assert_eq!(result_from_code(1, axes, BUTTON_A).unwrap(), None);
    }

    #[test]
    fn code_2_3_and_unknown_are_internal_error() {
        for code in [2, 3, -1, 99] {
            let result = result_from_code(code, [0.0; 6], 0);
            assert!(matches!(result, Err(DeviceError::Internal("gamepad read"))));
        }
    }

    #[test]
    fn code_0_flips_y_axis() {
        // .m 側は OS の生値（y 上が正）を渡す。契約は画面座標系（y 下が正）。
        let axes = [0.0, 1.0, 0.0, -1.0, 0.0, 0.0];
        let snapshot = result_from_code(0, axes, 0).unwrap().unwrap();
        assert_eq!(snapshot.left_stick, (0.0, -1.0));
        assert_eq!(snapshot.right_stick, (0.0, 1.0));
    }

    #[test]
    fn code_0_clamps_and_replaces_nan() {
        let axes = [2.0, f32::NAN, -2.0, 0.0, -1.0, 2.0];
        let snapshot = result_from_code(0, axes, 0).unwrap().unwrap();
        assert_eq!(snapshot.left_stick, (1.0, 0.0));
        assert_eq!(snapshot.right_stick, (-1.0, 0.0));
        assert_eq!(snapshot.left_trigger, 0.0);
        assert_eq!(snapshot.right_trigger, 1.0);
    }

    #[test]
    fn code_0_masks_undefined_button_bits() {
        let snapshot = result_from_code(0, [0.0; 6], BUTTON_A | (1 << 15))
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.buttons, BUTTON_A);
    }

    #[test]
    fn code_0_all_zero_is_connected_snapshot() {
        // 接続中で無入力の全 0 と未接続の区別は code のみが担う。
        let snapshot = result_from_code(0, [0.0; 6], 0).unwrap();
        assert!(snapshot.is_some());
    }
}
