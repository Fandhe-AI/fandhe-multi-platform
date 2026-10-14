//! ゲームコントローラ（ゲームパッド）入力の抽象（Phase 11 の拡充ループ、親 #932、
//! Issue #933）。`Motion`/`Location` と同じ継続 push 型（束 `continuous-push`）で、
//! 最新のスナップショットだけを保持する契約とする。
//!
//! # 契約（実装側が必ず守ること）
//!
//! - **値域**: スティックは各成分 `-1.0..=1.0`、トリガは `0.0..=1.0`。NaN は `0.0` へ
//!   置換してからクランプする（`motion::clamp_finite` と同じ順序、`f32::clamp` は
//!   NaN を素通しするため先に有限値化しないと契約を破る）。
//! - **軸の向き**: `fandhe_core::screens::tilt_ball` と同じ「画面座標系・y 下が正」に
//!   統一する（キャンバス左上原点、x 右・y 下が正）。XInput / `Windows.Gaming.Input` /
//!   `GCController` は y 上が正で返るため、**各 OS 実装側で符号を反転**してから
//!   [`GamepadSnapshot`] へ詰める。玉が転がる型サンプルの絵をそのまま流用するための
//!   統一であり、「上を正にする」慣例をここでは採らない。
//! - **未接続 vs 非対応 vs 読み取り失敗**: [`Gamepad::read`] は 3 通りを型で区別する。
//!   - 接続中のコントローラが無い: `Ok(None)`（全 0 の [`GamepadSnapshot`] を
//!     「未接続」の代用にしない。`location.rs` の「(0, 0) に測位できた」と
//!     「まだ測位できていない」の区別と同じ設計判断）。
//!   - 現在のプラットフォームでは未対応: `Err(DeviceError::Unsupported("gamepad"))`。
//!   - 接続はしているが読み取りが一時的に失敗した: `Err(DeviceError::Internal("gamepad
//!     read"))`。
//! - **複数台接続**: 先頭 1 台のみを扱う。2 台目以降は無視する（本 Issue のスコープ外、
//!   複数台の合成・切替は将来課題）。
//! - **切断後**: 直前の値を返し続けない。切断を検知したら次回以降の [`Gamepad::read`]
//!   は速やかに `Ok(None)` へ戻す（`location.rs::push_failure` が保持していた `Fix` を
//!   破棄するのと同じ「古い値の張り付き」対策）。

use std::sync::{Arc, Mutex};

use crate::DeviceError;

/// スティック・トリガ・ボタンの最新スナップショット。モジュール doc の値域・軸の
/// 向きの契約に従う。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GamepadSnapshot {
    /// 左スティック `(x, y)`。各成分 `-1.0..=1.0`（画面座標系、y 下が正）。
    pub left_stick: (f32, f32),
    /// 右スティック `(x, y)`。[`left_stick`](Self::left_stick) と同じ値域・向き。
    pub right_stick: (f32, f32),
    /// 左トリガ（`0.0..=1.0`）。
    pub left_trigger: f32,
    /// 右トリガ（`0.0..=1.0`）。
    pub right_trigger: f32,
    /// 押下中ボタンのビットマスク（[`BUTTON_*`](self) の OR 和）。未定義ビットは
    /// [`pressed_labels`] が無視する。
    pub buttons: u16,
}

/// ボタンのビット定数。1 実装しかない列挙型・個別構造体は作らず、`u16` ビットマスクに
/// 一本化する（実装が Windows のみの本 Issue の時点で複数実装向けの抽象を先取りしない）。
pub const BUTTON_A: u16 = 1 << 0;
pub const BUTTON_B: u16 = 1 << 1;
pub const BUTTON_X: u16 = 1 << 2;
pub const BUTTON_Y: u16 = 1 << 3;
pub const BUTTON_LEFT_SHOULDER: u16 = 1 << 4;
pub const BUTTON_RIGHT_SHOULDER: u16 = 1 << 5;
pub const BUTTON_LEFT_STICK: u16 = 1 << 6;
pub const BUTTON_RIGHT_STICK: u16 = 1 << 7;
pub const BUTTON_DPAD_UP: u16 = 1 << 8;
pub const BUTTON_DPAD_DOWN: u16 = 1 << 9;
pub const BUTTON_DPAD_LEFT: u16 = 1 << 10;
pub const BUTTON_DPAD_RIGHT: u16 = 1 << 11;
pub const BUTTON_MENU: u16 = 1 << 12;
pub const BUTTON_VIEW: u16 = 1 << 13;

/// 定義済みビットとラベルの対応表（[`pressed_labels`] の唯一の SSOT）。
const BUTTON_LABELS: &[(u16, &str)] = &[
    (BUTTON_A, "A"),
    (BUTTON_B, "B"),
    (BUTTON_X, "X"),
    (BUTTON_Y, "Y"),
    (BUTTON_LEFT_SHOULDER, "LB"),
    (BUTTON_RIGHT_SHOULDER, "RB"),
    (BUTTON_LEFT_STICK, "左スティック押込み"),
    (BUTTON_RIGHT_STICK, "右スティック押込み"),
    (BUTTON_DPAD_UP, "十字キー上"),
    (BUTTON_DPAD_DOWN, "十字キー下"),
    (BUTTON_DPAD_LEFT, "十字キー左"),
    (BUTTON_DPAD_RIGHT, "十字キー右"),
    (BUTTON_MENU, "メニュー"),
    (BUTTON_VIEW, "ビュー"),
];

/// `buttons` ビットマスクから押下中ボタンのラベル一覧を返す（定義順）。未定義ビットは
/// 無視する（OS から想定外の値が来ても panic・未定義動作にしない、security.md A03）。
pub fn pressed_labels(buttons: u16) -> Vec<&'static str> {
    BUTTON_LABELS
        .iter()
        .filter(|(bit, _)| buttons & bit != 0)
        .map(|(_, label)| *label)
        .collect()
}

/// ゲームコントローラ入力の契約（`Motion`/`Location` と同じくスレッド間で
/// 共有されうるため `Send + Sync`）。
pub trait Gamepad: Send + Sync {
    /// 最新のスナップショットを返す。モジュール doc の 3 分岐（未接続 `Ok(None)` /
    /// 非対応 `Unsupported` / 読み取り失敗 `Internal`）を守る契約。
    fn read(&self) -> Result<Option<GamepadSnapshot>, DeviceError>;
}

/// NaN を `0.0` へ置換してから `-1.0..=1.0` へ丸める（`motion::clamp_finite` と同型）。
pub fn normalize_axis(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-1.0, 1.0)
    }
}

/// NaN を `0.0` へ置換してから `0.0..=1.0` へ丸める（トリガ用、[`normalize_axis`] と
/// 同じ NaN 対策の順序）。
pub fn normalize_trigger(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

/// テスト・代替入力（未接続 / 非対応時のスライダー代替）用のダミー実装。実装の
/// 初期化失敗時にこれへ黙ってフォールバックしない契約は呼び出し側
/// （`fandhe-egui-proxy::device::gamepad`）が担う。
#[derive(Clone)]
pub struct DummyGamepad(Arc<Mutex<Option<GamepadSnapshot>>>);

impl DummyGamepad {
    /// 初期状態は「未接続」（`None`）。
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }

    /// スナップショットを書き込む（「接続した」扱いにする）。各値はモジュール doc の
    /// 値域へ丸めてから保存する。
    pub fn set(&self, snapshot: GamepadSnapshot) {
        let normalized = GamepadSnapshot {
            left_stick: (
                normalize_axis(snapshot.left_stick.0),
                normalize_axis(snapshot.left_stick.1),
            ),
            right_stick: (
                normalize_axis(snapshot.right_stick.0),
                normalize_axis(snapshot.right_stick.1),
            ),
            left_trigger: normalize_trigger(snapshot.left_trigger),
            right_trigger: normalize_trigger(snapshot.right_trigger),
            buttons: snapshot.buttons,
        };
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = Some(normalized);
    }

    /// 「未接続」へ戻す（切断後に古い値を返さない契約、モジュール doc 参照）。
    pub fn disconnect(&self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = None;
    }
}

impl Default for DummyGamepad {
    fn default() -> Self {
        Self::new()
    }
}

impl Gamepad for DummyGamepad {
    fn read(&self) -> Result<Option<GamepadSnapshot>, DeviceError> {
        // 他所の panic に由来する poison を呼び出し側へ伝播させない
        // （coding-rust.md: unwrap/expect を本実装コードで使わない方針）。
        let state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(*state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> GamepadSnapshot {
        GamepadSnapshot {
            left_stick: (0.5, -0.5),
            right_stick: (-0.25, 0.25),
            left_trigger: 0.3,
            right_trigger: 0.7,
            buttons: BUTTON_A | BUTTON_DPAD_UP,
        }
    }

    /// `Gamepad` 契約側の非対応経路（`Unsupported`）を確認する最小モック。
    struct UnsupportedGamepad;

    impl Gamepad for UnsupportedGamepad {
        fn read(&self) -> Result<Option<GamepadSnapshot>, DeviceError> {
            Err(DeviceError::Unsupported("gamepad"))
        }
    }

    #[test]
    fn unsupported_platform_returns_unsupported_error() {
        let gamepad = UnsupportedGamepad;
        assert!(matches!(
            gamepad.read(),
            Err(DeviceError::Unsupported("gamepad"))
        ));
    }

    #[test]
    fn initial_read_is_ok_none() {
        let gamepad = DummyGamepad::new();
        assert_eq!(gamepad.read().unwrap(), None);
    }

    #[test]
    fn set_then_read_returns_latest_snapshot() {
        let gamepad = DummyGamepad::new();
        gamepad.set(sample_snapshot());
        assert_eq!(gamepad.read().unwrap(), Some(sample_snapshot()));
    }

    #[test]
    fn disconnect_after_set_returns_to_none_and_does_not_keep_stale_value() {
        let gamepad = DummyGamepad::new();
        gamepad.set(sample_snapshot());
        gamepad.disconnect();
        assert_eq!(gamepad.read().unwrap(), None);
    }

    #[test]
    fn set_clamps_out_of_range_stick_and_trigger_values() {
        let gamepad = DummyGamepad::new();
        gamepad.set(GamepadSnapshot {
            left_stick: (2.0, -2.0),
            right_stick: (0.0, 0.0),
            left_trigger: -1.0,
            right_trigger: 2.0,
            buttons: 0,
        });
        let snapshot = gamepad.read().unwrap().unwrap();
        assert_eq!(snapshot.left_stick, (1.0, -1.0));
        assert_eq!(snapshot.left_trigger, 0.0);
        assert_eq!(snapshot.right_trigger, 1.0);
    }

    #[test]
    fn set_replaces_nan_axis_and_trigger_with_zero() {
        let gamepad = DummyGamepad::new();
        gamepad.set(GamepadSnapshot {
            left_stick: (f32::NAN, 0.5),
            right_stick: (0.0, 0.0),
            left_trigger: f32::NAN,
            right_trigger: 0.5,
            buttons: 0,
        });
        let snapshot = gamepad.read().unwrap().unwrap();
        assert_eq!(snapshot.left_stick.0, 0.0);
        assert_eq!(snapshot.left_stick.1, 0.5);
        assert_eq!(snapshot.left_trigger, 0.0);
    }

    #[test]
    fn normalize_axis_clamps_and_replaces_nan() {
        assert_eq!(normalize_axis(2.0), 1.0);
        assert_eq!(normalize_axis(-2.0), -1.0);
        assert_eq!(normalize_axis(f32::NAN), 0.0);
        assert_eq!(normalize_axis(0.3), 0.3);
    }

    #[test]
    fn normalize_trigger_clamps_and_replaces_nan() {
        assert_eq!(normalize_trigger(-1.0), 0.0);
        assert_eq!(normalize_trigger(2.0), 1.0);
        assert_eq!(normalize_trigger(f32::NAN), 0.0);
        assert_eq!(normalize_trigger(0.6), 0.6);
    }

    #[test]
    fn pressed_labels_lists_only_set_bits_in_defined_order() {
        let labels = pressed_labels(BUTTON_B | BUTTON_MENU);
        assert_eq!(labels, vec!["B", "メニュー"]);
    }

    #[test]
    fn pressed_labels_ignores_undefined_bits_without_panicking() {
        // 15 番目のビット（未定義）を含めても panic せず、定義済みビットのみ返す。
        let labels = pressed_labels(BUTTON_A | (1 << 15));
        assert_eq!(labels, vec!["A"]);
    }

    #[test]
    fn pressed_labels_empty_when_no_buttons_pressed() {
        assert!(pressed_labels(0).is_empty());
    }

    #[test]
    fn clone_shares_state() {
        let gamepad = DummyGamepad::new();
        let cloned = gamepad.clone();
        cloned.set(sample_snapshot());
        assert_eq!(gamepad.read().unwrap(), Some(sample_snapshot()));
    }
}
