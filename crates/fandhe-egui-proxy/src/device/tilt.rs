//! Phase 10（親 #827）型サンプル「傾きで玉が転がる」（Issue #830）の effect ドライバ。
//! `fandhe_core::screens::tilt_ball::{TiltBallState, step}`（端末依存ゼロ）へ、
//! `fandhe_device::motion::Motion` から読んだ傾き値と実測 `dt` を渡して毎フレーム
//! 物理を進める（`crate::screens::registry` の `effect` 区画から `fn(&S,
//! &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # デスクトップのスライダー代替と実機センサ（#831/#832）の関係
//!
//! [`shared_motion`] が返す `DummyMotion` はプロセス内で 1 個だけ共有する（`OnceLock`）。
//! デスクトップ（macOS/Windows/Linux）は [`drive`] が毎フレーム
//! `motion.set(state.tilt_x, state.tilt_y)` でスライダー値を書き込んでから読み戻す
//! （スライダー UI がセンサの代替として振る舞う、`fandhe_device::motion` モジュール doc
//! 参照）。iOS（#832）は [`drive`] が毎フレーム `device::ios::IosMotion` を pull し、
//! 読んだ値を `shared_motion().set(x, y)` へ書き込む（CoreMotion に push
//! コールバックを持たせず、既存の読み出し経路（`Motion::tilt()` 経由）をそのまま
//! 流用できる設計、`device/ios.rs` のモジュール doc 参照）。Android（#831）は
//! 実センサの push コールバックから直接 `shared_motion().set(x, y)` を呼ぶ想定
//! （`set` 呼び出し自体はデスクトップ/iOS 側の pull・Android の push で行い、
//! 実機では [`drive`] のデスクトップ分岐が上書きしないようにする）。

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use fandhe_core::screens::tilt_ball::{is_at_rest, step, TiltBallState};
use fandhe_device::motion::{DummyMotion, Motion};

use crate::app::CompletionSender;

/// 1 フレームで進める時間の上限（`fandhe_core::screens::tilt_ball::step` と同じ値。
/// ProMotion 120Hz・バックグラウンド復帰の飛び対策）。
const MAX_DT: f32 = 0.1;
/// `LAST_TICK` が未初期化（起動直後の 1 フレーム目）のときに使う既定 dt（60fps 相当）。
const DEFAULT_DT: f32 = 1.0 / 60.0;

/// 「傾き無し」とみなす閾値（重力ベクトル成分の絶対値、約 1 度相当）。
/// 実センサ（iOS `CMMotionManager` の `gravity`）は水平静置でも厳密な `0.0` を
/// 返さず微小ノイズが乗るため、`== 0.0` の等値比較では静止判定がほぼ成立しない
/// （Cursor Bugbot 指摘 Medium、PR #844: 毎フレーム完了送信が続き `request_repaint`
/// が止まらない）。閾値以下をノイズとして無視することで、デスクトップのスライダー
/// 入力（厳密な `0.0` を作れる）・実センサ双方で同じ静止判定を成立させる。
const TILT_EPSILON: f32 = 0.02;

/// iOS（pull 方式、モジュール doc 参照）で静止判定によりガードした後、次に
/// `CMMotionManager` を pull し直すまでの間隔。
///
/// iOS のセンサ実装は push コールバックを持たないため、ここで再描画予約を
/// 切ると次にセンサ値を読みに来る機会が無くなり、静止後に端末を傾けても
/// `update` が呼ばれなくなる（codex-review 指摘 P1、PR #844）。無条件に
/// 60fps で pull し続けると静止判定を入れた意味（Issue #830 実装計画 §2(d)）が
/// 薄れるため、この間隔での定期 pull に妥協する。
/// ponytail: 固定間隔のポーリング。取りこぼし（最大 `IOS_IDLE_POLL_INTERVAL` 分の
/// 反応遅延）が要件上問題になった場合は、加速度の変化量をトリガーに即時
/// 再描画する等の設計に上げる。
#[cfg(target_os = "ios")]
const IOS_IDLE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// プロセス内で共有する `DummyMotion`（デスクトップのスライダー代替・#831/#832 の
/// push 先、モジュール doc 参照）。
static MOTION: OnceLock<DummyMotion> = OnceLock::new();

/// 共有 `DummyMotion` への参照を返す。
pub fn shared_motion() -> &'static DummyMotion {
    MOTION.get_or_init(DummyMotion::new)
}

/// `SensorManager.GRAVITY_EARTH`（Android）と同値の標準重力加速度（m/s^2）。
/// [`gravity_to_tilt`] が重力ベクトルを `-1.0..=1.0` の傾き値へ正規化する除数として使う。
/// 実呼び出し元（`device/android.rs` の JNI エクスポート）は Android ターゲットのみに
/// 存在するため、他ターゲットでは `#[cfg(test)]` のユニットテストのみが参照する
/// （dead_code 誤検知を避けるため `cfg_attr` で許可する）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const GRAVITY_EARTH: f32 = 9.80665;

/// Android の重力センサ値（`TYPE_GRAVITY`、無ければ `TYPE_ACCELEROMETER`）を画面座標の
/// 傾き `(tilt_x, tilt_y)` へ写像する純関数（Issue #831）。`device/android.rs` の JNI
/// エクスポートから呼ばれ、ホストの `cargo test` で検証できるようロジックをここへ
/// 切り出している。
///
/// 入力 `(gx, gy)` はセンサ座標系（x 右・y 上、静止時の重力ベクトルは「空の方向」を向く。
/// 例: 水平置きで z 成分が正）。`rotation` は `Surface.ROTATION_*`（0=0°/1=90°/2=180°/
/// 3=270°、画面の回転方向）で、センサ軸を現在の画面向きへ合わせるために使う。
///
/// 出力は `fandhe_core::screens::tilt_ball` の契約（左上原点・x 右/y 下が正、
/// `tilt.0 > 0` で右、`tilt.1 > 0` で下へ加速）に従う。未知の `rotation` 値は
/// 回転なし（`Surface.ROTATION_0`）と同じ写像へフォールバックする（fail-safe。
/// OS から想定外の値が来ても panic・未定義動作にしない、security.md A03 準拠）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn gravity_to_tilt(gx: f32, gy: f32, rotation: i32) -> (f32, f32) {
    // 画面基準（右方向, 上方向）成分へ写像する。回転した分だけセンサ軸を逆回転させる。
    let (sx, sy) = match rotation {
        1 => (-gy, gx),
        2 => (-gx, -gy),
        3 => (gy, -gx),
        _ => (gx, gy),
    };
    (-sx / GRAVITY_EARTH, sy / GRAVITY_EARTH)
}

/// 直近の `tilt_ball_effect` 呼び出し時刻（dt 実測用）。
static LAST_TICK: Mutex<Option<Instant>> = Mutex::new(None);

/// 傾き 0 かつ静止中に完了送信をスキップしてよいかを判定する純関数（`drive` から
/// プラットフォーム cfg 抜きでテストできるよう切り出す）。
///
/// 「実測した傾き `(x, y)` が [`TILT_EPSILON`] 以下」「玉の速度が [`is_at_rest`]」に
/// 加え、「表示用の `state.tilt_x`/`tilt_y` も既に [`TILT_EPSILON`] 以下まで収束済み」
/// の 3 条件がすべて揃ったときのみスキップする。表示値が収束する前（端末を傾けてから
/// 水平へ戻した直後）にスキップすると、`state.tilt_x`/`tilt_y` が最後に傾けた値の
/// まま更新されずステータス・スライダーが水平を示さない（人間レビュアー指摘 P2、PR
/// #844）。収束するまでは `false` を返し、通常経路の完了送信で表示値を更新させる。
fn can_skip_send(state: &TiltBallState, x: f32, y: f32) -> bool {
    x.abs() <= TILT_EPSILON
        && y.abs() <= TILT_EPSILON
        && is_at_rest(state)
        && state.tilt_x.abs() <= TILT_EPSILON
        && state.tilt_y.abs() <= TILT_EPSILON
}

/// 前回呼び出しからの経過秒数を実測し、`[0.0, MAX_DT]` へクランプして返す。
/// 初回呼び出し（`LAST_TICK` が `None`）は [`DEFAULT_DT`] を使う。
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

/// `motion` から傾きを読み、静止 + 傾き 0 でなければ [`step`] を適用する完了を送る。
///
/// デスクトップのみ、読み出し前に `state.tilt_x`/`tilt_y`（スライダー入力値）を
/// `motion` へ書き込む（モジュール doc 参照。Android/iOS は実センサ値を上書きしない）。
/// `Motion::tilt()` が `Err` を返す（未対応プラットフォーム）場合は何もしない。
/// 傾きが [`TILT_EPSILON`] 以下（実センサのノイズをノイズとして無視するための
/// 閾値、PR #844 是正）かつ玉が静止中は送信をスキップし、egui をスリープさせる
/// （`CompletionSender::send` が毎回 `request_repaint` を呼ぶため、無条件送信だと
/// 静止中も毎フレーム描画が回り続けてしまう、Issue #830 実装計画 §2(d)）。iOS の
/// みこのスキップ中も [`IOS_IDLE_POLL_INTERVAL`] 間隔で起こし直し、次のセンサ
/// サンプルを取りこぼさないようにする（同 PR 是正、[`IOS_IDLE_POLL_INTERVAL`]
/// のコメント参照）。
fn drive(
    state: &TiltBallState,
    sender: &CompletionSender<TiltBallState>,
    motion: &DummyMotion,
    dt: f32,
) {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    motion.set(state.tilt_x, state.tilt_y);

    // iOS は実センサを pull し、共有状態（`motion`）へ書き込んでから下記で
    // 読み戻す（モジュール doc 参照）。`IosMotion::tilt()` が `Err` を返す
    // （device motion 非対応、シミュレータ等）場合は共有状態を書き換えず、
    // 既存の `(0.0, 0.0)` 初期値のまま扱う。
    #[cfg(target_os = "ios")]
    if let Ok((x, y)) = crate::device::ios::IosMotion.tilt() {
        motion.set(x, y);
    }

    let (x, y) = match motion.tilt() {
        Ok(tilt) => tilt,
        Err(_) => return,
    };

    if can_skip_send(state, x, y) {
        // iOS のみ: 定期的に起こして pull し直す（IOS_IDLE_POLL_INTERVAL のコメント参照）。
        // Android/デスクトップは push（#831 実装後）・次回入力で再度呼ばれる想定のため不要。
        #[cfg(target_os = "ios")]
        sender.request_repaint_after(IOS_IDLE_POLL_INTERVAL);
        return;
    }

    // 物理更新へ渡す入力は [`TILT_EPSILON`] 以下をゼロ扱いにする（表示用の
    // `state.tilt_x`/`tilt_y` は実測値 x/y をそのまま使う）。閾値が送信スキップの
    // 判定にしか使われないと、玉が動いている間は閾値内のノイズがそのまま
    // `step` の入力になり続け、水平に置いても玉が止まりきらない
    // （codex-review 指摘 P2、PR #844）。
    let phys_x = if x.abs() <= TILT_EPSILON { 0.0 } else { x };
    let phys_y = if y.abs() <= TILT_EPSILON { 0.0 } else { y };

    // 受信側（ProxyApp）がドロップ済み（アプリ終了中）の送信失敗は無視してよい
    // （coding-rust.md: panic を API 契約にしない、`device/http_fetch.rs` と同じ判断）。
    let _ = sender.send(Box::new(move |s: &mut TiltBallState| {
        s.tilt_x = x;
        s.tilt_y = y;
        step(s, (phys_x, phys_y), dt);
    }));
}

/// `registry.rs` の `effect` 区画から `tilt_ball` 画面用に配線される effect ドライバ。
pub fn tilt_ball_effect(state: &TiltBallState, sender: &CompletionSender<TiltBallState>) {
    let dt = measure_dt();
    drive(state, sender, shared_motion(), dt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::tilt_ball::TiltBallState;

    fn sender_with_channel() -> (
        CompletionSender<TiltBallState>,
        std::sync::mpsc::Receiver<crate::app::Completion<TiltBallState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    #[test]
    fn drive_is_noop_when_at_rest_and_tilt_is_zero() {
        let motion = DummyMotion::new();
        let (sender, rx) = sender_with_channel();
        let state = TiltBallState::default();

        drive(&state, &sender, &motion, 1.0 / 60.0);

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drive_is_noop_when_tilt_is_within_noise_epsilon() {
        // 実センサのノイズ相当（`TILT_EPSILON` 以下）は「傾き無し」として扱い、
        // 厳密な `0.0` でなくても静止判定が成立することを確認する
        // （Cursor Bugbot 指摘 Medium、PR #844: 等値比較だと静止判定が成立せず
        // 毎フレーム完了が送られ続けてしまっていた）。
        let motion = DummyMotion::new();
        let (sender, rx) = sender_with_channel();
        let mut state = TiltBallState::default();
        state.tilt_x = TILT_EPSILON;
        state.tilt_y = -TILT_EPSILON;

        drive(&state, &sender, &motion, 1.0 / 60.0);

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drive_applies_step_when_tilt_exceeds_noise_epsilon() {
        let motion = DummyMotion::new();
        let (sender, rx) = sender_with_channel();
        let mut state = TiltBallState::default();
        state.tilt_x = TILT_EPSILON + 0.01;

        drive(&state, &sender, &motion, 1.0 / 60.0);

        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn can_skip_send_is_false_while_stale_display_has_not_converged() {
        // 端末を傾けてから水平へ戻した直後を模す: 実測 (x, y) は閾値以下・玉は
        // 静止済みだが、表示用の state.tilt_x はまだ以前の傾き値のまま。
        // 収束前にスキップすると水平が表示されない（人間レビュアー指摘 P2、PR #844）。
        let mut state = TiltBallState::default();
        state.tilt_x = 0.5;

        assert!(!can_skip_send(&state, 0.0, 0.0));
    }

    #[test]
    fn can_skip_send_is_true_once_display_has_converged() {
        let state = TiltBallState::default();

        assert!(can_skip_send(&state, 0.0, 0.0));
    }

    #[test]
    fn drive_applies_step_when_tilted() {
        let motion = DummyMotion::new();
        let (sender, rx) = sender_with_channel();
        // desktop 経路では `drive` 自身が `state.tilt_x`/`tilt_y` を `motion` へ
        // 書き込んでから読み戻す（モジュール doc 参照）ため、傾きは `state` 側に
        // 与える。ここで `motion.set` を先に呼んでも `drive` の書き込みで上書きされ、
        // 傾き 0 のまま static 判定に落ちて完了が送られず `rx.recv()` がハングする
        // （実装検証中に踏んだ回帰、テストは実際の呼び出し順序に合わせておく）。
        let mut state = TiltBallState::default();
        state.tilt_x = 1.0;

        drive(&state, &sender, &motion, 1.0 / 60.0);

        let completion = rx.recv().expect("傾きありなら完了が送られる");
        let before = state.ball_position();
        completion(&mut state);
        assert!(state.ball_position().0 > before.0);
        assert_eq!(state.tilt_x, 1.0);
    }

    #[test]
    fn drive_writes_state_tilt_into_motion_on_desktop() {
        let motion = DummyMotion::new();
        let (sender, rx) = sender_with_channel();
        let mut state = TiltBallState::default();
        state.tilt_x = 0.7;
        state.tilt_y = -0.3;

        drive(&state, &sender, &motion, 1.0 / 60.0);

        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        assert_eq!(motion.tilt().unwrap(), (0.7, -0.3));

        // 傾きありのため完了が送られる（読み出しチャンネルを消費しておく）。
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn measure_dt_is_clamped_to_max() {
        // 他テストと static を共有するため厳密な値は問わず、範囲のみ検証する。
        let dt = measure_dt();
        assert!((0.0..=MAX_DT).contains(&dt));
    }

    #[test]
    fn gravity_to_tilt_upright_portrait_tilts_down() {
        // 縦持ちで直立（画面が鉛直）させると重力ベクトルの y 成分が正になり、
        // 玉は画面下（tilt_y > 0）へ落ちる。
        let (tilt_x, tilt_y) = gravity_to_tilt(0.0, GRAVITY_EARTH, 0);
        assert!((tilt_y - 1.0).abs() < 1e-5);
        assert!(tilt_x.abs() < 1e-5);
    }

    #[test]
    fn gravity_to_tilt_right_edge_down_tilts_right() {
        // 右端を下げる（右側が低くなる）とセンサの x 成分が負になり、
        // 玉は画面右（tilt_x > 0）へ転がる。
        let (tilt_x, tilt_y) = gravity_to_tilt(-GRAVITY_EARTH, 0.0, 0);
        assert!((tilt_x - 1.0).abs() < 1e-5);
        assert!(tilt_y.abs() < 1e-5);
    }

    #[test]
    fn gravity_to_tilt_rotation_90_swaps_axes() {
        // 90 度回転時はセンサ軸が入れ替わる。
        let (tilt_x, tilt_y) = gravity_to_tilt(0.0, GRAVITY_EARTH, 1);
        assert!((tilt_x - 1.0).abs() < 1e-5);
        assert!(tilt_y.abs() < 1e-5);
    }

    #[test]
    fn gravity_to_tilt_rotation_180_inverts_both_axes() {
        let (tilt_x, tilt_y) = gravity_to_tilt(0.0, GRAVITY_EARTH, 2);
        assert!((tilt_y - (-1.0)).abs() < 1e-5);
        assert!(tilt_x.abs() < 1e-5);
    }

    #[test]
    fn gravity_to_tilt_rotation_270_swaps_and_inverts() {
        let (tilt_x, tilt_y) = gravity_to_tilt(0.0, GRAVITY_EARTH, 3);
        assert!((tilt_x - (-1.0)).abs() < 1e-5);
        assert!(tilt_y.abs() < 1e-5);
    }

    #[test]
    fn gravity_to_tilt_unknown_rotation_falls_back_to_rotation_0() {
        // 未知の rotation 値（OS からの想定外入力）は回転なしと同じ結果にする
        // fail-safe（§3(b) 表の「その他」分岐）。
        let expected = gravity_to_tilt(1.0, 2.0, 0);
        let actual = gravity_to_tilt(1.0, 2.0, 7);
        assert_eq!(actual, expected);
    }

    #[test]
    fn gravity_to_tilt_out_of_range_input_is_not_clamped_here() {
        // gravity_to_tilt 自体は範囲外値をクランプしない。呼び出し側
        // （`DummyMotion::set`）が NaN 除去・`-1.0..=1.0` クランプを担う
        // （モジュール doc「デスクトップのスライダー代替と実機センサの関係」参照）。
        let (tilt_x, _) = gravity_to_tilt(-3.0 * GRAVITY_EARTH, 0.0, 0);
        assert!(tilt_x > 1.0);
    }
}
