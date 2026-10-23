package com.fandhe.androidapp;

import android.app.Activity;
import android.content.pm.ActivityInfo;
import android.os.Build;
import android.view.Display;
import android.view.Surface;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

// 画面向きの取得・固定（Issue #657、親 #128）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_screen_orientation.rs` が egui スレッドから
// `MainActivity.screenOrientationRequest` / `screenOrientationRead` を JNI で同期呼び出しする。
// Java から Rust への push は無い）。
//
// `setRequestedOrientation` は UI スレッドで呼ぶため、`request` は `runOnUiThread` へ渡して最長
// APPLY_TIMEOUT_MS だけ待つ。期限は GameActivity が `onPause` 等で UI スレッドをネイティブ側の
// 完了待ちにする際の相互待ちを必ず抜けるためのもの（ANR の 5 秒より短く、Rust driver の確認期限
// 3 秒より短い）。呼び出しごとに `Call` を作り、UI 側の適用開始と呼び出し側の放棄を CAS で排他に
// する。放棄が勝てば UI 側は OS に触れず RESULT_TIMEOUT（期限後に適用しない）、適用開始が勝てば
// 終了を待って実結果を返す。
//
// RESULT_OK は「OS 側の希望を設定し、読み戻して一致した」ことだけを表す。実際の向きの一致は
// Rust driver が `read` で確かめる（嘘の成功を作らない）。不一致・例外時は直前の希望へ戻す。
//
// `read` は呼び出しスレッドで直接読む（Display の回転・Configuration・希望の読み戻しで、
// Window は書き換えない）。
//
// ライフサイクル: 呼び出しごとの結果を同期で返すため、旧 Activity の結果が新しい要求を消費する
// 経路は無い。`onDestroy` 以降に UI キューで走る旧要求は OS に触れず RESULT_FAILED。固定は
// 再生成へ持ち越さない。背景（`onPause`/`onStop`）では何もしない（守らない範囲）。
//
// コード表の SSOT はここで、Rust 側 `android_screen_orientation.rs` が同じ値で写像する。
// lock: 0 = 解除 / 1 = 縦 / 2 = 横。read: int[3] = {orientation(1 縦・2 横), degrees, lock}。
final class ScreenOrientationHelper {
    static final int RESULT_OK = 0;
    static final int RESULT_TIMEOUT = 1;
    static final int RESULT_FAILED = 2;
    static final int RESULT_INVALID = 3;

    static final int LOCK_NONE = 0;
    static final int LOCK_PORTRAIT = 1;
    static final int LOCK_LANDSCAPE = 2;
    static final int LOCK_UNKNOWN = -1;
    static final int DEGREES_UNKNOWN = -1;

    private static final long APPLY_TIMEOUT_MS = 1000;

    private static final int PENDING = 0;
    private static final int STARTED = 1;
    private static final int ABANDONED = 2;

    // 旧・新インスタンス間で適用と破棄を直列化する。
    private static final Object LOCK = new Object();

    private static final class Call {
        final AtomicInteger state = new AtomicInteger(PENDING);
        final CountDownLatch finished = new CountDownLatch(1);
        volatile int code = RESULT_FAILED;
    }

    private final Activity activity;
    private boolean destroyed;

    ScreenOrientationHelper(Activity activity) {
        this.activity = activity;
    }

    int request(int lock) {
        final int target;
        switch (lock) {
            case LOCK_NONE:
                target = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED;
                break;
            case LOCK_PORTRAIT:
                target = ActivityInfo.SCREEN_ORIENTATION_SENSOR_PORTRAIT;
                break;
            case LOCK_LANDSCAPE:
                target = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE;
                break;
            default:
                return RESULT_INVALID;
        }
        final Call call = new Call();
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (call.state.compareAndSet(PENDING, STARTED)) {
                    try {
                        synchronized (LOCK) {
                            call.code = apply(target);
                        }
                    } finally {
                        call.finished.countDown();
                    }
                }
            }
        });
        boolean interrupted = false;
        try {
            try {
                if (call.finished.await(APPLY_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
                    return call.code;
                }
            } catch (InterruptedException e) {
                interrupted = true;
            }
            if (call.state.compareAndSet(PENDING, ABANDONED)) {
                return RESULT_TIMEOUT;
            }
            // 適用開始済み。取り消せないので終了まで待ち、実結果を返す。
            while (true) {
                try {
                    call.finished.await();
                    return call.code;
                } catch (InterruptedException e) {
                    interrupted = true;
                }
            }
        } finally {
            if (interrupted) {
                Thread.currentThread().interrupt();
            }
        }
    }

    // `LOCK` を保持した UI スレッドから呼ぶ。
    private int apply(int target) {
        if (destroyed || activity.isDestroyed()) {
            return RESULT_FAILED;
        }
        int before;
        try {
            before = activity.getRequestedOrientation();
        } catch (RuntimeException e) {
            return RESULT_FAILED;
        }
        try {
            activity.setRequestedOrientation(target);
            if (activity.getRequestedOrientation() == target) {
                return RESULT_OK;
            }
            // 契約: 失敗なら OS 側の希望は変えない。
            activity.setRequestedOrientation(before);
            return RESULT_FAILED;
        } catch (RuntimeException e) {
            // API 26 の半透明 Activity などで投げられる。取得済みの before へ復元を試みる。
            try {
                activity.setRequestedOrientation(before);
            } catch (RuntimeException e2) {
                // 復元できなくても成功には倒さない。
            }
            return RESULT_FAILED;
        }
    }

    // egui スレッドから呼ぶ。読めないときは null。
    int[] read() {
        try {
            synchronized (LOCK) {
                if (destroyed || activity.isDestroyed()) {
                    return null;
                }
            }
            Display display;
            if (Build.VERSION.SDK_INT >= 30) {
                display = activity.getDisplay();
            } else {
                display = activity.getWindowManager().getDefaultDisplay();
            }
            if (display == null) {
                return null;
            }
            int degrees;
            switch (display.getRotation()) {
                case Surface.ROTATION_0:
                    degrees = 0;
                    break;
                case Surface.ROTATION_90:
                    degrees = 90;
                    break;
                case Surface.ROTATION_180:
                    degrees = 180;
                    break;
                case Surface.ROTATION_270:
                    degrees = 270;
                    break;
                default:
                    degrees = DEGREES_UNKNOWN;
                    break;
            }
            int orientation = activity.getResources().getConfiguration().orientation;
            int lock;
            switch (activity.getRequestedOrientation()) {
                case ActivityInfo.SCREEN_ORIENTATION_SENSOR_PORTRAIT:
                    lock = LOCK_PORTRAIT;
                    break;
                case ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE:
                    lock = LOCK_LANDSCAPE;
                    break;
                case ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED:
                    lock = LOCK_NONE;
                    break;
                default:
                    lock = LOCK_UNKNOWN;
                    break;
            }
            // Configuration.ORIENTATION_PORTRAIT=1 / LANDSCAPE=2 はそのまま渡す。他は Rust が拒否する。
            return new int[] {orientation, degrees, lock};
        } catch (RuntimeException e) {
            return null;
        }
    }

    // `MainActivity.onDestroy`（UI スレッド）から呼ぶ。以後に届く旧要求は OS に触れない。
    void onDestroy() {
        synchronized (LOCK) {
            destroyed = true;
        }
    }
}
