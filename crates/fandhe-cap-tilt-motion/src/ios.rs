//! iOS 向け傾きセンサ実装（[`IosMotion`]、Issue #832）。`ios/fandhe_ios_motion.m`（CoreMotion の `CMMotionManager`）を
//! `unsafe extern "C"` 1 関数で呼び、本 crate の `driver` の effect ドライバから毎フレーム pull される。
//! 共有の `CMMotionManager`（`fandhe_ios_shared_motion_manager` など）は motion-raw-sensors の iOS 実装（`.m`）も使うため、
//! motion-raw-sensors の機能 crate が iOS の target 依存で本 crate に依存する（consumer-app-structure.md §8）。
//!
//! 守る範囲: main スレッド以外の呼び出しを FFI へ進めないこと、FFI 境界を越えた値の正規化。
//! 守らない範囲: センサの精度・周期、UI 向きの変化の即時反映。

use crate::contract::{Motion, clamp_finite};
use crate::error::TiltMotionError;
// main thread 判定は Apple 共通層（`fandhe-platform-apple`）が持つ。UIKit を触る呼び出し
// （`fandhe_ios_motion_read` 等）の前に、ワーカースレッドからの誤用をここで弾く（codex-review 指摘 P1、PR #844）。
use fandhe_platform_apple::is_main_thread;

// SAFETY: `ios/fandhe_ios_motion.m` が定義する C ABI（`build.rs` が iOS ターゲットの
// み・feature `device` 有効時のみコンパイル・リンクする）。契約は下記 `tilt`
// 実装のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_motion_read(out_x: *mut f32, out_y: *mut f32) -> i32;
}

/// iOS 実センサ実装（`CMMotionManager` の gravity、Issue #832）。
/// 本 crate の `driver` の effect ドライバから毎フレーム pull される
/// （push ではなく pull にする設計判断はモジュール doc・`driver.rs` 冒頭コメント参照）。
pub struct IosMotion;

impl Motion for IosMotion {
    #[allow(unsafe_code)]
    fn tilt(&self) -> Result<(f32, f32), TiltMotionError> {
        // `fandhe_ios_motion_read` は main スレッド専用（`is_main_thread` の
        // ドキュメンテーションコメント参照）。ワーカースレッドから呼ばれた場合は
        // FFI へ進まずエラーを返す。
        if !is_main_thread() {
            return Err(TiltMotionError::Internal("motion: called off main thread"));
        }

        let mut out_x: f32 = 0.0;
        let mut out_y: f32 = 0.0;

        // SAFETY: `fandhe_ios_motion_read` の契約（`.m` ファイル冒頭コメント）:
        // out 引数はこの関数呼び出し中のみ有効なスタック上の `f32` への
        // ポインタで、所有権移転・malloc は無い（malloc を伴う camera 経路と違い解放処理が
        // 不要）。戻り値が 0 以外のとき out 引数は書き換えられない契約のため、
        // 呼び出し直前の初期値 `(0.0, 0.0)` のまま扱う。
        let code = unsafe { fandhe_ios_motion_read(&mut out_x, &mut out_y) };

        match code {
            0 => {}
            // 1 = 端末が device motion 非対応（シミュレータ等）。0 以外の未知の
            // コードも安全側で同様に扱う（契約違反を検出した経路）。
            _ => return Err(TiltMotionError::Unsupported("motion")),
        }

        // `fandhe_ios_motion_read` は現在の UI 向き（`UIInterfaceOrientation`）基準
        // の画面右・画面上方向へ既に回転済みの値を返す（`fandhe_ios_motion.m` の
        // `FandheRotateGravityToInterfaceOrientation` 参照、Bugbot 指摘 Medium 是正:
        // portrait 前提の Y 反転のみだと landscape / iPad 回転で玉が誤った軸へ
        // 転がっていた）。ここではその画面基準の y（上方向が正）を
        // `fandhe_core::screens::tilt_ball` の座標系（x: 右方向、y: 下方向が正）へ
        // 合わせるため反転するだけでよい（画面上向き正 → 反転で下向き正、向きが
        // 変わっても常に「画面の上」基準なのでこの反転だけで全 UI 向きに対応できる）。
        // NaN → 0.0 置換 + -1.0..=1.0 クランプは `DummyMotion::set` 側でも行う
        // 二重防御（security.md A08）だが、FFI 境界を越えた値をこの関数内でも
        // 正規化しておく。
        Ok((clamp_finite(out_x), clamp_finite(-out_y)))
    }
}
