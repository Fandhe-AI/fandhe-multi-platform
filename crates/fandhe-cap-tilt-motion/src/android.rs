//! Android 向け傾きセンサの受け口（Issue #831、Phase 10 型サンプル「傾けると玉が転がる」の実センサ経路）。
//! Java 側 `TiltMotionHelper.nativeOnMotionChanged`（UI スレッドの `SensorEventListener#onSensorChanged`）が受けた重力センサ値
//! `(x, y)` と画面回転 `rotation`（`Surface.ROTATION_*`）を、[`crate::gravity_to_tilt`] で画面座標の傾きへ写像して
//! [`crate::shared_motion`] へ push する。`tilt_ball_effect` が毎フレームここを読むだけで実機の傾きが反映される
//! （新しい `Motion` 実装は追加しない設計、#831 実装計画 §3(a)）。
//!
//! Java から Rust への一方向（Rust から Java を呼ぶ入口は無い）。画面が傾きを使わないときの再描画の抑制は
//! [`set_screen_active`] のゲートで行う（codex-review 指摘 P2、Issue #831）。
//!
//! 守る範囲: 傾きの値を毎回保存すること、傾きを使わない画面で再描画を要求し続けないこと。
//! 守らない範囲: センサの精度・周期。

use std::sync::atomic::{AtomicBool, Ordering};

/// 現在表示中の画面が傾き（`tilt_ball`）を使うかどうか。Java 側の `motionListener` は画面に関わらず常にセンサを登録して
/// 通知するため、素直に毎回 `request_repaint` すると傾きを使わない画面でも `SENSOR_DELAY_GAME` の頻度で再描画要求が発生し
/// 不要な電力消費になる。起動画面（`FANDHE_PROXY_APP` で決まる画面は `Activity` 生存中は変わらない）の決定直後に
/// 最終成果物（`fandhe-gallery` の `android_main`）が [`set_screen_active`] で設定する。
static SCREEN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 画面が傾きを使うかを差し替える（`android_main` が起動画面を決定した直後に呼ぶ）。
pub fn set_screen_active(active: bool) {
    SCREEN_ACTIVE.store(active, Ordering::Relaxed);
}

/// `TiltMotionHelper.nativeOnMotionChanged`（Java 側）から呼ばれる JNI エクスポート関数（Issue #831）。
/// 値の保存自体は画面に関わらず毎回行うが、`request_repaint` は [`set_screen_active`] が真の場合に限る。傾き画面が有効な場合は、
/// 静止時にセンサが厳密な `(0, 0)` を送ってきた場合（水平置き等）や初回センサ通知前に `drive` が非送信で描画が止まっていても、
/// 以後の非ゼロ値到着で確実に再開する（`drive` 自体の自走に依存しない）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`JNIEnv`/`jclass` は使わず、受け取った値を計算して static へ store するのみ）。
/// `#[no_mangle]` は JNI 命名規約上の固定シンボル名であり、本 crate は cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_TiltMotionHelper_nativeOnMotionChanged(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    x: f32,
    y: f32,
    rotation: i32,
) {
    let (tilt_x, tilt_y) = crate::gravity_to_tilt(x, y, rotation);
    crate::shared_motion().set(tilt_x, tilt_y);
    if SCREEN_ACTIVE.load(Ordering::Relaxed) {
        fandhe_runtime::request_repaint();
    }
}
