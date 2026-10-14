package com.fandhe.androidapp;

import android.app.Activity;
import android.os.Build;
import android.view.View;
import android.view.Window;

import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

// 全画面表示（システムバー非表示、Issue #1314、親 #1133）の Android 実装ヘルパー。
// MainActivity から生成・委譲される。Rust の `device/android_fullscreen_immersive.rs` が
// egui スレッドから `MainActivity.fullscreenSet` を JNI で同期呼び出しし、現在の表示状態は
// 既存の insets リスナー経由の `onInsets` が native へ push する（Rust は atomic を読むだけ）。
//
// `WindowInsetsControllerCompat.hide/show` は UI スレッドで呼ぶため、`set` は `runOnUiThread`
// へ渡して最長 APPLY_TIMEOUT_MS だけ待つ。期限は GameActivity が `onPause` 等で UI スレッドを
// ネイティブ側の完了待ちにする際の相互待ちを必ず抜けるためのもの（ANR の 5 秒より短い）。
// 呼び出しごとに `Call` を作り、UI 側の適用開始と呼び出し側の放棄を CAS で排他にする
// （`ScreenWakeLockHelper` と同形。放棄が勝てば Window に触れず RESULT_TIMEOUT）。
// RESULT_OK は「hide/show を呼び終えた」だけを意味し、全画面になったかは読み戻しで確定する。
//
// ライフサイクル: `onDestroy`（Activity 再生成を含む）で `destroyed` にし、状態を「読めない」
// (-1) として native へ送る。新しい Window はバー表示から始まり、要求は持ち越さない。
// 背景遷移では何もしない（バーの戻りは insets 経由で表示が追従する）。
final class FullscreenHelper {
    // 結果コード。SSOT はここで、Rust 側 `android_fullscreen_immersive.rs::decode` が同じ値で写像する。
    static final int RESULT_OK = 0;
    static final int RESULT_TIMEOUT = 1;
    static final int RESULT_FAILED = 2;

    // native へ送る状態。-1 は「読めない」。
    static final int STATE_UNKNOWN = -1;
    static final int STATE_NORMAL = 0;
    static final int STATE_FULLSCREEN = 1;

    private static final long APPLY_TIMEOUT_MS = 1000;

    private static final int PENDING = 0;
    private static final int STARTED = 1;
    private static final int ABANDONED = 2;

    // 実体は Rust `Java_com_fandhe_androidapp_FullscreenHelper_nativeOnFullscreenChanged`。
    static native void nativeOnFullscreenChanged(int state);

    // 現行インスタンス。旧インスタンスの `onDestroy` が新インスタンスの報告を潰さないために使う。
    private static volatile FullscreenHelper current;

    private static final class Call {
        final AtomicInteger state = new AtomicInteger(PENDING);
        final CountDownLatch finished = new CountDownLatch(1);
        volatile int code = RESULT_FAILED;
    }

    private final Activity activity;
    private volatile boolean destroyed;
    private int lastState = STATE_UNKNOWN; // UI スレッドのみ

    FullscreenHelper(Activity activity) {
        this.activity = activity;
    }

    // `onCreate`（UI スレッド）から呼ぶ。
    void register() {
        current = this;
    }

    // egui スレッドから呼ぶ。UI スレッドで hide/show を呼び、その結果コードを返す。
    int set(final boolean on) {
        final Call call = new Call();
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (call.state.compareAndSet(PENDING, STARTED)) {
                    try {
                        call.code = apply(on);
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

    // UI スレッドから呼ぶ。
    private int apply(boolean on) {
        if (destroyed || activity.isDestroyed()) {
            return RESULT_FAILED;
        }
        try {
            Window window = activity.getWindow();
            if (window == null) {
                return RESULT_FAILED;
            }
            View decor = window.getDecorView();
            WindowInsetsControllerCompat controller = WindowCompat.getInsetsController(window, decor);
            if (controller == null) {
                return RESULT_FAILED;
            }
            if (on) {
                controller.setSystemBarsBehavior(
                        WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
                controller.hide(WindowInsetsCompat.Type.systemBars());
            } else {
                controller.show(WindowInsetsCompat.Type.systemBars());
            }
            return RESULT_OK;
        } catch (RuntimeException e) {
            return RESULT_FAILED;
        }
    }

    // MainActivity の既存 insets リスナー（UI スレッド）から呼ぶ。変化したときだけ native へ送る。
    // API 30+ は statusBars / navigationBars を個別に見る（`systemBars()` は全種が見えるときだけ true）。
    // API 30 未満は decor の systemUiVisibility フラグで判定する。
    void onInsets(WindowInsetsCompat insets) {
        // 現行インスタンス以外（破棄中・差し替え済み）の報告は新 Window の状態を潰すので捨てる。
        if (destroyed || current != this) {
            return;
        }
        boolean hidden;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            hidden = !insets.isVisible(WindowInsetsCompat.Type.statusBars())
                    && !insets.isVisible(WindowInsetsCompat.Type.navigationBars());
        } else {
            // API 24-29 は isVisible が statusBars / navigationBars に対し true のままになる。
            // compat の hide は SYSTEM_UI_FLAG_FULLSCREEN / HIDE_NAVIGATION を立てるので、そのフラグで読む。
            // スワイプの一時表示ではシステムがフラグを落とすため、バー再表示にも追従する。
            int flags = activity.getWindow().getDecorView().getWindowSystemUiVisibility();
            int both = View.SYSTEM_UI_FLAG_FULLSCREEN | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION;
            hidden = (flags & both) == both;
        }
        int state = hidden ? STATE_FULLSCREEN : STATE_NORMAL;
        if (state != lastState) {
            lastState = state;
            nativeOnFullscreenChanged(state);
        }
    }

    // `MainActivity.onDestroy`（UI スレッド）から呼ぶ。以後に届く旧要求は Window に触れない。
    void onDestroy() {
        destroyed = true;
        if (current == this) {
            current = null;
            nativeOnFullscreenChanged(STATE_UNKNOWN);
        }
    }
}
