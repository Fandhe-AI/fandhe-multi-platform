package com.fandhe.platform;

import android.app.Activity;
import android.view.Window;
import android.view.WindowManager;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

// 画面スリープ抑止（Issue #1280、親 #1129、#1187）の Android 実装ヘルパー。利用者の `MainActivity` には
// 何も書かせない。Rust の `src/android.rs` が egui スレッドから static の `set(Activity, keep)` /
// `isHeld(Activity)` を JNI で同期呼び出しする。Activity への参加は `ScreenWakeLockLifecycle`
// （非公開 ContentProvider、`FeatureLifecycleProvider` 継承）が行い、生成時に `attach`、破棄時に `detach` を呼ぶ。
// Activity ごとの helper は `HELPERS` に持つ（egui スレッドと UI スレッドの両方から触るので同期化し、
// 破棄時に必ず外す）。表に無い Activity への要求は RESULT_FAILED / false を返す（嘘の成功にしない）。
//
//
// `Window.addFlags/clearFlags(FLAG_KEEP_SCREEN_ON)` は UI スレッドで呼ぶ必要があるため、
// `set` は `runOnUiThread` へ渡して最長 APPLY_TIMEOUT_MS だけ待つ。Rust 側 `drive` は
// `set_keep_awake` の直後に `is_held` を読むので、戻る時点で反映と読み戻しを終えている。
// 期限は GameActivity が `onPause` 等で UI スレッドをネイティブ側の完了待ちにする際の
// 相互待ちを必ず抜けるためのもの（ANR の 5 秒より短い）。
//
// 呼び出しごとに `Call` を作り、UI 側の適用開始と呼び出し側の放棄を CAS で排他にする。
// 放棄が勝てば UI 側は Window にも `held` にも触れず RESULT_TIMEOUT（期限切れ後に適用しない）、
// 適用開始が勝てば終了を待って実結果を返す（失敗報告と実状態を食い違わせない）。
// `held` は UI スレッドで読み戻したフラグの値。
//
// ライフサイクル: `onDestroy`（Activity 再生成を含む）で `destroyed` にして `held` を
// 落とす。新しい Activity の Window にはフラグが無く、Rust 側の画面も未要求から作り直される
// ため、要求は持ち越さない。背景（`onPause`/`onStop`）ではフラグを残す（見えない間は OS が
// 効かせず、前面復帰で再び効く。守らない範囲）。
final class ScreenWakeLockHelper {
    // 結果コード。SSOT はここで、Rust 側 `android.rs::decode` が同じ値で写像する。
    static final int RESULT_OK = 0;
    static final int RESULT_TIMEOUT = 1;
    static final int RESULT_FAILED = 2;

    private static final long APPLY_TIMEOUT_MS = 1000;

    private static final int PENDING = 0;
    private static final int STARTED = 1;
    private static final int ABANDONED = 2;

    private static final class Call {
        // 未開始 -> 適用開始（UI 側）/ 放棄（呼び出し側）を CAS で決める。勝った側だけが進める。
        final AtomicInteger state = new AtomicInteger(PENDING);
        final CountDownLatch finished = new CountDownLatch(1);
        volatile int code = RESULT_FAILED;
    }

    // Activity ごとの helper（上記クラス冒頭コメント参照）。
    private static final Map<Activity, ScreenWakeLockHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private final Activity activity;
    private final Object lock = new Object();
    private boolean held;
    private boolean destroyed;

    private ScreenWakeLockHelper(Activity activity) {
        this.activity = activity;
    }

    // Activity の生成時（`ScreenWakeLockLifecycle` から）。
    static void attach(Activity activity) {
        HELPERS.put(activity, new ScreenWakeLockHelper(activity));
    }

    // Activity の破棄時（`ScreenWakeLockLifecycle` から）。以後に届く旧要求は Window に触れない。
    static void detach(Activity activity) {
        ScreenWakeLockHelper helper = HELPERS.remove(activity);
        if (helper != null) {
            helper.onDestroy();
        }
    }

    // egui スレッドから呼ぶ入口。helper が無い（生成前・破棄後）なら失敗を返す。
    static int set(Activity activity, boolean keep) {
        ScreenWakeLockHelper helper = HELPERS.get(activity);
        return helper == null ? RESULT_FAILED : helper.setKeepAwake(keep);
    }

    static boolean isHeld(Activity activity) {
        ScreenWakeLockHelper helper = HELPERS.get(activity);
        return helper != null && helper.held();
    }

    // egui スレッドから呼ぶ。UI スレッドで適用し、読み戻した結果を返す。
    // RESULT_TIMEOUT を返すのは「未開始のまま放棄できた」ときだけで、以後 UI 側は Window に触れない
    // （FLAG_KEEP_SCREEN_ON も held も変わらない）。適用開始済みなら終了を待ち実結果を返す。
    private int setKeepAwake(final boolean keep) {
        final Call call = new Call();
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (call.state.compareAndSet(PENDING, STARTED)) {
                    try {
                        synchronized (lock) {
                            call.code = apply(keep);
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
            // 適用開始済み（UI スレッドで実行中）。取り消せないので終了まで待ち、実結果を返す。
            // 待たずに失敗を返すと、返却後に addFlags が走り失敗報告と実状態が食い違う。
            // 実行中の UI 側は短い Window 操作と `lock` だけで、egui スレッドの完了待ちを含まない。
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

    // `lock` を保持した UI スレッドから呼ぶ。
    private int apply(boolean keep) {
        if (destroyed || activity.isDestroyed()) {
            held = false;
            return RESULT_FAILED;
        }
        try {
            Window window = activity.getWindow();
            if (window == null) {
                held = false;
                return RESULT_FAILED;
            }
            if (keep) {
                window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            } else {
                window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            }
            held = (window.getAttributes().flags
                    & WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) != 0;
            return held == keep ? RESULT_OK : RESULT_FAILED;
        } catch (RuntimeException e) {
            // 更新中の例外でも `held` を実フラグへ再同期する（古い値を返し続けない）。
            // 再読込も失敗する場合は実状態が不明なので false に倒す（嘘の成功を返さない）。
            try {
                held = (activity.getWindow().getAttributes().flags
                        & WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) != 0;
            } catch (RuntimeException e2) {
                held = false;
            }
            return RESULT_FAILED;
        }
    }

    private boolean held() {
        synchronized (lock) {
            return held;
        }
    }

    // `detach`（UI スレッド）から呼ぶ。以後に届く旧要求は Window に触れない。
    private void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            held = false;
        }
    }
}
