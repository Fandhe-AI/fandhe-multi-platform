package com.fandhe.androidapp;

import android.app.ActivityManager;
import android.content.ComponentCallbacks2;
import android.content.Context;
import android.content.res.Configuration;

import androidx.activity.ComponentActivity;

// メモリ逼迫・負荷の通知画面（Issue #913、親 #179。共通コアと macOS / Windows は #911）の Android 実装
// ヘルパー。MainActivity から生成・委譲される（Rust の `device/android_memory_pressure.rs` が
// `MainActivity.memoryPressureStart` / `Stop` / `Read` を JNI で呼び、合図を `nativeOnMemoryTrim` で受ける）。
// Application Context へ ComponentCallbacks2 を登録し、onTrimMemory(level) の level を値のまま Rust へ流す。
// 段階は今後増えうるため大小・特定値の判定はせず、未知値も捨てない。onLowMemory は API 34 以上で呼ばれず
// 対応する種類も無いため何もしない（trim 値を捏造しない）。
//
// コードの SSOT はここで、Rust 側 `android_memory_pressure.rs` が同じ値を写像する。
//   start の戻り値: 0 = 受け付けた / 1 = 失敗
//   read の戻り値: 1 = lowMemory / 0 = 不足ではない / -1 = 失敗
//
// 今の状態は覚えず、read() が ActivityManager.getMemoryInfo の lowMemory を毎回 OS へ問う。権限・マニフェスト
// 宣言は不要。
//
// スレッドとロック: 購読 id（current）・登録中のコールバック・destroyed は `lock` の内側だけで読み書きする。
// コールバックは main thread で届き、native 呼び出しはロックの外で行う。ロック取得順は Java -> Rust の一方向。
// read は egui スレッドから同期で呼べる。
//
// ライフサイクル: onPause / onStop では止めない（背景でこそ合図が要る）。Application Context は Activity より
// 長生きするため、onDestroy で必ず登録を解除する（解除しないと旧インスタンスの登録が残り続ける）。停止を
// 失敗として送らない。新インスタンスは購読なしで始まり、Rust 側の画面は Idle へ戻るため古い購読は drop され、
// 新インスタンスへ届く古い id の stop は id 不一致で何もしない。id・例外の内容はログへ出さない。
// 守らない範囲: 画面を開く前・止めた後・背景で停止中の合図、OS 間の「逼迫」基準の差、システム全体の値で
// あること、API 34 以上は UI_HIDDEN / BACKGROUND の 2 段階しか届かないこと。
final class MemoryPressureHelper {
    static final int START_OK = 0;
    static final int START_FAILED = 1;

    private static final long NONE = 0L;

    private static native void nativeOnMemoryTrim(long id, int level);

    private final ComponentActivity activity;
    private final Object lock = new Object();
    private long current = NONE;
    private ComponentCallbacks2 callbacks;
    private boolean destroyed;

    MemoryPressureHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    int start(long id) {
        synchronized (lock) {
            if (destroyed) {
                return START_FAILED;
            }
            unregisterLocked();
            ComponentCallbacks2 cb = new ComponentCallbacks2() {
                @Override
                public void onTrimMemory(int level) {
                    notifyTrim(this, level);
                }

                @Override
                public void onLowMemory() {}

                @Override
                public void onConfigurationChanged(Configuration newConfig) {}
            };
            try {
                activity.getApplicationContext().registerComponentCallbacks(cb);
            } catch (RuntimeException e) {
                return START_FAILED;
            }
            callbacks = cb;
            current = id;
            return START_OK;
        }
    }

    // 現在の id と一致するときだけ解除する（再生成後の新インスタンスへ届く古い id の stop は何もしない）。
    void stop(long id) {
        synchronized (lock) {
            if (id == current) {
                unregisterLocked();
            }
        }
    }

    void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            unregisterLocked();
        }
    }

    int read() {
        try {
            ActivityManager am =
                    (ActivityManager) activity.getSystemService(Context.ACTIVITY_SERVICE);
            if (am == null) {
                return -1;
            }
            ActivityManager.MemoryInfo info = new ActivityManager.MemoryInfo();
            am.getMemoryInfo(info);
            return info.lowMemory ? 1 : 0;
        } catch (RuntimeException e) {
            return -1;
        }
    }

    // lock の内側で呼ぶ。解除の失敗は無視する（すでに外れている場合と区別できず、状態は空にする）。
    private void unregisterLocked() {
        if (callbacks != null) {
            try {
                activity.getApplicationContext().unregisterComponentCallbacks(callbacks);
            } catch (RuntimeException e) {
                // 何もしない
            }
            callbacks = null;
        }
        current = NONE;
    }

    private void notifyTrim(ComponentCallbacks2 source, int level) {
        long id;
        synchronized (lock) {
            if (source != callbacks || current == NONE) {
                return;
            }
            id = current;
        }
        nativeOnMemoryTrim(id, level);
    }
}
