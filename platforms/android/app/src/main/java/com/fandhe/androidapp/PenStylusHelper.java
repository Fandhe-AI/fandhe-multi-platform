package com.fandhe.androidapp;

import android.view.InputDevice;
import android.view.MotionEvent;

// ペン・筆圧入力（Issue #357、親 #95）の Android 側観測ヘルパー。
// winit の Android backend は tool type と AXIS_TILT を捨てるため、MainActivity の
// `dispatchTouchEvent` から MotionEvent を観測し（消費しない）、主ポインタ 1 本の
// 種類・筆圧・傾きを Rust の `device/android_pen_stylus.rs`（SharedPen）へ push する。
// 位置は送らない（egui のポインタから取る）。UI スレッドだけで呼ばれるため lock は持たない。
// ponytail: 主ポインタ 1 本・履歴サンプルを読まない。速い筆致で点が粗くなるなら getHistorical* を足す。
final class PenStylusHelper {
    private static final int INVALID = -1;

    private int trackedId = INVALID;

    private static native void nativeOnPenSample(
            int toolType, float pressure, float tiltRad, boolean hasTilt);

    private static native void nativeOnPenClear();

    /** MotionEvent を観測する。イベントは消費しない（呼び出し側が super の戻り値を返す）。 */
    void onTouch(MotionEvent ev) {
        switch (ev.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                trackedId = ev.getPointerId(0);
                push(ev);
                break;
            case MotionEvent.ACTION_MOVE:
            case MotionEvent.ACTION_POINTER_DOWN:
                push(ev);
                break;
            case MotionEvent.ACTION_POINTER_UP:
                if (ev.getPointerId(ev.getActionIndex()) == trackedId) {
                    release();
                } else {
                    push(ev);
                }
                break;
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_CANCEL:
                release();
                break;
            default:
                break;
        }
    }

    // 追跡中ポインタの生値を送る。いなければ接触なしへ戻す。
    private void push(MotionEvent ev) {
        if (trackedId == INVALID) {
            return;
        }
        int i = ev.findPointerIndex(trackedId);
        if (i < 0) {
            release();
            return;
        }
        // 傾きを報告しない機器の 0 を「垂直」と取り違えないよう、軸の有無を別に渡す。
        InputDevice dev = ev.getDevice();
        boolean hasTilt =
                dev != null && dev.getMotionRange(MotionEvent.AXIS_TILT, ev.getSource()) != null;
        nativeOnPenSample(
                ev.getToolType(i),
                ev.getPressure(i),
                ev.getAxisValue(MotionEvent.AXIS_TILT, i),
                hasTilt);
    }

    private void release() {
        trackedId = INVALID;
        nativeOnPenClear();
    }

    /** 背景遷移で接触なしに戻す（最後の値を出し続けない）。 */
    void onPause() {
        release();
    }

    /** Activity 破棄で接触なしに戻す。clear は冪等で値を消す方向にしか働かない。 */
    void onDestroy() {
        release();
    }
}
