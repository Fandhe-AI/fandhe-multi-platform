//! 型サンプル「傾きで玉が転がる」の Web 経路（Issue #989、親 #988）。
//!
//! `fandhe_core::screens::tilt_ball`（端末依存ゼロ）の状態を
//! [`super::mount::mount_with_effect`] の effect として観測し、
//! `window` の `deviceorientation`（継続 push 型、`Motion`/`location` と同じ束）から
//! 重力ベクトル相当の傾きを取り、`step` を毎フレーム進める。`fandhe-device` の
//! `Motion` トレイトは egui 経路（デスクトップのスライダー代替・iOS/Android の実センサ、
//! `fandhe-egui-proxy::device::tilt` 参照）専用で、Web は `location.rs`/`gamepad.rs` と
//! 同じ理由（`fandhe-egui-proxy` の feature `device` が wasm32 に依存を持ち込まない設計）で
//! これを通らず、本モジュールが DOM レンダラ側の対応実装にあたる。
//!
//! # 重力ベクトルへの変換（画面座標: x 右・y 下が正）
//!
//! `DeviceOrientationEvent.beta`/`gamma`（度）から、端末座標の「下向き」ベクトル
//! `(dx, dy) = (cos(beta)·sin(gamma), -sin(beta))`（右・上が正の基底）を求め、
//! `screen.orientation.angle`（0/90/180/270）で `fandhe-egui-proxy::device::
//! tilt::gravity_to_tilt` と同じ表に従って画面基準へ回転してから
//! `tilt = (sx, -sy)` へ写す（`-1.0..=1.0` へクランプ）。`beta`/`gamma` が
//! `null`（Chrome はセンサ無し環境で全 `null` のイベントを 1 回出す）・非有限値の
//! 場合は [`Reading::Unavailable`] にする（[`tilt_from_orientation`] 参照）。
//!
//! 横持ちでの向きの正しさは実機でしか確かめられない（out-of-scope-tracking.md
//! 対象、PR 本文に人間担当として明記する）。
//!
//! # 嘘の成功を出さない（device-bridge-contracts.md 6）
//!
//! 初期表示は [`fandhe_core::screens::tilt_ball::SensorStatus::Waiting`]。API 不在・
//! 非 secure context・null イベント・3 秒間無応答のいずれかで
//! [`SensorStatus::Unavailable`] にする（0 の傾きを成功値として出さない）。
//!
//! # ライフサイクル（device-bridge-contracts.md 1・9）
//!
//! 非表示（`document.hidden`）・`pagehide` ではリスナーを外して `Waiting` に戻し、
//! 最後の値を使い続けない。`pageshow`・可視化への復帰で再登録する。`deviceorientation`
//! リスナーと生成した `Closure` はマウント単位で [`web::ENTRIES`] が保持し続け、
//! stop/start では登録・解除のみを行う（`Closure` 自体は drop しない）ため、
//! `location.rs::stop` が踏んだ「drop 済み `Closure` への遅延コールバック」問題が
//! 構造的に起きない。リークは 1 マウントあたり `deviceorientation` 用 + ライフサイクル用の
//! 2 個で打ち止め（モジュール冒頭のリーク方針、`mount.rs` にアンマウント API が無い
//! 天井は `gamepad.rs`/`location.rs` と共通）。
//!
//! # 純関数を `wasm32` gate の外に置く理由
//!
//! [`tilt_from_orientation`]/[`Reading`]/[`Shown`]/[`can_skip`] はブラウザ API を
//! 呼ばない変換・判定ロジックのみを持つ。`location.rs`/`gamepad.rs` と同じ観点で
//! native の `cargo test` から固定できるようにするため、ブラウザ API を直接使う部分
//! （[`mod@web`]）だけを `wasm32` へ gate する。

use fandhe_core::screens::tilt_ball::{is_at_rest, SensorStatus, TiltBallState};

/// 「傾き無し」とみなす閾値（`fandhe-egui-proxy::device::tilt::TILT_EPSILON` と同じ値。
/// 実センサのノイズを無視し、静止判定を成立させる）。
const TILT_EPSILON: f32 = 0.02;

/// `DeviceOrientationEvent.beta`/`gamma`（度）と `screen.orientation.angle` から
/// 画面座標の傾き `(tilt_x, tilt_y)` を求める純関数（モジュール doc の変換式参照）。
/// `beta`/`gamma` のどちらかが `None`・非有限値なら `None`（センサ無し・null イベント）。
/// 出力は必ず `-1.0..=1.0` に収まる。未知の `screen_angle` は `0` と同じ扱いにする
/// （fail-safe、`gravity_to_tilt` の「その他」分岐と同じ判断）。
// native ビルドでは `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る
// （`gamepad.rs`/`location.rs` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn tilt_from_orientation(
    beta: Option<f64>,
    gamma: Option<f64>,
    screen_angle: i32,
) -> Option<(f32, f32)> {
    let beta = beta?;
    let gamma = gamma?;
    if !beta.is_finite() || !gamma.is_finite() {
        return None;
    }
    let b = beta.to_radians();
    let g = gamma.to_radians();
    // 端末座標の「下向き」ベクトル（右・上が正の基底）。
    let dx = (b.cos() * g.sin()) as f32;
    let dy = -(b.sin()) as f32;

    // 画面基準（右方向・上方向）へ回転する（`gravity_to_tilt` と同じ表）。
    let (sx, sy) = match screen_angle {
        90 => (-dy, dx),
        180 => (-dx, -dy),
        270 => (dy, -dx),
        _ => (dx, dy),
    };

    Some(((sx).clamp(-1.0, 1.0), (-sy).clamp(-1.0, 1.0)))
}

/// リスナーが最後に観測した内容（[`mod@web`] のハンドラが書き、rAF ループのみが
/// 画面へ反映する。書き手を 1 つに保つ設計、モジュール doc 参照）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Reading {
    /// 取得を試みている（初回応答待ち・非表示からの再開直後）。
    Waiting,
    /// 取得できない（API 不在・非 secure context・null イベント・3 秒間無応答）。
    Unavailable,
    /// 実センサ値を取得できている（デッドゾーン畳み込み前の生値）。
    Live(f32, f32),
}

/// 直近の render で画面が表示している内容（`gamepad.rs::Shown` と同じ役割。
/// 「変化が無ければ再描画をスキップする」判定のためだけに使う）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shown {
    pub sensor: SensorStatus,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub at_rest: bool,
}

impl Shown {
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn from_state(state: &TiltBallState) -> Self {
        Self {
            sensor: state.sensor,
            tilt_x: state.tilt_x,
            tilt_y: state.tilt_y,
            at_rest: is_at_rest(state),
        }
    }
}

/// `reading` を適用せずスキップしてよいかを判定する（`fandhe-egui-proxy::device::
/// tilt::can_skip_send` と同じ考え方。無駄な再描画を避ける）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn can_skip(shown: &Shown, reading: &Reading) -> bool {
    match reading {
        Reading::Live(x, y) => {
            shown.sensor == SensorStatus::Live
                && x.abs() <= TILT_EPSILON
                && y.abs() <= TILT_EPSILON
                && shown.at_rest
                && shown.tilt_x.abs() <= TILT_EPSILON
                && shown.tilt_y.abs() <= TILT_EPSILON
        }
        Reading::Waiting => shown.sensor == SensorStatus::Waiting,
        Reading::Unavailable => shown.sensor == SensorStatus::Unavailable,
    }
}

/// 最後に `deviceorientation` を受けてから `timeout_ms` 以上経ったかを判定する純関数。
/// 初回応答待ちを `Unavailable` にする判定に使う（1 回きりのタイマーだと、停止前の
/// 古いタイマーが再開後の試行を誤って失敗にする。PR #997 codex / Bugbot 指摘）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_stale(last_event_ms: f64, now_ms: f64, timeout_ms: f64) -> bool {
    now_ms - last_event_ms >= timeout_ms
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::tilt_ball::{step, SensorStatus, TiltBallState};
    use js_sys::Reflect;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    use super::{can_skip, is_stale, tilt_from_orientation, Reading, Shown, TILT_EPSILON};
    use crate::mount::Applier;

    /// 1 フレームで進める時間の上限（`fandhe-egui-proxy::device::tilt::MAX_DT` と同じ値）。
    const MAX_DT: f32 = 0.1;
    /// マウント後、最初の `requestAnimationFrame` コールバックで使う既定 dt
    /// （60fps 相当。直前のタイムスタンプが無いため実測できない）。
    const DEFAULT_DT: f32 = 1.0 / 60.0;
    /// 初回の `deviceorientation` が届かないまま `Unavailable` にするまでの猶予
    /// （Firefox デスクトップ・許可されていない iOS Safari のように 1 件も来ない環境向け）。
    /// 判定は rAF ループの [`run_tick`] が毎フレーム [`is_stale`] で行う（再開のたびに
    /// 登録時刻から数え直すため、停止前の古いタイマーが新しい試行を失敗にしない）。
    const UNAVAILABLE_TIMEOUT_MS: f64 = 3_000.0;

    /// rAF 自己再スケジュール用クロージャの保持型（`gamepad.rs::FrameClosureHolder` と同じ
    /// clippy::type_complexity 回避のエイリアス）。
    type FrameClosureHolder = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;
    /// `deviceorientation`/ライフサイクルイベント購読クロージャの型
    /// （`location.rs::PageLifecycleClosure` と同じエイリアス）。
    type EventClosure = Closure<dyn FnMut(web_sys::Event)>;

    /// マウント単位のエントリ。`on_orientation`/`on_lifecycle` はページが生きている間
    /// 保持し続け、stop/start では `add`/`removeEventListener` の登録・解除のみを行う
    /// （モジュール冒頭「ライフサイクル」節参照。`Closure` 自体は drop しない）。
    struct Entry {
        reading: Reading,
        last_ts_ms: Option<f64>,
        /// 最後に `deviceorientation` を受けた時刻（`Date.now()`、ミリ秒）。登録直後は
        /// 登録時刻を入れ、そこから [`UNAVAILABLE_TIMEOUT_MS`] の猶予を数える。
        last_event_ms: f64,
        /// API 不在・非 secure context のときは `None`（そもそもリスナーを作らない）。
        on_orientation: Option<EventClosure>,
        listening: bool,
    }

    thread_local! {
        /// マウント ID ごとの傾き取得状態（`gamepad.rs::SHOWN`/`location.rs::ACTIVE` と
        /// 同じくマウント単位で持つ）。エントリの有無は「このマウントが初期化済みか」の
        /// 判定にも使う（[`tilt_ball_effect`] 参照）。
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());

        /// マウント ID ごとの直近表示内容（[`Shown`] の doc 参照）。
        static SHOWN: RefCell<HashMap<u64, Shown>> = RefCell::new(HashMap::new());

        /// マウント ID ごとに保持する「可視化・pagehide/pageshow で呼び直す」共通
        /// ライフサイクルクロージャ。`visibilitychange`（document）・`pagehide`/
        /// `pageshow`（window）の 3 イベントへ同じクロージャを登録する（モジュール doc
        /// 「マウントあたり 2 個で打ち止め」節参照）。
        static LIFECYCLE: RefCell<HashMap<u64, EventClosure>> = RefCell::new(HashMap::new());
    }

    /// `target[name]` が呼び出し可能な関数として実在するかを確認する
    /// （`gamepad.rs::has_function`/`share.rs::has_function` と同じ理由。型に現れない
    /// JS 側の実態を境界で検証する、security.md A03）。
    fn has_function(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|value| value.is_function())
            .unwrap_or(false)
    }

    /// `window.DeviceOrientationEvent` が実在し、かつ secure context かを確認する。
    fn api_available(window: &web_sys::Window) -> bool {
        window.is_secure_context() && has_function(window, "DeviceOrientationEvent")
    }

    /// `screen.orientation.angle`（整数度）を読む。取得できなければ `0`
    /// （回転なしと同じ扱いにする fail-safe、[`tilt_from_orientation`] の
    /// 「未知の `screen_angle`」分岐と同じ判断）。
    fn read_screen_angle() -> i32 {
        web_sys::window()
            .and_then(|w| w.screen().ok())
            .and_then(|s| s.orientation().angle().ok())
            .map(|angle| angle as i32)
            .unwrap_or(0)
    }

    /// `mount_id` の `reading` を書き換える（存在しなければ何もしない。エントリ削除後の
    /// 遅延コールバックを握り潰す契約、`location.rs::update_last_applied` と同じ考え方）。
    fn set_reading(mount_id: u64, reading: Reading) {
        ENTRIES.with(|cell| {
            if let Some(entry) = cell.borrow_mut().get_mut(&mount_id) {
                entry.reading = reading;
            }
        });
    }

    /// `deviceorientation` イベントハンドラ。`reading` を書くだけで、画面への反映は
    /// rAF ループ（[`run_tick`]）のみが行う（モジュール doc「書き手を 1 つに保つ」参照）。
    fn handle_orientation(mount_id: u64, event: web_sys::Event) {
        let event: web_sys::DeviceOrientationEvent = event.unchecked_into();
        let angle = read_screen_angle();
        let reading = match tilt_from_orientation(event.beta(), event.gamma(), angle) {
            Some((x, y)) => Reading::Live(x, y),
            None => Reading::Unavailable,
        };
        ENTRIES.with(|cell| {
            if let Some(entry) = cell.borrow_mut().get_mut(&mount_id) {
                entry.reading = reading;
                entry.last_event_ms = js_sys::Date::now();
            }
        });
    }

    /// `mount_id` のセンサ取得を開始する。API 不在・非 secure context なら
    /// リスナーを作らず `Unavailable` にする（契約表 6「嘘の成功」を出さない）。
    /// 既にリスナー（`Closure`）を作成済み（2 回目以降の `start`）なら、同じ
    /// `Closure` を再登録するだけで新規作成しない（モジュール doc「drop しない」参照）。
    /// 既に登録中なら何もしない（`pageshow` 等で呼び直されても `Live` を `Waiting` へ
    /// 戻さない。PR #997 Bugbot 指摘）。
    fn start(mount_id: u64) {
        let listening = ENTRIES.with(|cell| {
            cell.borrow()
                .get(&mount_id)
                .map(|e| e.listening)
                .unwrap_or(false)
        });
        if listening {
            return;
        }
        let Some(window) = web_sys::window() else {
            set_reading(mount_id, Reading::Unavailable);
            return;
        };
        if !api_available(&window) {
            ENTRIES.with(|cell| {
                let mut map = cell.borrow_mut();
                let entry = map.entry(mount_id).or_insert_with(|| Entry {
                    reading: Reading::Unavailable,
                    last_ts_ms: None,
                    last_event_ms: 0.0,
                    on_orientation: None,
                    listening: false,
                });
                entry.reading = Reading::Unavailable;
                entry.listening = false;
            });
            return;
        }

        let has_closure = ENTRIES.with(|cell| {
            cell.borrow()
                .get(&mount_id)
                .map(|e| e.on_orientation.is_some())
                .unwrap_or(false)
        });

        if !has_closure {
            let closure =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                    handle_orientation(mount_id, event);
                });
            ENTRIES.with(|cell| {
                cell.borrow_mut().insert(
                    mount_id,
                    Entry {
                        reading: Reading::Waiting,
                        last_ts_ms: None,
                        last_event_ms: 0.0,
                        on_orientation: Some(closure),
                        listening: false,
                    },
                );
            });
        } else {
            set_reading(mount_id, Reading::Waiting);
        }

        let registered = ENTRIES.with(|cell| {
            let mut map = cell.borrow_mut();
            let Some(entry) = map.get_mut(&mount_id) else {
                return false;
            };
            let Some(closure) = entry.on_orientation.as_ref() else {
                return false;
            };
            let ok = window
                .add_event_listener_with_callback(
                    "deviceorientation",
                    closure.as_ref().unchecked_ref(),
                )
                .is_ok();
            entry.listening = ok;
            entry.last_event_ms = js_sys::Date::now();
            ok
        });

        if !registered {
            set_reading(mount_id, Reading::Unavailable);
        }
    }

    /// `mount_id` のセンサ取得を止める。リスナーを外し `Waiting` へ戻す（最後の値を
    /// 使い続けない、契約表 1）。`Closure` は drop せず保持し続ける（モジュール doc参照）。
    fn stop(mount_id: u64) {
        ENTRIES.with(|cell| {
            if let Some(entry) = cell.borrow_mut().get_mut(&mount_id) {
                if entry.listening {
                    if let (Some(window), Some(closure)) =
                        (web_sys::window(), entry.on_orientation.as_ref())
                    {
                        let _ = window.remove_event_listener_with_callback(
                            "deviceorientation",
                            closure.as_ref().unchecked_ref(),
                        );
                    }
                    entry.listening = false;
                }
                entry.reading = Reading::Waiting;
                entry.last_ts_ms = None;
            }
        });
    }

    /// `mount_id` のライフサイクル購読（`visibilitychange`/`pagehide`/`pageshow`）を
    /// （未登録なら）1 度だけ登録する。可視なら [`start`]、非表示なら [`stop`] を呼ぶ
    /// 共通ハンドラを 3 イベントへ登録する（モジュール doc参照）。
    fn register_lifecycle_once(mount_id: u64) {
        let already = LIFECYCLE.with(|cell| cell.borrow().contains_key(&mount_id));
        if already {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };

        let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            // `pagehide` 時点では `document.hidden` がまだ false のことがあるため、
            // イベント種別で判定して必ず止める（PR #997 codex 指摘）。
            let hidden = event.type_() == "pagehide"
                || web_sys::window()
                    .and_then(|w| w.document())
                    .map(|d| d.hidden())
                    .unwrap_or(true);
            if hidden {
                stop(mount_id);
            } else {
                start(mount_id);
            }
        });

        // 登録に失敗しても初回 `start` 自体は動くため、エラーは無視する
        // （panic を API 契約にしない、coding-rust.md 準拠）。
        let _ = document
            .add_event_listener_with_callback("visibilitychange", closure.as_ref().unchecked_ref());
        let _ =
            window.add_event_listener_with_callback("pagehide", closure.as_ref().unchecked_ref());
        let _ =
            window.add_event_listener_with_callback("pageshow", closure.as_ref().unchecked_ref());

        LIFECYCLE.with(|cell| {
            cell.borrow_mut().insert(mount_id, closure);
        });
    }

    /// `mount_id` の直前フレームからの経過秒数を実測し、`[0.0, MAX_DT]` へクランプして
    /// 返す（`gamepad.rs::measure_dt` と同じ役割）。
    fn measure_dt(mount_id: u64, ts_ms: f64) -> f32 {
        ENTRIES.with(|cell| {
            let mut map = cell.borrow_mut();
            let Some(entry) = map.get_mut(&mount_id) else {
                return DEFAULT_DT;
            };
            let dt = match entry.last_ts_ms {
                Some(prev) => (((ts_ms - prev) / 1000.0) as f32).clamp(0.0, MAX_DT),
                None => DEFAULT_DT,
            };
            entry.last_ts_ms = Some(ts_ms);
            dt
        })
    }

    /// `window.requestAnimationFrame(closure)` を呼ぶ（`gamepad.rs::schedule_next` と同じ）。
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

    /// 1 フレームぶんの処理: `reading` を読み、`Shown` と比較して必要なら適用する。
    fn run_tick(mount_id: u64, applier: &Applier<TiltBallState>, ts_ms: f64) {
        let dt = measure_dt(mount_id, ts_ms);
        let now_ms = js_sys::Date::now();
        let reading = ENTRIES.with(|cell| {
            let mut map = cell.borrow_mut();
            let entry = map.get_mut(&mount_id)?;
            // 初回待ちのまま一定時間イベントが来なければ `Unavailable` にする（契約表 6）。
            // `Live` 後の途絶は失敗にしない: Chrome は向きが有意に変わったときだけ
            // `deviceorientation` を出すため、机に置いた端末ではイベントが止まっても
            // 最後の値が正しい。非表示・`pagehide` では `stop` が `Waiting` へ戻すので、
            // 切断後に古い値を使い続けることは無い（契約表 1）。
            if entry.listening
                && entry.reading == Reading::Waiting
                && is_stale(entry.last_event_ms, now_ms, UNAVAILABLE_TIMEOUT_MS)
            {
                entry.reading = Reading::Unavailable;
            }
            Some(entry.reading)
        });
        let Some(reading) = reading else {
            return;
        };
        let shown = SHOWN.with(|cell| cell.borrow().get(&mount_id).copied());
        let Some(shown) = shown else {
            return;
        };
        if can_skip(&shown, &reading) {
            return;
        }
        applier.apply(move |s: &mut TiltBallState| match reading {
            Reading::Live(x, y) => {
                s.sensor = SensorStatus::Live;
                s.tilt_x = x;
                s.tilt_y = y;
                let phys_x = if x.abs() <= TILT_EPSILON { 0.0 } else { x };
                let phys_y = if y.abs() <= TILT_EPSILON { 0.0 } else { y };
                step(s, (phys_x, phys_y), dt);
            }
            Reading::Waiting => s.sensor = SensorStatus::Waiting,
            Reading::Unavailable => s.sensor = SensorStatus::Unavailable,
        });
    }

    /// `mount_id` の rAF ループを起動する（`gamepad.rs::start_loop` と同じ組み方。
    /// マウントのアンマウント API が無いため、このクロージャはページ寿命ぶん
    /// 生存し続ける、`gamepad.rs` と共通の天井）。
    // ponytail: `Closure` 1 個をページ寿命ぶん保持し続ける意図的なリーク。
    // upgrade path: `mount.rs` にアンマウント API が入ったら停止経路を足す。
    fn start_loop(mount_id: u64, applier: Applier<TiltBallState>) {
        let holder: FrameClosureHolder = Rc::new(RefCell::new(None));
        let holder_for_closure = holder.clone();
        *holder.borrow_mut() = Some(Closure::new(move |ts_ms: f64| {
            run_tick(mount_id, &applier, ts_ms);
            schedule_next(&holder_for_closure);
        }));
        schedule_next(&holder);
    }

    /// `mount_with_effect` へ渡す effect。`tilt_ball` 画面のセンサ取得を継続駆動する。
    ///
    /// 要求が無ければ何もしない契約（`mount.rs` の「effect フック」節）を、
    /// 「[`SHOWN`] を常に最新の表示内容へ更新しつつ、このマウントが既に初期化済み
    /// （[`ENTRIES`] にエントリがある）なら以降は初期化しない」という形で満たす
    /// （`gamepad.rs::gamepad_show_effect` と同じ考え方）。
    pub fn tilt_ball_effect(state: &TiltBallState, applier: &Applier<TiltBallState>) {
        let mount_id = applier.mount_id();
        SHOWN.with(|cell| {
            cell.borrow_mut().insert(mount_id, Shown::from_state(state));
        });

        let already_initialized = ENTRIES.with(|cell| cell.borrow().contains_key(&mount_id));
        if already_initialized {
            return;
        }

        register_lifecycle_once(mount_id);
        start(mount_id);
        start_loop(mount_id, applier.clone());
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::tilt_ball_effect;

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < EPS, "{a} !~= {b}");
    }

    #[test]
    fn is_stale_only_after_timeout() {
        assert!(!is_stale(1_000.0, 3_999.0, 3_000.0));
        assert!(is_stale(1_000.0, 4_000.0, 3_000.0));
    }

    #[test]
    fn tilt_from_orientation_flat_is_zero() {
        let (x, y) = tilt_from_orientation(Some(0.0), Some(0.0), 0).unwrap();
        approx(x, 0.0);
        approx(y, 0.0);
    }

    #[test]
    fn tilt_from_orientation_upright_tilts_down() {
        let (x, y) = tilt_from_orientation(Some(90.0), Some(0.0), 0).unwrap();
        approx(x, 0.0);
        approx(y, 1.0);
    }

    #[test]
    fn tilt_from_orientation_right_gamma_tilts_right() {
        let (x, y) = tilt_from_orientation(Some(0.0), Some(90.0), 0).unwrap();
        approx(x, 1.0);
        approx(y, 0.0);
    }

    #[test]
    fn tilt_from_orientation_negative_gamma_tilts_left() {
        let (x, _y) = tilt_from_orientation(Some(0.0), Some(-45.0), 0).unwrap();
        assert!(x < 0.0);
    }

    #[test]
    fn tilt_from_orientation_none_when_beta_or_gamma_missing() {
        assert_eq!(tilt_from_orientation(None, Some(0.0), 0), None);
        assert_eq!(tilt_from_orientation(Some(0.0), None, 0), None);
        assert_eq!(tilt_from_orientation(None, None, 0), None);
    }

    #[test]
    fn tilt_from_orientation_none_when_non_finite() {
        assert_eq!(tilt_from_orientation(Some(f64::NAN), Some(0.0), 0), None);
        assert_eq!(
            tilt_from_orientation(Some(0.0), Some(f64::INFINITY), 0),
            None
        );
    }

    #[test]
    fn tilt_from_orientation_rotation_table_matches_gravity_to_tilt_signs() {
        // 各回転角で「立てる」（beta=90）操作の結果が egui 版 gravity_to_tilt の
        // 回転表と同じ符号パターンになることを確認する。
        let base = tilt_from_orientation(Some(90.0), Some(0.0), 0).unwrap();
        assert_eq!(base, (0.0, 1.0));

        let r90 = tilt_from_orientation(Some(90.0), Some(0.0), 90).unwrap();
        approx(r90.0, 1.0);
        approx(r90.1, 0.0);

        let r180 = tilt_from_orientation(Some(90.0), Some(0.0), 180).unwrap();
        approx(r180.0, 0.0);
        approx(r180.1, -1.0);

        let r270 = tilt_from_orientation(Some(90.0), Some(0.0), 270).unwrap();
        approx(r270.0, -1.0);
        approx(r270.1, 0.0);
    }

    #[test]
    fn tilt_from_orientation_unknown_angle_falls_back_to_zero() {
        let expected = tilt_from_orientation(Some(30.0), Some(10.0), 0);
        let actual = tilt_from_orientation(Some(30.0), Some(10.0), 45);
        assert_eq!(actual, expected);
    }

    #[test]
    fn tilt_from_orientation_output_is_always_clamped() {
        let (x, y) = tilt_from_orientation(Some(180.0), Some(180.0), 0).unwrap();
        assert!((-1.0..=1.0).contains(&x));
        assert!((-1.0..=1.0).contains(&y));
    }

    fn shown(sensor: SensorStatus, tilt_x: f32, tilt_y: f32, at_rest: bool) -> Shown {
        Shown {
            sensor,
            tilt_x,
            tilt_y,
            at_rest,
        }
    }

    #[test]
    fn can_skip_true_for_initial_waiting_display_and_waiting_reading() {
        let s = shown(SensorStatus::Waiting, 0.0, 0.0, true);
        assert!(can_skip(&s, &Reading::Waiting));
    }

    #[test]
    fn can_skip_false_when_waiting_display_receives_live_reading() {
        let s = shown(SensorStatus::Waiting, 0.0, 0.0, true);
        assert!(!can_skip(&s, &Reading::Live(0.5, 0.0)));
    }

    #[test]
    fn can_skip_true_when_live_display_within_epsilon_and_at_rest() {
        let s = shown(SensorStatus::Live, 0.0, 0.0, true);
        assert!(can_skip(&s, &Reading::Live(0.0, 0.0)));
    }

    #[test]
    fn can_skip_false_when_live_display_has_not_converged() {
        let s = shown(SensorStatus::Live, 0.5, 0.0, true);
        assert!(!can_skip(&s, &Reading::Live(0.0, 0.0)));
    }

    #[test]
    fn can_skip_false_when_not_at_rest() {
        let s = shown(SensorStatus::Live, 0.0, 0.0, false);
        assert!(!can_skip(&s, &Reading::Live(0.0, 0.0)));
    }

    #[test]
    fn can_skip_false_when_live_display_receives_unavailable_reading() {
        let s = shown(SensorStatus::Live, 0.0, 0.0, true);
        assert!(!can_skip(&s, &Reading::Unavailable));
    }

    #[test]
    fn can_skip_true_for_matching_unavailable() {
        let s = shown(SensorStatus::Unavailable, 0.0, 0.0, true);
        assert!(can_skip(&s, &Reading::Unavailable));
    }
}
