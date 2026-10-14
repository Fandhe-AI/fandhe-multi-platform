//! 型サンプル「ゲームコントローラ」の Web 経路（Issue #936、親 #932）。
//!
//! `fandhe_core::screens::gamepad_show`（端末依存ゼロ）の状態を
//! [`super::mount::mount_with_effect`] の effect として観測し、Gamepad API
//! （`navigator.getGamepads()`、pull 型）を `requestAnimationFrame` の自己再
//! スケジュールでポーリングして結果を書き戻す。`fandhe-device` の `Gamepad`
//! トレイトは Windows 実装（Issue #933）のみが使う経路で、Web は
//! `location.rs`/`share.rs` と同じ理由（`fandhe-egui-proxy` の feature `device`
//! が wasm32 に依存を持ち込まない設計）でこれを通らず、本モジュールが
//! DOM レンダラ側の対応実装にあたる。ただし値域の正規化（`normalize_axis`/
//! `normalize_trigger`）とボタンラベル整形（`pressed_labels`）は
//! `fandhe_device::gamepad` を共有する（正規化ロジックの SSOT を 1 箇所に保つ）。
//!
//! # ポーリング方式（独自タイマーを作らない理由）
//!
//! Gamepad API は継続 push 型のイベントを持たず、毎フレーム
//! `navigator.getGamepads()` を読み直す pull 型が標準的な使い方
//! （MDN Gamepad API ガイド）。本モジュールもブラウザの `requestAnimationFrame`
//! （フレーム更新の標準経路）を自己再スケジュールしてポーリングし、
//! `setInterval` 等の独自タイマー機構は作らない。
//!
//! `gamepaddisconnected` イベントは購読しない: 毎フレーム pull しているため、
//! 切断は次フレームの `getGamepads()` が返す null / `connected === false` で
//! 検知できる（`fandhe_device::gamepad` モジュール doc の契約と同じ「古い値を
//! 張り付かせない」の web 版の実現方法）。`pagehide`/`pageshow` の購読も不要:
//! `requestAnimationFrame` はページが非表示・bfcache 中は発火せず、表示復帰で
//! 自動的に再開する（`watchPosition` のように明示的に止める対象が無い）。
//!
//! # 軸の向き（符号反転しない）
//!
//! `fandhe_device::gamepad` モジュール doc の契約（画面座標系・y 下が正）に対し、
//! Web Gamepad API の `axes` は既に y 下が正で返る（Windows/`XInput` の y 上が正
//! とは逆）。そのため他 OS 実装と異なり**符号反転しない**（`snapshot_from_pads`
//! のテストで固定する）。
//!
//! # 非標準配置の扱い（Issue #859 の 3 状態設計と対で「未接続」に化けさせない）
//!
//! `mapping !== "standard"` の接続は `DeviceError::Unsupported("gamepad
//! mapping")` として返す。`gamepad_show` 画面の `GamepadView` は 3 状態のみで
//! 4 状態目の追加は他 OS 実装（並列実装中の Android 等）の網羅 `match` に波及する
//! ため行わず、`UnsupportedAlt`（「非対応（代替入力）」）で表示する。「未接続」
//! （`DisconnectedAlt`）とは別のペイロード文字列で区別する（両方とも
//! `frame_from_read` では `Frame::Unsupported` へ写るが、`DeviceError::Unsupported`
//! のペイロードで「永続的な非対応」（API 不在・非 secure context）と区別できる
//! ようにしておく。[`mod@web`] の `run_tick` 参照）。
//!
//! # 純関数を `wasm32` gate の外に置く理由
//!
//! [`RawPad`]/[`snapshot_from_pads`]/[`Frame`]/[`frame_from_read`]/[`Shown`]/
//! [`can_skip`] は、JS 値から剥がした生値 → 型安全なスナップショット・次フレーム
//! の表示内容という、ブラウザ API を呼ばない変換ロジックのみを持つ。
//! `location.rs`/`fandhe-egui-proxy::device::gamepad` と同じ観点で native の
//! `cargo test` から固定できるようにするため、ブラウザ API を直接使う部分
//! （[`mod@web`]）だけを `wasm32` へ gate する。

use fandhe_core::screens::gamepad_show::GamepadView;
use fandhe_device::gamepad::{
    normalize_axis, normalize_trigger, pressed_labels, GamepadSnapshot, BUTTON_A, BUTTON_B,
    BUTTON_DPAD_DOWN, BUTTON_DPAD_LEFT, BUTTON_DPAD_RIGHT, BUTTON_DPAD_UP, BUTTON_LEFT_SHOULDER,
    BUTTON_LEFT_STICK, BUTTON_MENU, BUTTON_RIGHT_SHOULDER, BUTTON_RIGHT_STICK, BUTTON_VIEW,
    BUTTON_X, BUTTON_Y,
};
use fandhe_device::DeviceError;

/// 「傾き無し」とみなす閾値（`fandhe-egui-proxy::device::gamepad::STICK_EPSILON` と
/// 同じ値。デッドゾーン内は 0 として畳み、`can_skip` の静止判定と揃える）。
const STICK_EPSILON: f32 = 0.02;

/// 永続的な非対応（API 不在・非 secure context。差し替えで解消しない）を表す
/// [`DeviceError::Unsupported`] のペイロード。`fandhe_device::gamepad` の
/// 使用例（モジュール doc の 3 分岐）と同じ `"gamepad"` を使う。
/// native ビルドでは `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る
/// （`share.rs::UNAVAILABLE` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const UNSUPPORTED_PLATFORM: &str = "gamepad";
/// 非標準配置（`mapping !== "standard"`。コントローラの差し替えで解消しうる）を表す
/// [`DeviceError::Unsupported`] のペイロード。[`UNSUPPORTED_PLATFORM`] と区別する。
pub(crate) const UNSUPPORTED_MAPPING: &str = "gamepad mapping";

/// `getGamepads()` が返す 1 要素ぶんの生値。JS 側の `Gamepad` オブジェクトから
/// 必要なフィールドだけを剥がした型（`id`〈デバイス名〉は読まない。
/// security.md A09、フィンガープリント素材をログ・保存・送信しない方針と同じ
/// 理由で、そもそも取り込まない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) struct RawPad {
    pub connected: bool,
    /// `mapping === "standard"` か。
    pub standard: bool,
    pub axes: Vec<f64>,
    /// `(pressed, value)` の組。
    pub buttons: Vec<(bool, f64)>,
}

/// `getGamepads()` の配列（null 混在）から、先頭の「`Some` かつ `connected`」の
/// 1 台だけを見て [`fandhe_device::gamepad`] モジュール doc の 3 分岐へ写す。
///
/// - 該当する台が無い: `Ok(None)`（全 0 スナップショットを「未接続」の代用にしない）
/// - `!standard`: `Err(DeviceError::Unsupported(UNSUPPORTED_MAPPING))`
/// - `standard` だが `axes`/`buttons` の要素数が足りない: `Err(DeviceError::Internal(
///   "gamepad read"))`（欠けた要素を 0 で埋めない。添字アクセスは長さ検査の後のみ）
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn snapshot_from_pads(
    pads: &[Option<RawPad>],
) -> Result<Option<GamepadSnapshot>, DeviceError> {
    let Some(pad) = pads.iter().flatten().find(|pad| pad.connected) else {
        return Ok(None);
    };
    if !pad.standard {
        return Err(DeviceError::Unsupported(UNSUPPORTED_MAPPING));
    }
    if pad.axes.len() < 4 || pad.buttons.len() < 16 {
        return Err(DeviceError::Internal("gamepad read"));
    }

    let axis = |i: usize| normalize_axis(pad.axes[i] as f32);
    let left_stick = (axis(0), axis(1));
    let right_stick = (axis(2), axis(3));
    let left_trigger = normalize_trigger(pad.buttons[6].1 as f32);
    let right_trigger = normalize_trigger(pad.buttons[7].1 as f32);

    // 標準配置ボタン index → 本クレート共通の `BUTTON_*` ビット対応
    // （W3C Gamepad API の standard mapping。index 16 以降〈ホーム等〉は無視する。
    // モジュール関数として切り出さず、この関数専用のローカル `match` に留める
    // （呼び出し元が本関数のみのため、dead_code 検査対象を増やさない、ponytail）。
    let mut buttons = 0u16;
    for (index, (pressed, _)) in pad.buttons.iter().enumerate() {
        if !pressed {
            continue;
        }
        let bit = match index {
            0 => BUTTON_A,
            1 => BUTTON_B,
            2 => BUTTON_X,
            3 => BUTTON_Y,
            4 => BUTTON_LEFT_SHOULDER,
            5 => BUTTON_RIGHT_SHOULDER,
            8 => BUTTON_VIEW,
            9 => BUTTON_MENU,
            10 => BUTTON_LEFT_STICK,
            11 => BUTTON_RIGHT_STICK,
            12 => BUTTON_DPAD_UP,
            13 => BUTTON_DPAD_DOWN,
            14 => BUTTON_DPAD_LEFT,
            15 => BUTTON_DPAD_RIGHT,
            _ => continue,
        };
        buttons |= bit;
    }

    Ok(Some(GamepadSnapshot {
        left_stick,
        right_stick,
        left_trigger,
        right_trigger,
        buttons,
    }))
}

/// 次フレームで画面へ適用する内容（[`fandhe_device::gamepad::Gamepad::read`] の
/// 結果から決まる、`fandhe-egui-proxy::device::gamepad::resolve_frame` の Web 版）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Frame {
    /// 実コントローラを読めている。`x`/`y` はデッドゾーン畳み込み済みの左スティック。
    Connected { x: f32, y: f32, pressed: String },
    /// 未接続（代替入力が効く）。
    Disconnected,
    /// 非対応（代替入力が効く。永続的な非対応・非標準配置のいずれも含む）。
    Unsupported,
}

/// [`fandhe_device::gamepad::Gamepad::read`] の結果を [`Frame`] へ写す
/// （`fandhe-egui-proxy::device::gamepad::resolve_frame` と同じ判定基準）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn frame_from_read(read: Result<Option<GamepadSnapshot>, DeviceError>) -> Frame {
    match read {
        Ok(Some(snapshot)) => {
            let pressed = pressed_labels(snapshot.buttons).join("、");
            let deadzone = |v: f32| if v.abs() <= STICK_EPSILON { 0.0 } else { v };
            Frame::Connected {
                x: deadzone(snapshot.left_stick.0),
                y: deadzone(snapshot.left_stick.1),
                pressed,
            }
        }
        Ok(None) => Frame::Disconnected,
        Err(DeviceError::Unsupported(_)) => Frame::Unsupported,
        // タイムアウト・その他の一時的な読み取り失敗も未接続と同じ扱いへ倒す
        // （`gamepad_show::GamepadView` は 3 状態のみのため、専用の失敗表示は
        // 本 Issue のスコープ外、`fandhe-egui-proxy::device::gamepad::resolve_frame`
        // と同じ判断）。
        Err(_) => Frame::Disconnected,
    }
}

/// 直近の render で画面が表示している内容（`web::run_tick` が
/// `applier.apply` 前後で更新する。効果は「変化が無ければ再描画をスキップする」
/// 判定のためだけに使う）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shown {
    pub view: GamepadView,
    pub pressed: String,
    pub at_rest: bool,
}

/// `frame` を適用せずスキップしてよいかを判定する（`fandhe-egui-proxy::device::
/// gamepad::can_skip_send` と同じ考え方。無駄な再描画を避ける）。
///
/// Web DOM 出力では代替入力ウィジェット（スライダー・チェックボックス）が
/// `Node::Widget` の空 `div` プレースホルダとして描画され実際には操作できない
/// （`dom_tree.rs` 参照、既存の天井）ため、`alt_x`/`alt_y`/`alt_button` は
/// 常に既定値のままで変化しない。`DisconnectedAlt`/`UnsupportedAlt` の判定は
/// view と静止状態の一致だけを見れば足り、`fandhe-egui-proxy` 版のように
/// alt 側を比較する必要はない。
// ponytail: `Node::Widget` の実 DOM マッピングを導入して代替入力が実際に操作
// できるようになったら、alt 側の変化も比較対象へ足す必要がある
// （upgrade path、`dom_tree.rs`/`mount.rs` 側の実装が前提）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn can_skip(shown: &Shown, frame: &Frame) -> bool {
    match frame {
        Frame::Connected { x, y, pressed } => {
            shown.view == GamepadView::Connected
                && shown.at_rest
                && &shown.pressed == pressed
                && *x == 0.0
                && *y == 0.0
        }
        Frame::Disconnected => shown.view == GamepadView::DisconnectedAlt && shown.at_rest,
        Frame::Unsupported => shown.view == GamepadView::UnsupportedAlt && shown.at_rest,
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::gamepad_show::{
        apply_connected, apply_disconnected, apply_unsupported, is_at_rest, pressed_text,
        GamepadShowState,
    };
    use fandhe_device::gamepad::{Gamepad, GamepadSnapshot};
    use fandhe_device::DeviceError;
    use js_sys::Reflect;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    use super::UNSUPPORTED_PLATFORM;
    use super::{can_skip, frame_from_read, snapshot_from_pads, Frame, RawPad, Shown};
    use crate::mount::Applier;

    /// 1 フレームで進める時間の上限（`fandhe-egui-proxy::device::gamepad::MAX_DT` と
    /// 同じ値）。
    const MAX_DT: f32 = 0.1;
    /// マウント後、最初の `requestAnimationFrame` コールバックで使う既定 dt
    /// （60fps 相当。直前のタイムスタンプが無いため実測できない）。
    const DEFAULT_DT: f32 = 1.0 / 60.0;

    /// rAF 自己再スケジュール用クロージャの保持型（clippy::type_complexity 回避の
    /// エイリアス）。`schedule_next` / `start_loop` の両方で使う。
    type FrameClosureHolder = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;

    thread_local! {
        /// マウント ID ごとの「直近表示内容」（[`Shown`] の doc 参照）。
        /// `ACTIVE`（`location.rs`）と同じくマウント単位で持つ。エントリの有無は
        /// 「このマウントの rAF ループが既に起動済みか」の判定にも使う
        /// （`gamepad_show_effect` 参照）。
        static SHOWN: RefCell<HashMap<u64, Shown>> = RefCell::new(HashMap::new());

        /// マウント ID ごとの直近フレームタイムスタンプ（`DOMHighResTimeStamp`、ms）。
        /// dt の実測に使う。
        static LAST_TS_MS: RefCell<HashMap<u64, f64>> = RefCell::new(HashMap::new());
    }

    /// `target[name]` が呼び出し可能な関数として実在するかを確認する
    /// （`share.rs::has_function` と同じ理由。`Navigator::get_gamepads` は
    /// web-sys の型上は常に呼び出せるように見えるが、非対応ブラウザでは実際には
    /// `undefined` になる。型に現れない JS 側の実態を境界で検証する、
    /// security.md A03）。
    fn has_function(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|value| value.is_function())
            .unwrap_or(false)
    }

    /// `navigator.getGamepads()` を読み、`Vec<Option<RawPad>>` へ剥がす。
    /// 非 secure context・API 不在・呼び出し自体の失敗は
    /// `Err(Unsupported(UNSUPPORTED_PLATFORM))`（永続的な非対応）にする。
    fn read_raw_pads() -> Result<Vec<Option<RawPad>>, DeviceError> {
        let window = web_sys::window().ok_or(DeviceError::Unsupported(UNSUPPORTED_PLATFORM))?;
        if !window.is_secure_context() {
            return Err(DeviceError::Unsupported(UNSUPPORTED_PLATFORM));
        }
        let navigator = window.navigator();
        let navigator_value: JsValue = navigator.clone().into();
        if !has_function(&navigator_value, "getGamepads") {
            return Err(DeviceError::Unsupported(UNSUPPORTED_PLATFORM));
        }
        let array = navigator
            .get_gamepads()
            .map_err(|_| DeviceError::Unsupported(UNSUPPORTED_PLATFORM))?;

        let mut pads = Vec::with_capacity(array.length() as usize);
        for i in 0..array.length() {
            let item = array.get(i);
            if item.is_null() || item.is_undefined() {
                pads.push(None);
                continue;
            }
            let Ok(pad) = item.dyn_into::<web_sys::Gamepad>() else {
                pads.push(None);
                continue;
            };
            let axes: Vec<f64> = pad
                .axes()
                .iter()
                .map(|value| value.as_f64().unwrap_or(f64::NAN))
                .collect();
            let buttons: Vec<(bool, f64)> = pad
                .buttons()
                .iter()
                .map(|value| match value.dyn_into::<web_sys::GamepadButton>() {
                    Ok(button) => (button.pressed(), button.value()),
                    // 要素が `GamepadButton` へキャストできない想定外の値は
                    // 「未押下・値 0」として扱う（偽の押下を作らない、
                    // security.md A03 と同じ fail-safe 方針）。
                    Err(_) => (false, 0.0),
                })
                .collect();
            pads.push(Some(RawPad {
                connected: pad.connected(),
                standard: pad.mapping() == web_sys::GamepadMappingType::Standard,
                axes,
                buttons,
            }));
        }
        Ok(pads)
    }

    /// Web（Gamepad API）実装。状態を持たない unit struct（`read()` のたびに
    /// `navigator.getGamepads()` を pull する。`WindowsGamepad` と同型）。
    pub struct WebGamepad;

    impl Gamepad for WebGamepad {
        fn read(&self) -> Result<Option<GamepadSnapshot>, DeviceError> {
            let pads = read_raw_pads()?;
            snapshot_from_pads(&pads)
        }
    }

    /// `mount_id` の直前フレームからの経過秒数を実測し、`[0.0, MAX_DT]` へ
    /// クランプして返す（`fandhe-egui-proxy::device::gamepad::measure_dt` と
    /// 同じ役割。引数は `requestAnimationFrame` コールバックが渡す
    /// `DOMHighResTimeStamp`〈ms〉）。
    fn measure_dt(mount_id: u64, ts_ms: f64) -> f32 {
        LAST_TS_MS.with(|cell| {
            let mut map = cell.borrow_mut();
            let dt = match map.get(&mount_id) {
                Some(prev) => (((ts_ms - prev) / 1000.0) as f32).clamp(0.0, MAX_DT),
                None => DEFAULT_DT,
            };
            map.insert(mount_id, ts_ms);
            dt
        })
    }

    /// `window.requestAnimationFrame(closure)` を呼ぶ。失敗はコンソールへ出すのみ
    /// （panic を API 契約にしない、coding-rust.md 準拠。次フレームが来なくなる
    /// だけで、既に適用済みの表示が壊れるわけではない）。
    fn schedule_next(closure_holder: &FrameClosureHolder) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let holder = closure_holder.borrow();
        let Some(closure) = holder.as_ref() else {
            return;
        };
        if let Err(err) = window.request_animation_frame(closure.as_ref().unchecked_ref()) {
            web_sys::console::error_1(&err);
        }
    }

    /// 1 フレームぶんの処理: 読み取り → 適用要否判定 → 必要なら適用。戻り値は
    /// 「次フレームも再スケジュールするか」（`false` は
    /// `Err(Unsupported(UNSUPPORTED_PLATFORM))`〈永続的な非対応〉のときのみ。
    /// それ以外〈未接続・非標準配置・一時失敗・接続中・スキップ時〉は常に
    /// 再スケジュールし、接続・切断・差し替えを次フレームで検知し続ける）。
    fn run_tick(mount_id: u64, applier: &Applier<GamepadShowState>, ts_ms: f64) -> bool {
        let dt = measure_dt(mount_id, ts_ms);
        let read = WebGamepad.read();
        let persistent_unsupported = matches!(&read, Err(DeviceError::Unsupported(payload)) if *payload == UNSUPPORTED_PLATFORM);
        let frame = frame_from_read(read);

        let shown = SHOWN.with(|cell| cell.borrow().get(&mount_id).cloned());
        if let Some(shown) = shown {
            if !can_skip(&shown, &frame) {
                applier.apply(move |state| match frame {
                    Frame::Connected { x, y, pressed } => apply_connected(state, x, y, pressed, dt),
                    Frame::Disconnected => apply_disconnected(state, dt),
                    Frame::Unsupported => apply_unsupported(state, dt),
                });
            }
        }

        !persistent_unsupported
    }

    /// `mount_id` の rAF ループを起動する。`holder` は自己参照クロージャの標準的な
    /// 組み方（`Rc<RefCell<Option<Closure>>>` を複製して自分自身から次回分を
    /// 再登録する）。マウントのアンマウント API が無いため、このクロージャは
    /// ページ寿命ぶん生存し続ける（`location.rs::Watch` の `forget` と同じ天井）。
    // ponytail: `Closure` 1 個をページ寿命ぶん保持し続ける意図的なリーク。
    // upgrade path: `mount.rs` にアンマウント API が入ったら停止経路を足す。
    fn start_loop(mount_id: u64, applier: Applier<GamepadShowState>) {
        let holder: FrameClosureHolder = Rc::new(RefCell::new(None));
        let holder_for_closure = holder.clone();
        *holder.borrow_mut() = Some(Closure::new(move |ts_ms: f64| {
            if run_tick(mount_id, &applier, ts_ms) {
                schedule_next(&holder_for_closure);
            }
        }));
        schedule_next(&holder);
    }

    /// `mount_with_effect` へ渡す effect。`gamepad_show` 画面のポーリングを
    /// 継続駆動する。
    ///
    /// 要求が無ければ何もしない契約（`mount.rs` の「effect フック」節）を、
    /// 「[`SHOWN`] を常に最新の表示内容へ更新しつつ、このマウントの rAF ループが
    /// 既に起動済みなら以降は起動しない」という形で満たす（rAF ループ自体は
    /// `window.requestAnimationFrame` に駆動されるため、この effect は
    /// 初回マウント時の起動と、以後の再描画のたびの [`SHOWN`] 更新のためだけに
    /// 呼ばれる）。
    pub fn gamepad_show_effect(state: &GamepadShowState, applier: &Applier<GamepadShowState>) {
        let mount_id = applier.mount_id();
        let already_running = SHOWN.with(|cell| {
            let mut map = cell.borrow_mut();
            let existed = map.contains_key(&mount_id);
            map.insert(
                mount_id,
                Shown {
                    view: state.view,
                    pressed: pressed_text(state).to_string(),
                    at_rest: is_at_rest(state),
                },
            );
            existed
        });
        if already_running {
            return;
        }
        start_loop(mount_id, applier.clone());
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::gamepad_show_effect;

#[cfg(test)]
mod tests {
    use super::*;

    fn standard_pad(axes: [f64; 4], buttons: &[(bool, f64)]) -> RawPad {
        RawPad {
            connected: true,
            standard: true,
            axes: axes.to_vec(),
            buttons: buttons.to_vec(),
        }
    }

    fn buttons_all_released() -> Vec<(bool, f64)> {
        vec![(false, 0.0); 16]
    }

    #[test]
    fn snapshot_from_pads_empty_or_all_null_is_ok_none() {
        assert_eq!(snapshot_from_pads(&[]).unwrap(), None);
        assert_eq!(snapshot_from_pads(&[None, None]).unwrap(), None);
    }

    #[test]
    fn snapshot_from_pads_disconnected_only_is_ok_none() {
        let mut pad = standard_pad([0.0, 0.0, 0.0, 0.0], &buttons_all_released());
        pad.connected = false;
        assert_eq!(snapshot_from_pads(&[Some(pad)]).unwrap(), None);
    }

    #[test]
    fn snapshot_from_pads_non_standard_mapping_is_unsupported() {
        let mut pad = standard_pad([0.0, 0.0, 0.0, 0.0], &buttons_all_released());
        pad.standard = false;
        let result = snapshot_from_pads(&[Some(pad)]);
        assert!(matches!(
            result,
            Err(DeviceError::Unsupported(UNSUPPORTED_MAPPING))
        ));
    }

    #[test]
    fn snapshot_from_pads_short_axes_or_buttons_is_internal_error() {
        let short_axes = RawPad {
            connected: true,
            standard: true,
            axes: vec![0.0, 0.0, 0.0],
            buttons: buttons_all_released(),
        };
        assert!(matches!(
            snapshot_from_pads(&[Some(short_axes)]),
            Err(DeviceError::Internal("gamepad read"))
        ));

        let short_buttons = RawPad {
            connected: true,
            standard: true,
            axes: vec![0.0, 0.0, 0.0, 0.0],
            buttons: vec![(false, 0.0); 15],
        };
        assert!(matches!(
            snapshot_from_pads(&[Some(short_buttons)]),
            Err(DeviceError::Internal("gamepad read"))
        ));
    }

    #[test]
    fn snapshot_from_pads_does_not_flip_axis_sign() {
        // Web Gamepad API は既に y 下が正で返るため、Windows と異なり反転しない
        // （モジュール doc「軸の向き」節参照）。
        let pad = standard_pad([0.0, 1.0, 0.0, -1.0], &buttons_all_released());
        let snapshot = snapshot_from_pads(&[Some(pad)]).unwrap().unwrap();
        assert_eq!(snapshot.left_stick.1, 1.0);
        assert_eq!(snapshot.right_stick.1, -1.0);
    }

    #[test]
    fn snapshot_from_pads_clamps_out_of_range_axis_and_replaces_nan_trigger() {
        let mut buttons = buttons_all_released();
        buttons[6] = (false, f64::NAN);
        buttons[7] = (false, 2.0);
        let pad = standard_pad([2.0, -2.0, 0.0, 0.0], &buttons);
        let snapshot = snapshot_from_pads(&[Some(pad)]).unwrap().unwrap();
        assert_eq!(snapshot.left_stick, (1.0, -1.0));
        assert_eq!(snapshot.left_trigger, 0.0);
        assert_eq!(snapshot.right_trigger, 1.0);
    }

    #[test]
    fn snapshot_from_pads_maps_button_indices_and_ignores_undefined() {
        let mut buttons = buttons_all_released();
        buttons[0] = (true, 1.0);
        buttons[9] = (true, 1.0);
        buttons[12] = (true, 1.0);
        let pad = standard_pad([0.0, 0.0, 0.0, 0.0], &buttons);
        let snapshot = snapshot_from_pads(&[Some(pad)]).unwrap().unwrap();
        assert_eq!(snapshot.buttons, BUTTON_A | BUTTON_MENU | BUTTON_DPAD_UP);
    }

    #[test]
    fn snapshot_from_pads_ignores_button_index_16_and_beyond() {
        let mut buttons = buttons_all_released();
        buttons.push((true, 1.0)); // index 16（ホーム等）
        let pad = standard_pad([0.0, 0.0, 0.0, 0.0], &buttons);
        let snapshot = snapshot_from_pads(&[Some(pad)]).unwrap().unwrap();
        assert_eq!(snapshot.buttons, 0);
    }

    #[test]
    fn snapshot_from_pads_skips_leading_null_and_uses_first_connected() {
        let pad = standard_pad([0.5, 0.0, 0.0, 0.0], &buttons_all_released());
        let snapshot = snapshot_from_pads(&[None, Some(pad)]).unwrap().unwrap();
        assert_eq!(snapshot.left_stick.0, 0.5);
    }

    #[test]
    fn frame_from_read_maps_unsupported_and_internal_and_none() {
        assert_eq!(
            frame_from_read(Err(DeviceError::Unsupported(UNSUPPORTED_PLATFORM))),
            Frame::Unsupported
        );
        assert_eq!(
            frame_from_read(Err(DeviceError::Unsupported(UNSUPPORTED_MAPPING))),
            Frame::Unsupported
        );
        assert_eq!(
            frame_from_read(Err(DeviceError::Internal("gamepad read"))),
            Frame::Disconnected
        );
        assert_eq!(frame_from_read(Ok(None)), Frame::Disconnected);
    }

    #[test]
    fn frame_from_read_folds_deadzone_to_zero() {
        let snapshot = GamepadSnapshot {
            left_stick: (0.01, -0.01),
            right_stick: (0.0, 0.0),
            left_trigger: 0.0,
            right_trigger: 0.0,
            buttons: 0,
        };
        match frame_from_read(Ok(Some(snapshot))) {
            Frame::Connected { x, y, .. } => {
                assert_eq!(x, 0.0);
                assert_eq!(y, 0.0);
            }
            other => panic!("Connected を期待したが {other:?} だった"),
        }
    }

    #[test]
    fn can_skip_true_for_initial_disconnected_alt_at_rest() {
        let shown = Shown {
            view: GamepadView::DisconnectedAlt,
            pressed: String::new(),
            at_rest: true,
        };
        assert!(can_skip(&shown, &Frame::Disconnected));
    }

    #[test]
    fn can_skip_false_when_view_transitions() {
        let shown = Shown {
            view: GamepadView::DisconnectedAlt,
            pressed: String::new(),
            at_rest: true,
        };
        assert!(!can_skip(&shown, &Frame::Unsupported));
    }

    #[test]
    fn can_skip_false_when_pressed_changes() {
        let shown = Shown {
            view: GamepadView::Connected,
            pressed: String::new(),
            at_rest: true,
        };
        let frame = Frame::Connected {
            x: 0.0,
            y: 0.0,
            pressed: "A".to_string(),
        };
        assert!(!can_skip(&shown, &frame));
    }

    #[test]
    fn can_skip_false_when_stick_non_zero() {
        let shown = Shown {
            view: GamepadView::Connected,
            pressed: String::new(),
            at_rest: true,
        };
        let frame = Frame::Connected {
            x: 0.5,
            y: 0.0,
            pressed: String::new(),
        };
        assert!(!can_skip(&shown, &frame));
    }
}
