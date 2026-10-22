package com.fandhe.androidapp;

import android.annotation.TargetApi;
import android.app.Activity;
import android.content.pm.PackageManager;
import android.os.Build;
import android.view.Window;
import android.view.WindowManager;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.Consumer;

// 画面キャプチャの防止・検知（Issue #584、親 #120）の Android 実装ヘルパー。MainActivity から
// 生成・委譲され、Rust の `device/android_capture_protection.rs` が egui スレッドから
// `MainActivity.captureProtection*` を JNI で呼ぶ（`device/capture_protection.rs` のドライバが
// 要求の直後と 1 秒間隔で読み戻す pull 型。Java から Rust への push は作らない）。
//
// 防止: `FLAG_SECURE` を Window に付け外しする（全 API レベル。最近使ったアプリの画面にも効く）。
// `Window.addFlags/clearFlags` は UI スレッド限定なので、`set` は `runOnUiThread` へ渡して最長
// APPLY_TIMEOUT_MS だけ待つ（GameActivity の `onPause` との相互待ちを抜けるため）。呼び出しごとに
// `Call` を作り、UI 側の適用開始と呼び出し側の放棄を CAS で排他にする。放棄が勝てば UI 側は
// Window にも状態にも触れず RESULT_TIMEOUT（期限切れ後に適用しない）、適用開始が勝てば終了を
// 待って実結果を返す。表示用の状態は UI スレッドで Window 属性から読み戻した値だけを使う。
//
// 検知: スクリーンショットは `Activity.registerScreenCaptureCallback`（API 34+、DETECT_SCREEN_CAPTURE）、
// 録画は `WindowManager.addScreenRecordingCallback`（API 35+、DETECT_SCREEN_RECORDING）。
// API が無い・権限が許可されていない・登録に失敗したときは「読めない」（-1）を返し、成功値には
// しない。API 固有の型は Api34Impl / Api35Impl に閉じ、古い端末のクラス検証を避ける。
// 検知した事実・回数・例外の内容はログに出さない。
//
// ライフサイクル: Activity 再生成では保留中の `Call` を `destroyed` で破棄し（旧要求は新しい
// Window に触れない）、保護の要求値 `requested` は static で持ち越して新インスタンスの `onCreate`
// で掛け直す（親の決定事項 1）。検知コールバックは `onStart` で登録し `onStop` で解除する。登録は
// main executor で届き `onStart` も main で走るため、登録から初期値の保存までの間に通知が
// 割り込まない。解除後に main queue に残っていた旧通知は世代（`recordingEpoch`）で捨てる。
// 背景にいる間の録画状態は「読めない」とし、最後の値を返し続けない。スクショ回数は
// プロセス内で単調増加させ、再生成でも 0 に戻さない。
//
// 守る範囲: 1 プロセス内の順序・失敗を成功と見せないこと・期限後に適用しないこと・登録と解除の対応。
// 守らない範囲: 背景中のスクリーンショット（登録外）、adb・計装のスクリーンショット、保護中に
// スクショ通知が届くかは OS 依存（実機確認 #575 で記録）、プロセス終了後は要求を持ち越さない、
// 外部カメラでの撮影、`FLAG_SECURE` を掛ける前に始まっていた録画。
final class CaptureProtectionHelper {
    // 結果コード。SSOT はここで、Rust 側 `android_capture_protection.rs` が同じ値で写像する。
    static final int RESULT_OK = 0;
    static final int RESULT_TIMEOUT = 1;
    static final int RESULT_FAILED = 2;

    // 状態値。STATE_UNKNOWN は「読めない」で、値域（0/1・回数 >= 0）と衝突しない。
    static final int STATE_UNKNOWN = -1;
    static final int STATE_OFF = 0;
    static final int STATE_ON = 1;

    // 対応状況のビット。
    static final int SUPPORT_PREVENT = 1;
    static final int SUPPORT_RECORDING = 2;
    static final int SUPPORT_SCREENSHOT = 4;

    private static final long APPLY_TIMEOUT_MS = 1000;

    private static final int PENDING = 0;
    private static final int STARTED = 1;
    private static final int ABANDONED = 2;

    // 保護の要求値と各インスタンスの状態を守るロック。要求値は再生成をまたいで残す。
    private static final Object LOCK = new Object();
    private static boolean requested;
    // スクリーンショットの検知回数。プロセス内で単調増加。
    private static final AtomicLong SCREENSHOT_COUNT = new AtomicLong();

    private static final class Call {
        final AtomicInteger state = new AtomicInteger(PENDING);
        final CountDownLatch finished = new CountDownLatch(1);
        volatile int code = RESULT_FAILED;
    }

    private final Activity activity;
    // 以下は `LOCK` の内側で読み書きする。
    private int protectedState = STATE_UNKNOWN;
    private boolean destroyed;
    private int recordingState = STATE_UNKNOWN;
    private int recordingEpoch;
    private Object recordingCallback;
    private Object screenshotCallback;
    private boolean screenshotRegistered;

    CaptureProtectionHelper(Activity activity) {
        this.activity = activity;
    }

    // ---- 防止 ----

    // egui スレッドから呼ぶ。UI スレッドで適用し、結果コードを返す。
    // RESULT_TIMEOUT は「未開始のまま放棄できた」ときだけで、以後 UI 側は Window に触れない。
    int set(final boolean on) {
        final Call call = new Call();
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (call.state.compareAndSet(PENDING, STARTED)) {
                    try {
                        synchronized (LOCK) {
                            call.code = apply(on);
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
            // 適用開始済みは取り消せない。終了を待って実結果を返し、失敗報告と実状態を食い違わせない。
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
    private int apply(boolean on) {
        if (destroyed || activity.isDestroyed()) {
            // 旧 Activity への要求。`requested` は新 Activity 側の状態なので変えない。
            protectedState = STATE_UNKNOWN;
            return RESULT_FAILED;
        }
        try {
            Window window = activity.getWindow();
            if (window == null) {
                protectedState = STATE_UNKNOWN;
                return RESULT_FAILED;
            }
            if (on) {
                window.addFlags(WindowManager.LayoutParams.FLAG_SECURE);
            } else {
                window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE);
            }
            protectedState = readFlag(window);
            if (protectedState == (on ? STATE_ON : STATE_OFF)) {
                requested = on;
                return RESULT_OK;
            }
            return RESULT_FAILED;
        } catch (RuntimeException e) {
            // 更新中の例外でも実フラグへ再同期する。読めなければ「読めない」に倒す。
            protectedState = rereadState();
            return RESULT_FAILED;
        }
    }

    private int rereadState() {
        try {
            return readFlag(activity.getWindow());
        } catch (RuntimeException e) {
            return STATE_UNKNOWN;
        }
    }

    private static int readFlag(Window window) {
        if (window == null) {
            return STATE_UNKNOWN;
        }
        boolean secure =
                (window.getAttributes().flags & WindowManager.LayoutParams.FLAG_SECURE) != 0;
        return secure ? STATE_ON : STATE_OFF;
    }

    // UI スレッドで読み戻してキャッシュした値。
    int isProtected() {
        synchronized (LOCK) {
            return protectedState;
        }
    }

    // ---- 対応状況・検知 ----

    int support() {
        int bits = SUPPORT_PREVENT;
        if (screenshotUsable()) {
            bits |= SUPPORT_SCREENSHOT;
        }
        if (recordingUsable()) {
            bits |= SUPPORT_RECORDING;
        }
        return bits;
    }

    int recording() {
        synchronized (LOCK) {
            return recordingState;
        }
    }

    long screenshotCount() {
        synchronized (LOCK) {
            return screenshotRegistered ? SCREENSHOT_COUNT.get() : -1L;
        }
    }

    private boolean screenshotUsable() {
        return Build.VERSION.SDK_INT >= 34
                && granted("android.permission.DETECT_SCREEN_CAPTURE");
    }

    private boolean recordingUsable() {
        return Build.VERSION.SDK_INT >= 35
                && granted("android.permission.DETECT_SCREEN_RECORDING");
    }

    private boolean granted(String permission) {
        try {
            return activity.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED;
        } catch (RuntimeException e) {
            return false;
        }
    }

    // ---- ライフサイクル（すべて UI スレッド） ----

    // `MainActivity.onCreate` の `super.onCreate` 直後。最初のフレームと Recents のサムネイル前に
    // 要求値を掛け直す。
    void onCreate() {
        synchronized (LOCK) {
            try {
                Window window = activity.getWindow();
                if (window != null && requested) {
                    window.addFlags(WindowManager.LayoutParams.FLAG_SECURE);
                }
                protectedState = readFlag(window);
            } catch (RuntimeException e) {
                protectedState = rereadState();
            }
        }
    }

    void onStart() {
        synchronized (LOCK) {
            if (destroyed) {
                return;
            }
            if (!screenshotRegistered && screenshotUsable()) {
                try {
                    screenshotCallback = Api34Impl.register(activity, SCREENSHOT_COUNT);
                    screenshotRegistered = true;
                } catch (RuntimeException e) {
                    screenshotCallback = null;
                    screenshotRegistered = false;
                }
            }
            if (recordingCallback == null && recordingUsable()) {
                recordingEpoch++;
                final int epoch = recordingEpoch;
                try {
                    Consumer<Integer> consumer = new Consumer<Integer>() {
                        @Override
                        public void accept(Integer value) {
                            synchronized (LOCK) {
                                if (epoch == recordingEpoch && recordingCallback != null) {
                                    recordingState = Api35Impl.toState(value);
                                }
                            }
                        }
                    };
                    int initial = Api35Impl.register(activity, consumer);
                    recordingCallback = consumer;
                    recordingState = Api35Impl.toState(initial);
                } catch (RuntimeException e) {
                    recordingCallback = null;
                    recordingState = STATE_UNKNOWN;
                }
            }
        }
    }

    void onStop() {
        synchronized (LOCK) {
            unregisterAll();
        }
    }

    // 再生成を含む破棄。旧要求は以後 Window に触れない。`requested` は次の Activity へ残す。
    void onDestroy() {
        synchronized (LOCK) {
            destroyed = true;
            protectedState = STATE_UNKNOWN;
            unregisterAll();
        }
    }

    // `LOCK` を保持して呼ぶ。登録と解除を対応させ、世代を進めて旧通知を無効にする。
    private void unregisterAll() {
        recordingEpoch++;
        if (recordingCallback != null) {
            try {
                Api35Impl.unregister(activity, recordingCallback);
            } catch (RuntimeException e) {
                // 解除済み・Window 不在。世代で無効化済みなので残さない。
            }
            recordingCallback = null;
        }
        recordingState = STATE_UNKNOWN;
        if (screenshotCallback != null) {
            try {
                Api34Impl.unregister(activity, screenshotCallback);
            } catch (RuntimeException e) {
                // 同上。
            }
            screenshotCallback = null;
        }
        screenshotRegistered = false;
    }

    // API 34 固有の型を閉じ込める（古い端末でクラスが検証されないようにする）。
    @TargetApi(34)
    private static final class Api34Impl {
        static Object register(Activity activity, final AtomicLong counter) {
            Activity.ScreenCaptureCallback cb = new Activity.ScreenCaptureCallback() {
                @Override
                public void onScreenCaptured() {
                    counter.incrementAndGet();
                }
            };
            activity.registerScreenCaptureCallback(activity.getMainExecutor(), cb);
            return cb;
        }

        static void unregister(Activity activity, Object cb) {
            activity.unregisterScreenCaptureCallback((Activity.ScreenCaptureCallback) cb);
        }
    }

    // API 35 固有の型を閉じ込める。
    @TargetApi(35)
    private static final class Api35Impl {
        static int register(Activity activity, Consumer<Integer> consumer) {
            return activity.getWindowManager()
                    .addScreenRecordingCallback(activity.getMainExecutor(), consumer);
        }

        @SuppressWarnings("unchecked")
        static void unregister(Activity activity, Object consumer) {
            activity.getWindowManager().removeScreenRecordingCallback((Consumer<Integer>) consumer);
        }

        // SCREEN_RECORDING_STATE_VISIBLE = 1 / NOT_VISIBLE = 0。それ以外は「読めない」。
        static int toState(Integer value) {
            return value == null ? STATE_UNKNOWN : toState(value.intValue());
        }

        static int toState(int value) {
            if (value == WindowManager.SCREEN_RECORDING_STATE_VISIBLE) {
                return STATE_ON;
            }
            if (value == WindowManager.SCREEN_RECORDING_STATE_NOT_VISIBLE) {
                return STATE_OFF;
            }
            return STATE_UNKNOWN;
        }
    }
}
