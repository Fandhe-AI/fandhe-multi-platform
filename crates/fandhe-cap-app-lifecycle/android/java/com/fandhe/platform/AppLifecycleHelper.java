package com.fandhe.platform;

import android.app.Activity;
import android.app.ActivityManager;

import androidx.activity.ComponentActivity;
import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// アプリの前面・背面の通知画面（Issue #895、親 #178。共通コアと macOS / Windows は #893）の Android 実装
// ヘルパー。利用者の Activity には何も書かせない（Rust の `fandhe-cap-app-lifecycle/src/android.rs` が static の
// `start(Activity, long)` / `stop(Activity, long)` / `read(Activity)` を JNI で呼び、出来事を `nativeOnAppLifecycle` で受ける）。
// Activity の生成・破棄・Started / Stopped / Resumed / Paused は `AppLifecycleLifecycle`（非公開 ContentProvider、
// `FeatureLifecycleProvider` 継承）が知らせる。Activity ごとの helper は `HELPERS` に持つ。helper が無い（生成前・破棄後）の
// start は失敗で確定し、結果が来ないまま待たせない。
// 知らせる種類は onResume（操作の対象になった）/ onPause（外れた）/ onStart（見えるようになった）/
// onStop（見えなくなった）の 4 つ。凍結・bfcache 復帰を知らせる API は無く、Rust 側が「取れない」と宣言する。
//
// コードの SSOT はここで、Rust 側 `android.rs` が `contract` の
// `event_from_apple_code` / `current_from_apple_bits` で同じ値を写像する。
//   出来事: 0 = Activated / 1 = Deactivated / 2 = Hidden / 3 = Shown
//   start の戻り値: 0 = 受け付けた / 1 = 失敗
//   read の戻り値: 負 = 失敗。bit 0-1 操作の対象か、bit 2-3 見えているか (0 いいえ / 1 はい / 2 不明)
//
// 今の状態は覚えず、read() が ActivityManager.getMyMemoryState の importance を毎回読む
// （FOREGROUND = 対象かつ見える / VISIBLE = 見えるが対象でない / それより低い = どちらでもない /
// 想定外の値 = 不明。前面へは畳まない）。権限・マニフェスト宣言・OS の資源は使わない。
//
// スレッドとロック: 購読 id（current）と destroyed は `lock` の内側だけで読み書きする。on* は main thread で
// 呼ばれ、native 呼び出しはロックの外で行う（古い id は Rust の登録表が捨てる）。ロック取得順は
// Java -> Rust の一方向。read は egui スレッドから同期で呼べる。
//
// ライフサイクル: onPause / onStop では購読を止めない（止めると背面の出来事を受けられない）。
// isChangingConfigurations() が真の間の onPause / onStop は、再生成であって前面・背面の変化ではないので
// 出来事にしない。onDestroy は購読を空にし、停止を失敗として送らない。新インスタンスは購読を持たない状態から
// 始まり、Rust 側の画面は Idle へ戻るため古い購読は drop される。id・例外の内容はログへ出さない。
// 守らない範囲: importance の更新が通知より遅れうること（OS 側の時間差）、マルチウィンドウでフォーカスが
// 無くても FOREGROUND と読まれうること、停止中の Activity の再生成、画面を開く前・止めた後の出来事。
final class AppLifecycleHelper {
    static final int START_OK = 0;
    static final int START_FAILED = 1;

    static final int EVENT_ACTIVATED = 0;
    static final int EVENT_DEACTIVATED = 1;
    static final int EVENT_HIDDEN = 2;
    static final int EVENT_SHOWN = 3;

    private static final long NONE = 0L;
    private static final int FIELD_UNKNOWN = 2;

    private static native void nativeOnAppLifecycle(long id, int code);

    // Activity ごとの helper。Rust のワーカーと UI スレッドの両方から触るので同期化し、Activity が GC されれば項目も消える。
    private static final Map<Activity, AppLifecycleHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private final ComponentActivity activity;
    private final Object lock = new Object();
    private long current = NONE;
    private boolean destroyed;

    private AppLifecycleHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // Activity の生成時（`AppLifecycleLifecycle` から、UI スレッド）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new AppLifecycleHelper(activity));
    }

    // Activity の破棄時（同上）。保持している資源を解放し、以後の開始を拒否する。
    static void detach(Activity activity) {
        AppLifecycleHelper helper = HELPERS.remove(activity);
        if (helper != null) {
            helper.onDestroy();
        }
    }

    // Rust から呼ぶ入口。helper が無い（生成前・破棄後）呼び出しは失敗で確定し、結果が来ないまま待たせない。
    static int start(Activity activity, long id) {
        AppLifecycleHelper helper = HELPERS.get(activity);
        return helper == null ? START_FAILED : helper.startInternal(id);
    }

    static void stop(Activity activity, long id) {
        AppLifecycleHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.stopInternal(id);
        }
    }

    static int read(Activity activity) {
        AppLifecycleHelper helper = HELPERS.get(activity);
        return helper == null ? -1 : helper.readInternal();
    }

    private int startInternal(long id) {
        synchronized (lock) {
            if (destroyed) {
                return START_FAILED;
            }
            current = id;
            return START_OK;
        }
    }

    // Activity のライフサイクル通知（`AppLifecycleLifecycle` から、UI スレッド）。helper が無ければ何もしない。
    static void started(Activity activity) {
        AppLifecycleHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.onStart();
        }
    }

    static void stopped(Activity activity) {
        AppLifecycleHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.onStop();
        }
    }

    static void resumed(Activity activity) {
        AppLifecycleHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.onResume();
        }
    }

    static void paused(Activity activity) {
        AppLifecycleHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.onPause();
        }
    }

    // 現在の id と一致するときだけ空にする（再生成後の新インスタンスへ届く古い id の stop は何もしない）。
    private void stopInternal(long id) {
        synchronized (lock) {
            if (id == current) {
                current = NONE;
            }
        }
    }

    private void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            current = NONE;
        }
    }

    void onResume() {
        notifyEvent(EVENT_ACTIVATED);
    }

    void onStart() {
        notifyEvent(EVENT_SHOWN);
    }

    void onPause() {
        if (!activity.isChangingConfigurations()) {
            notifyEvent(EVENT_DEACTIVATED);
        }
    }

    void onStop() {
        if (!activity.isChangingConfigurations()) {
            notifyEvent(EVENT_HIDDEN);
        }
    }

    private int readInternal() {
        try {
            ActivityManager.RunningAppProcessInfo info = new ActivityManager.RunningAppProcessInfo();
            ActivityManager.getMyMemoryState(info);
            int importance = info.importance;
            if (importance == ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND) {
                return 1 | (1 << 2);
            }
            if (importance == ActivityManager.RunningAppProcessInfo.IMPORTANCE_VISIBLE) {
                return 0 | (1 << 2);
            }
            if (importance > ActivityManager.RunningAppProcessInfo.IMPORTANCE_VISIBLE) {
                return 0;
            }
            return FIELD_UNKNOWN | (FIELD_UNKNOWN << 2);
        } catch (RuntimeException e) {
            return -1;
        }
    }

    private void notifyEvent(int code) {
        long id;
        synchronized (lock) {
            if (current == NONE) {
                return;
            }
            id = current;
        }
        nativeOnAppLifecycle(id, code);
    }
}
