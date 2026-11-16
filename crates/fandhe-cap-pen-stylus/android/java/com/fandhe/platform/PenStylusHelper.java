package com.fandhe.platform;

import android.app.Activity;
import android.view.InputDevice;
import android.view.MotionEvent;

import androidx.activity.ComponentActivity;
import androidx.core.util.Consumer;

import java.util.Map;
import java.util.WeakHashMap;

// ペン・筆圧入力（Issue #357、親 #95）の Android 側観測ヘルパー。
// winit の Android backend は tool type と AXIS_TILT を捨てるため、`PenStylusLifecycle` が `onHostCreated` で共通層 `HostInput`（#1327）へ
// 登録したタッチの観測者として MotionEvent を受け（消費しない）、主ポインタ 1 本の
// 種類・筆圧・傾きを Rust の `fandhe-cap-pen-stylus/src/android.rs`（SharedPen）へ push する。
// 位置は送らない（egui のポインタから取る）。UI スレッドだけで呼ばれるため lock は持たない。
// 観測者は `static` ではなくこのインスタンスが持つ同一の `Consumer`（再登録の重複排除が効くよう、メソッド参照を毎回書かない）。
// 背景遷移・破棄は `onHostPaused` / `onHostDestroyed` から届く（`Activity.onPause()` / `onDestroy()` の `super` の中。従来の
// `MainActivity` は `super.onPause()` の後・`onDestroy` では `super` の前だったので、`super` に対する前後だけが違う。
// どちらも接触なしへ戻す冪等な操作）。
// ponytail: 主ポインタ 1 本・履歴サンプルを読まない。速い筆致で点が粗くなるなら getHistorical* を足す。
final class PenStylusHelper {
    private static final int INVALID = -1;

    // Activity ごとの helper。再生成後は新しい Activity に新しい helper が付き、旧 Activity の分は破棄時に外す。
    private static final Map<Activity, PenStylusHelper> HELPERS = new WeakHashMap<>();

    private final Consumer<MotionEvent> touchObserver = this::onTouch;
    private int trackedId = INVALID;

    // `onHostCreated`（UI スレッド）から呼ぶ。
    static void attach(ComponentActivity activity) {
        PenStylusHelper helper = new PenStylusHelper();
        synchronized (HELPERS) {
            HELPERS.put(activity, helper);
        }
        HostInput.addTouchObserver(activity, helper.touchObserver);
    }

    // `onHostPaused`（UI スレッド）から呼ぶ。
    static void paused(Activity activity) {
        PenStylusHelper helper;
        synchronized (HELPERS) {
            helper = HELPERS.get(activity);
        }
        if (helper != null) {
            helper.onPause();
        }
    }

    // `onHostDestroyed`（UI スレッド）から呼ぶ。
    static void detach(Activity activity) {
        PenStylusHelper helper;
        synchronized (HELPERS) {
            helper = HELPERS.remove(activity);
        }
        if (helper != null) {
            helper.onDestroy();
        }
    }

    private static native void nativeOnPenSample(
            int toolType, float pressure, float tiltRad, boolean hasTilt);

    private static native void nativeOnPenClear();

    /** MotionEvent を観測する。イベントは消費しない（呼び出し側が super の戻り値を返す）。 */
    private void onTouch(MotionEvent ev) {
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
    private void onPause() {
        release();
    }

    /** Activity 破棄で接触なしに戻す。clear は冪等で値を消す方向にしか働かない。 */
    private void onDestroy() {
        release();
    }
}
