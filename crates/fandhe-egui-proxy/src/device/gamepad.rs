//! Phase 11 の拡充ループ（親 #932）型サンプル「ゲームコントローラ」（Issue #933）の
//! effect ドライバ。`fandhe_core::screens::gamepad_show`（端末依存ゼロ）へ、
//! `fandhe_device::gamepad::Gamepad` から読んだ最新スナップショットを渡す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # 実装スコープ（Issue #933・#934・#935）
//!
//! Windows は [`WindowsGamepad`](crate::device::windows_gamepad::WindowsGamepad)、
//! iOS / macOS は [`AppleGamepad`](crate::device::apple_gamepad::AppleGamepad)
//! （`GCController` を pull で読む、`device/apple_gamepad.rs` 参照）、Android は
//! [`AndroidGamepad`](crate::device::android_gamepad::AndroidGamepad)
//! （`device/android_gamepad.rs` 参照）を使う。それ以外（Linux/その他）は常に
//! `DeviceError::Unsupported("gamepad")` を返す `UnsupportedGamepad` にする
//! （`location.rs` の cfg カスケードと同型。ダミー座標へフォールバックしない）。
//!
//! 代替入力（スライダー・チェックボックス）は `fandhe_core::screens::gamepad_show::
//! GamepadShowState` 自身の `alt_x`/`alt_y`/`alt_button` フィールドとして画面側で
//! 直接バインドされている（`gamepad_show_screen` の `.slider()`/`.checkbox()`
//! 参照）ため、`tilt.rs`/`location.rs` の `DummyMotion`/`SharedLocation` のような
//! 別建ての共有ダミーオブジェクトは本ドライバでは持たない（実装が非対応・未接続の
//! ときに [`fandhe_core::screens::gamepad_show::apply_disconnected`]/[`apply_unsupported`]
//! が状態の `alt_x`/`alt_y`/`alt_button` をそのまま読む設計、ponytail: 不要な抽象を
//! 増やさない）。

use std::sync::Mutex;
use std::time::Instant;

use fandhe_core::screens::gamepad_show::{
    apply_connected, apply_disconnected, apply_unsupported, is_at_rest, pressed_text,
    GamepadShowState, GamepadView,
};
use fandhe_device::gamepad::{pressed_labels, Gamepad};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 1 フレームで進める時間の上限（`tilt_ball::step` と同じ値）。
const MAX_DT: f32 = 0.1;
/// `LAST_TICK` が未初期化（起動直後の 1 フレーム目）のときに使う既定 dt（60fps 相当）。
const DEFAULT_DT: f32 = 1.0 / 60.0;
/// 「傾き無し」とみなす閾値（`device/tilt.rs::TILT_EPSILON` と同じ考え方）。
const STICK_EPSILON: f32 = 0.02;
/// 押下中ボタン名の区切り文字。
const LABEL_SEPARATOR: &str = "、";

/// Windows / iOS / macOS（いずれもプル方式、`WindowsGamepad`/`AppleGamepad` モジュール
/// doc 参照）で静止判定によりガードした後、次に OS 側の現在値を読み直すまでの間隔。
///
/// これらの実装はホットプラグ通知・イベント購読を持たない（モジュール doc 参照）ため、
/// スティック中央化・押下解除で送信をスキップした後に再描画予約を切ると、次に
/// `ProxyApp::update` が呼ばれる機会（他の UI 入力）が無い限りポーリングが止まり、
/// 以後スティックを動かしても・未接続からコントローラを接続しても検知できない
/// （codex-review 指摘 P1・Cursor Bugbot 指摘 High、PR #937）。`device/tilt.rs::
/// IOS_IDLE_POLL_INTERVAL` と同じ考え方で、スキップ中も定期的に起こし直す。
/// ponytail: 固定間隔のポーリング。取りこぼし（最大この間隔分の反応遅延）が要件上
/// 問題になった場合は、`Windows.Gaming.Input`/`GCControllerDidConnectNotification`
/// のイベント購読へ上げる。
#[cfg(any(target_os = "windows", target_os = "ios", target_os = "macos"))]
const IDLE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// 常に非対応を返す実装（Windows/iOS/macOS/Android 以外の cfg 分岐、`location.rs` の
/// `UNSUPPORTED_PUSHED` 相当の判断をここでは実装自体で表現する）。それらのビルドでは
/// `platform_gamepad()` の cfg 分岐から参照されないため `dead_code` を許可する
/// （`device/tilt.rs::GRAVITY_EARTH` と同じ判断）。
#[cfg_attr(
    any(
        target_os = "windows",
        target_os = "ios",
        target_os = "macos",
        target_os = "android"
    ),
    allow(dead_code)
)]
struct UnsupportedGamepad;

impl Gamepad for UnsupportedGamepad {
    fn read(&self) -> Result<Option<fandhe_device::gamepad::GamepadSnapshot>, DeviceError> {
        Err(DeviceError::Unsupported("gamepad"))
    }
}

/// 実行対象プラットフォームに応じた `Gamepad` 実装を返す（`clipboard.rs::
/// shared_clipboard` と同型の cfg カスケード。状態を持たない実装のみのため
/// `OnceLock` は使わず、呼び出しごとに構築する）。
fn platform_gamepad() -> Box<dyn Gamepad> {
    #[cfg(target_os = "windows")]
    {
        Box::new(crate::device::windows_gamepad::WindowsGamepad)
    }
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        Box::new(crate::device::apple_gamepad::AppleGamepad)
    }
    #[cfg(target_os = "android")]
    {
        Box::new(crate::device::android_gamepad::AndroidGamepad)
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "ios",
        target_os = "macos",
        target_os = "android"
    )))]
    {
        Box::new(UnsupportedGamepad)
    }
}

/// 直近の `gamepad_show_effect` 呼び出し時刻（dt 実測用、`device/tilt.rs::LAST_TICK`
/// と同じ役割）。
static LAST_TICK: Mutex<Option<Instant>> = Mutex::new(None);

/// 前回呼び出しからの経過秒数を実測し、`[0.0, MAX_DT]` へクランプして返す
/// （`device/tilt.rs::measure_dt` と同型）。
fn measure_dt() -> f32 {
    let mut last = LAST_TICK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    let dt = match *last {
        Some(prev) => now.duration_since(prev).as_secs_f32(),
        None => DEFAULT_DT,
    };
    *last = Some(now);
    dt.clamp(0.0, MAX_DT)
}

/// `gamepad` から読んだ結果と代替入力から、次フレームで使う
/// `(表示状態, 左スティック x, y, 押下中ボタン名)` を決める純関数。実コントローラが
/// 読めているとき（`Ok(Some(..))`）は代替入力を一切見ない（上書きしない契約、
/// `gamepad_show` モジュール doc 参照）。
fn resolve_frame(
    state: &GamepadShowState,
    gamepad: &dyn Gamepad,
) -> (GamepadView, f32, f32, String) {
    match gamepad.read() {
        Ok(Some(snapshot)) => {
            let pressed = pressed_labels(snapshot.buttons).join(LABEL_SEPARATOR);
            (
                GamepadView::Connected,
                snapshot.left_stick.0,
                snapshot.left_stick.1,
                pressed,
            )
        }
        Ok(None) => alt_frame(state, GamepadView::DisconnectedAlt),
        Err(DeviceError::Unsupported(_)) => alt_frame(state, GamepadView::UnsupportedAlt),
        // タイムアウト・その他の一時的な読み取り失敗も代替入力へ倒す（未接続と同じ扱い。
        // `gamepad_show::GamepadView` は 3 状態のみのため、専用の失敗表示は本 Issue の
        // スコープ外とする）。
        Err(_) => alt_frame(state, GamepadView::DisconnectedAlt),
    }
}

/// 代替入力（画面状態の `alt_x`/`alt_y`/`alt_button`）から表示情報を組み立てる。
fn alt_frame(state: &GamepadShowState, view: GamepadView) -> (GamepadView, f32, f32, String) {
    let pressed = if state.alt_button {
        "代替ボタン".to_string()
    } else {
        String::new()
    };
    (view, state.alt_x, state.alt_y, pressed)
}

/// 傾き 0 かつ静止中・表示状態も押下も変化が無ければ完了送信をスキップしてよいかを
/// 判定する純関数（`device/tilt.rs::can_skip_send` と同じ考え方。egui を無駄に
/// 再描画させないため）。
fn can_skip_send(
    state: &GamepadShowState,
    view: GamepadView,
    x: f32,
    y: f32,
    pressed: &str,
) -> bool {
    view == state.view
        && pressed == pressed_text(state)
        && x.abs() <= STICK_EPSILON
        && y.abs() <= STICK_EPSILON
        && is_at_rest(state)
}

/// `gamepad` から次フレームの入力を決め、変化があれば [`apply_connected`]/
/// [`apply_disconnected`]/[`apply_unsupported`] のいずれかを適用する完了を送る
/// （テストから `cfg` 抜きで検証できるよう `gamepad_show_effect` 本体から切り出す、
/// `device/tilt.rs::drive` と同型）。
fn drive(
    state: &GamepadShowState,
    sender: &CompletionSender<GamepadShowState>,
    gamepad: &dyn Gamepad,
    dt: f32,
) {
    let (view, x, y, pressed) = resolve_frame(state, gamepad);

    if can_skip_send(state, view, x, y, &pressed) {
        // Windows/iOS/macOS のみ: プル方式のためイベント購読が無く、定期的に起こして
        // 読み直さないと以後のスティック操作・接続検知を取りこぼす
        // （IDLE_POLL_INTERVAL のコメント参照）。
        #[cfg(any(target_os = "windows", target_os = "ios", target_os = "macos"))]
        sender.request_repaint_after(IDLE_POLL_INTERVAL);
        return;
    }

    let phys_x = if x.abs() <= STICK_EPSILON { 0.0 } else { x };
    let phys_y = if y.abs() <= STICK_EPSILON { 0.0 } else { y };

    // 受信側（ProxyApp）がドロップ済み（アプリ終了中）の送信失敗は無視してよい
    // （coding-rust.md: panic を API 契約にしない、`device/tilt.rs` と同じ判断）。
    let _ = sender.send(Box::new(move |s: &mut GamepadShowState| match view {
        GamepadView::Connected => apply_connected(s, phys_x, phys_y, pressed, dt),
        GamepadView::DisconnectedAlt => apply_disconnected(s, dt),
        GamepadView::UnsupportedAlt => apply_unsupported(s, dt),
    }));
}

/// `registry.rs` の `effect` 区画から `gamepad_show` 画面用に配線される effect ドライバ。
pub fn gamepad_show_effect(state: &GamepadShowState, sender: &CompletionSender<GamepadShowState>) {
    let dt = measure_dt();
    drive(state, sender, platform_gamepad().as_ref(), dt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::gamepad::{DummyGamepad, GamepadSnapshot, BUTTON_A};

    fn sender_with_channel() -> (
        CompletionSender<GamepadShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<GamepadShowState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    #[test]
    fn resolve_frame_uses_real_stick_and_ignores_alt_when_connected() {
        let gamepad = DummyGamepad::new();
        gamepad.set(GamepadSnapshot {
            left_stick: (0.5, -0.5),
            right_stick: (0.0, 0.0),
            left_trigger: 0.0,
            right_trigger: 0.0,
            buttons: BUTTON_A,
        });
        let mut state = GamepadShowState::default();
        state.alt_x = 0.9;
        state.alt_y = 0.9;

        let (view, x, y, pressed) = resolve_frame(&state, &gamepad);
        assert_eq!(view, GamepadView::Connected);
        assert_eq!((x, y), (0.5, -0.5));
        assert_eq!(pressed, "A");
    }

    #[test]
    fn resolve_frame_falls_back_to_alt_when_disconnected() {
        let gamepad = DummyGamepad::new();
        let mut state = GamepadShowState::default();
        state.alt_x = 0.3;
        state.alt_y = -0.3;
        state.alt_button = true;

        let (view, x, y, pressed) = resolve_frame(&state, &gamepad);
        assert_eq!(view, GamepadView::DisconnectedAlt);
        assert_eq!((x, y), (0.3, -0.3));
        assert_eq!(pressed, "代替ボタン");
    }

    #[test]
    fn resolve_frame_falls_back_to_alt_when_unsupported() {
        struct Unsupported;
        impl Gamepad for Unsupported {
            fn read(&self) -> Result<Option<GamepadSnapshot>, DeviceError> {
                Err(DeviceError::Unsupported("gamepad"))
            }
        }
        let state = GamepadShowState::default();
        let (view, _, _, _) = resolve_frame(&state, &Unsupported);
        assert_eq!(view, GamepadView::UnsupportedAlt);
    }

    #[test]
    fn drive_applies_connected_frame_when_stick_moved() {
        let gamepad = DummyGamepad::new();
        gamepad.set(GamepadSnapshot {
            left_stick: (1.0, 0.0),
            right_stick: (0.0, 0.0),
            left_trigger: 0.0,
            right_trigger: 0.0,
            buttons: 0,
        });
        let (sender, rx) = sender_with_channel();
        let state = GamepadShowState::default();

        drive(&state, &sender, &gamepad, 1.0 / 60.0);

        let completion = rx.recv().expect("完了が送られる");
        let mut applied = state;
        completion(&mut applied);
        assert_eq!(applied.view, GamepadView::Connected);
    }

    #[test]
    fn drive_skips_send_when_disconnected_alt_input_at_rest() {
        let gamepad = DummyGamepad::new();
        let (sender, rx) = sender_with_channel();
        let state = GamepadShowState::default();

        drive(&state, &sender, &gamepad, 1.0 / 60.0);

        assert!(
            rx.try_recv().is_err(),
            "初期状態は既に DisconnectedAlt かつ静止中"
        );
    }

    #[test]
    fn drive_sends_when_view_transitions_to_connected() {
        let gamepad = DummyGamepad::new();
        gamepad.set(GamepadSnapshot {
            left_stick: (0.0, 0.0),
            right_stick: (0.0, 0.0),
            left_trigger: 0.0,
            right_trigger: 0.0,
            buttons: 0,
        });
        let (sender, rx) = sender_with_channel();
        let state = GamepadShowState::default();

        // スティック入力は 0 だが、接続状態への遷移自体が変化のため送信される。
        drive(&state, &sender, &gamepad, 1.0 / 60.0);

        assert!(rx.try_recv().is_ok());
    }

    /// Issue #934: 接続中から切断された場合、途中切断（`DisconnectedAlt`）へ
    /// 速やかに遷移する（古い値の張り付き対策、モジュール doc「切断後」参照）。
    #[test]
    fn drive_sends_disconnected_when_connected_gamepad_is_removed() {
        let gamepad = DummyGamepad::new();
        gamepad.set(GamepadSnapshot {
            left_stick: (0.5, 0.0),
            right_stick: (0.0, 0.0),
            left_trigger: 0.0,
            right_trigger: 0.0,
            buttons: 0,
        });
        let (sender, rx) = sender_with_channel();
        let state = GamepadShowState::default();

        drive(&state, &sender, &gamepad, 1.0 / 60.0);
        let completion = rx.recv().expect("接続時の完了が送られる");
        let mut applied = state;
        completion(&mut applied);
        assert_eq!(applied.view, GamepadView::Connected);

        gamepad.disconnect();
        drive(&applied, &sender, &gamepad, 1.0 / 60.0);
        let completion = rx.recv().expect("切断時の完了が送られる");
        completion(&mut applied);
        assert_eq!(applied.view, GamepadView::DisconnectedAlt);
    }

    /// Issue #934: `Err(Internal(..))`（読み取り失敗）は `Connected` へ倒さず
    /// 代替入力表示（`DisconnectedAlt`）へ倒す（`resolve_frame` の `Err(_)` 分岐）。
    #[test]
    fn resolve_frame_treats_internal_error_as_disconnected_alt_not_connected() {
        struct FailingGamepad;
        impl Gamepad for FailingGamepad {
            fn read(&self) -> Result<Option<GamepadSnapshot>, DeviceError> {
                Err(DeviceError::Internal("gamepad read"))
            }
        }
        let state = GamepadShowState::default();
        let (view, _, _, _) = resolve_frame(&state, &FailingGamepad);
        assert_ne!(view, GamepadView::Connected);
        assert_eq!(view, GamepadView::DisconnectedAlt);
    }
}
