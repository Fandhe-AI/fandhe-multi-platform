//! Windows 向けゲームコントローラ実装（Issue #933）。`device/gamepad.rs` の
//! `shared_gamepad()` の Windows 経路から呼ばれ、WinRT
//! `Windows.Gaming.Input.Gamepad`（<https://learn.microsoft.com/en-us/uwp/api/windows.gaming.input.gamepad>）
//! を読む。
//!
//! `Gamepad::Gamepads()`/`GetCurrentReading()` は `windows` クレート側で safe fn として
//! ラップ済みのため `unsafe` は不要（`windows_text_scale.rs` と同じ判断。
//! `fandhe-egui-proxy` の Windows `deny(unsafe_code)` に対する `#[allow(unsafe_code)]` も
//! 付けない）。先頭 1 台のみを扱う契約（`fandhe_device::gamepad` モジュール doc 参照）で、
//! 複数台接続時は `Gamepads()` が返す一覧の先頭要素のみを見る。
//!
//! `LeftThumbstickY`/`RightThumbstickY` は WinRT では上方向が正だが、`fandhe_device::gamepad`
//! の契約は画面座標系（y 下が正、`tilt_ball` と同じ）のため、ここで符号を反転してから
//! [`fandhe_device::gamepad::GamepadSnapshot`] へ詰める（モジュール doc の軸の向き参照）。
//!
//! `HRESULT` の生値をログ・エラー文へ出さず、固定ラベルの `DeviceError::Internal` へ
//! 畳む（`windows_text_scale.rs`/`windows_share.rs` と同じ方針、security.md A09）。

use fandhe_device::gamepad::{
    normalize_axis, normalize_trigger, Gamepad as GamepadTrait, GamepadSnapshot, BUTTON_A,
    BUTTON_B, BUTTON_DPAD_DOWN, BUTTON_DPAD_LEFT, BUTTON_DPAD_RIGHT, BUTTON_DPAD_UP,
    BUTTON_LEFT_SHOULDER, BUTTON_LEFT_STICK, BUTTON_MENU, BUTTON_RIGHT_SHOULDER,
    BUTTON_RIGHT_STICK, BUTTON_VIEW, BUTTON_X, BUTTON_Y,
};
use fandhe_device::DeviceError;
use windows::Gaming::Input::{Gamepad as WinGamepad, GamepadButtons, GamepadReading};

/// [`fandhe_device::gamepad::Gamepad`] の Windows 実装。状態を持たない
/// （`Gamepads()` は毎回 OS 側の一覧を引く。`windows_text_scale.rs` と同じ判断で、
/// 常駐スレッド・イベント購読は持たない、ホットプラグ通知は本 Issue のスコープ外）。
pub(crate) struct WindowsGamepad;

/// `GamepadButtons`（WinRT）から [`fandhe_device::gamepad`] のビットマスクへの写像
/// （SSOT を本関数に一本化する）。`Paddle*` は本 Issue の [`fandhe_device::gamepad`]
/// ビット定数に対応が無いため無視する（スコープ外、モジュール doc 参照）。
fn map_buttons(buttons: GamepadButtons) -> u16 {
    let mut mask = 0u16;
    let pairs: &[(GamepadButtons, u16)] = &[
        (GamepadButtons::A, BUTTON_A),
        (GamepadButtons::B, BUTTON_B),
        (GamepadButtons::X, BUTTON_X),
        (GamepadButtons::Y, BUTTON_Y),
        (GamepadButtons::LeftShoulder, BUTTON_LEFT_SHOULDER),
        (GamepadButtons::RightShoulder, BUTTON_RIGHT_SHOULDER),
        (GamepadButtons::LeftThumbstick, BUTTON_LEFT_STICK),
        (GamepadButtons::RightThumbstick, BUTTON_RIGHT_STICK),
        (GamepadButtons::DPadUp, BUTTON_DPAD_UP),
        (GamepadButtons::DPadDown, BUTTON_DPAD_DOWN),
        (GamepadButtons::DPadLeft, BUTTON_DPAD_LEFT),
        (GamepadButtons::DPadRight, BUTTON_DPAD_RIGHT),
        (GamepadButtons::Menu, BUTTON_MENU),
        (GamepadButtons::View, BUTTON_VIEW),
    ];
    for (winrt_bit, fandhe_bit) in pairs {
        if buttons.contains(*winrt_bit) {
            mask |= fandhe_bit;
        }
    }
    mask
}

/// WinRT の `GamepadReading`（y 上が正）を [`GamepadSnapshot`]（画面座標系、y 下が正）へ
/// 変換する純関数（`cfg` 無しでホストの `cargo test` から検証できるよう分離する）。
fn reading_to_snapshot(reading: GamepadReading) -> GamepadSnapshot {
    GamepadSnapshot {
        left_stick: (
            normalize_axis(reading.LeftThumbstickX as f32),
            normalize_axis(-reading.LeftThumbstickY as f32),
        ),
        right_stick: (
            normalize_axis(reading.RightThumbstickX as f32),
            normalize_axis(-reading.RightThumbstickY as f32),
        ),
        left_trigger: normalize_trigger(reading.LeftTrigger as f32),
        right_trigger: normalize_trigger(reading.RightTrigger as f32),
        buttons: map_buttons(reading.Buttons),
    }
}

impl GamepadTrait for WindowsGamepad {
    fn read(&self) -> Result<Option<GamepadSnapshot>, DeviceError> {
        let gamepads = WinGamepad::Gamepads().map_err(|_| DeviceError::Internal("gamepad read"))?;
        let Ok(first) = gamepads.GetAt(0) else {
            // 接続中のコントローラが無い（`GetAt` の範囲外エラー）。
            // モジュール doc の契約: 全 0 スナップショットではなく `Ok(None)` を返す。
            return Ok(None);
        };
        let reading = first
            .GetCurrentReading()
            .map_err(|_| DeviceError::Internal("gamepad read"))?;
        Ok(Some(reading_to_snapshot(reading)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(
        buttons: GamepadButtons,
        left: (f64, f64),
        right: (f64, f64),
        lt: f64,
        rt: f64,
    ) -> GamepadReading {
        GamepadReading {
            Timestamp: 0,
            Buttons: buttons,
            LeftTrigger: lt,
            RightTrigger: rt,
            LeftThumbstickX: left.0,
            LeftThumbstickY: left.1,
            RightThumbstickX: right.0,
            RightThumbstickY: right.1,
        }
    }

    #[test]
    fn reading_to_snapshot_flips_y_axis_for_screen_coordinates() {
        // WinRT は上方向が正、fandhe_device::gamepad の契約は画面座標系（y 下が正）。
        let snapshot = reading_to_snapshot(reading(
            GamepadButtons::None,
            (0.0, 1.0),
            (0.0, -1.0),
            0.0,
            0.0,
        ));
        assert_eq!(snapshot.left_stick, (0.0, -1.0));
        assert_eq!(snapshot.right_stick, (0.0, 1.0));
    }

    #[test]
    fn reading_to_snapshot_clamps_and_normalizes_triggers() {
        let snapshot = reading_to_snapshot(reading(
            GamepadButtons::None,
            (0.0, 0.0),
            (0.0, 0.0),
            2.0,
            -1.0,
        ));
        assert_eq!(snapshot.left_trigger, 1.0);
        assert_eq!(snapshot.right_trigger, 0.0);
    }

    #[test]
    fn map_buttons_translates_winrt_bits_to_fandhe_bits() {
        let mask = map_buttons(GamepadButtons::A | GamepadButtons::DPadUp);
        assert_eq!(mask, BUTTON_A | BUTTON_DPAD_UP);
    }

    #[test]
    fn map_buttons_ignores_paddles_out_of_scope() {
        let mask = map_buttons(GamepadButtons::Paddle1 | GamepadButtons::A);
        assert_eq!(mask, BUTTON_A);
    }

    #[test]
    fn map_buttons_returns_zero_for_no_buttons() {
        assert_eq!(map_buttons(GamepadButtons::None), 0);
    }
}
