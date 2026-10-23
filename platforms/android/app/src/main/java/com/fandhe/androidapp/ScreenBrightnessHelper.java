package com.fandhe.androidapp;

import android.app.Activity;
import android.view.Window;
import android.view.WindowManager;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

// 画面輝度の上書き（Issue #635、親 #126）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_screen_brightness.rs` が egui スレッドから
// `MainActivity.screenBrightnessSet` / `screenBrightnessClear` / `screenBrightnessRead` を
// JNI で同期呼び出しする。Java から Rust への push は無い）。
//
// `Window.setAttributes` は UI スレッドで呼ぶ必要があるため、set / clear は `runOnUiThread` へ
// 渡して最長 APPLY_TIMEOUT_MS だけ待つ。期限は GameActivity が `onPause` 等で UI スレッドを
// ネイティブ側の完了待ちにする際の相互待ちを必ず抜けるためのもの（ANR の 5 秒より短い）。
// 呼び出しごとに `Call` を作り、UI 側の適用開始と呼び出し側の放棄を CAS で排他にする。
// 放棄が勝てば UI 側は Window にも `requested` にも触れず RESULT_TIMEOUT（期限後に適用しない）、
// 適用開始が勝てば終了を待って実結果を返す。egui スレッドは `lock` を持ったまま待たない。
//
// 成否は `getAttributes().screenBrightness` の読み戻しで決める。`read` は UI スレッドが更新した
// `cached` を返し、egui スレッドから Window を直接読まない。
//
// ライフサイクル: 要求値 `requested` は static で持ち越し、`onCreate` で新しい Window へ掛け直す
// （設定変更による再生成）。実終了（isFinishing かつ設定変更でない）では捨てる。旧 Activity の
// 保留中の要求は `destroyed` で Window に触れない。背景（`onPause`/`onStop`）では何もしない
// （OS が利用者の値で表示し、前面復帰で再び効く。守らない範囲）。
//
// 結果コード・番兵値の SSOT はここで、Rust 側 `android_screen_brightness.rs` が同じ値で写像する。
final class ScreenBrightnessHelper {
    static final int RESULT_OK = 0;
    static final int RESULT_TIMEOUT = 1;
    static final int RESULT_FAILED = 2;
    static final int RESULT_INVALID = 3;

    // 読み取り値の番兵。値域 0〜1 と衝突しない。
    static final float READ_NONE = -1.0f;
    static final float READ_UNKNOWN = -2.0f;

    private static final long APPLY_TIMEOUT_MS = 1000;
    private static final float TOLERANCE = 1e-4f;

    private static final int PENDING = 0;
    private static final int STARTED = 1;
    private static final int ABANDONED = 2;

    // 再生成をまたぐ要求値。READ_NONE は未要求。`LOCK` の内側でのみ触る。
    private static final Object LOCK = new Object();
    private static float requested = READ_NONE;

    private static final class Call {
        final AtomicInteger state = new AtomicInteger(PENDING);
        final CountDownLatch finished = new CountDownLatch(1);
        volatile int code = RESULT_FAILED;
    }

    private final Activity activity;
    private boolean destroyed;
    private float cached = READ_UNKNOWN;

    ScreenBrightnessHelper(Activity activity) {
        this.activity = activity;
    }

    int set(float level) {
        if (Float.isNaN(level) || Float.isInfinite(level) || level < 0.0f || level > 1.0f) {
            return RESULT_INVALID;
        }
        return submit(level);
    }

    int clear() {
        return submit(WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE);
    }

    private int submit(final float target) {
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
            // 適用開始済み。取り消せないので終了まで待ち、実結果を返す（失敗報告と実状態を食い違わせない）。
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

    // `LOCK` を保持した UI スレッドから呼ぶ。target < 0 は解除。
    private int apply(float target) {
        if (destroyed || activity.isDestroyed()) {
            cached = READ_UNKNOWN;
            return RESULT_FAILED;
        }
        Window window = null;
        float before = READ_UNKNOWN;
        try {
            window = activity.getWindow();
            if (window == null) {
                cached = READ_UNKNOWN;
                return RESULT_FAILED;
            }
            before = window.getAttributes().screenBrightness;
            writeBrightness(window, target);
            float actual = window.getAttributes().screenBrightness;
            boolean ok = target < 0.0f ? actual < 0.0f : Math.abs(actual - target) <= TOLERANCE;
            if (ok) {
                requested = target < 0.0f ? READ_NONE : target;
                cached = normalize(actual);
                return RESULT_OK;
            }
            // 失敗時は直前の状態のまま、という契約。戻せたかは読み戻しで確定する。
            writeBrightness(window, before);
            cached = normalize(window.getAttributes().screenBrightness);
            return RESULT_FAILED;
        } catch (RuntimeException e) {
            // 属性は途中まで書き換わっている可能性がある。取得済みの Window と before で復元を試み、
            // 読み戻しで確定する。戻せたか確認できなければ不明にして、成功には倒さない。
            cached = READ_UNKNOWN;
            if (window != null && before != READ_UNKNOWN) {
                try {
                    writeBrightness(window, before);
                } catch (RuntimeException e2) {
                    // 復元の書き込みに失敗しても、下の読み戻しで実値を確かめる。
                }
                try {
                    float actual = window.getAttributes().screenBrightness;
                    boolean restored = before < 0.0f ? actual < 0.0f : Math.abs(actual - before) <= TOLERANCE;
                    if (restored) {
                        cached = normalize(actual);
                    }
                } catch (RuntimeException e3) {
                    cached = READ_UNKNOWN;
                }
            }
            return RESULT_FAILED;
        }
    }

    private static void writeBrightness(Window window, float value) {
        WindowManager.LayoutParams lp = window.getAttributes();
        lp.screenBrightness = value;
        window.setAttributes(lp);
    }

    private static float normalize(float v) {
        return v < 0.0f ? READ_NONE : v;
    }

    // egui スレッドから呼ぶ。UI スレッドが更新した値を返す。
    float read() {
        synchronized (LOCK) {
            return cached;
        }
    }

    // `MainActivity.onCreate`（UI スレッド）で `super.onCreate` の直後に呼ぶ。持ち越した要求を掛け直す。
    void onCreate() {
        synchronized (LOCK) {
            try {
                Window window = activity.getWindow();
                if (window == null) {
                    cached = READ_UNKNOWN;
                    return;
                }
                if (requested != READ_NONE) {
                    writeBrightness(window, requested);
                }
                cached = normalize(window.getAttributes().screenBrightness);
            } catch (RuntimeException e) {
                cached = READ_UNKNOWN;
            }
        }
    }

    // `MainActivity.onDestroy`（UI スレッド）から呼ぶ。以後に届く旧要求は Window に触れない。
    void onDestroy() {
        synchronized (LOCK) {
            destroyed = true;
            cached = READ_UNKNOWN;
            if (activity.isFinishing() && !activity.isChangingConfigurations()) {
                requested = READ_NONE;
            }
        }
    }
}
