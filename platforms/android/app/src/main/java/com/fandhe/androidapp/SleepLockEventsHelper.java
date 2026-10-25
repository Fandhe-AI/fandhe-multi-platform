package com.fandhe.androidapp;

import android.annotation.SuppressLint;
import android.app.KeyguardManager;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.os.Handler;
import android.os.Looper;
import android.os.PowerManager;

import androidx.activity.ComponentActivity;

// スリープ・画面ロック・解除の通知画面（Issue #811、親 #167）の Android 実装ヘルパー。MainActivity から
// 生成・委譲される（Rust の `device/android_sleep_lock_events.rs` が `MainActivity.sleepLockEventsStart` /
// `Stop` / `Read` を JNI で呼び、出来事を `nativeOnSleepLockEvent` で受ける）。
// 知らせる種類は ACTION_SCREEN_OFF / ON / USER_PRESENT（解除）だけ。システムのスリープ・復帰と
// ロックした瞬間を知らせるブロードキャストは無い。
//
// コードの SSOT はここで、Rust 側 `android_sleep_lock_events.rs` の decode_* が同じ値で写像する。
//   出来事: 0 = SCREEN_OFF / 1 = SCREEN_ON / 2 = USER_PRESENT
//   start の戻り値: 0 = 受け付けた / 1 = 失敗
//   read の戻り値: 負 = 失敗。bit 0-1 画面 (0 消灯 / 1 点灯 / 2 不明)、bit 2-3 ロック (0 解除 / 1 ロック中 / 2 不明)
//
// 登録: Application Context へ動的に登録する（SCREEN_OFF / ON はマニフェスト宣言では届かない）。3 つとも
// システムだけが送る保護されたブロードキャストで、Android 14 以上でもこの場合は RECEIVER_EXPORTED /
// NOT_EXPORTED を指定しない（公式 broadcasts ガイド、Android 14 behavior changes）。新しい権限は不要。
//
// スレッドとロック: 状態（current / receiver / destroyed）は `lock` の内側だけで読み書きする。
// onReceive は main looper で動き、native 呼び出しはロックの外で行う（古い id は Rust の登録表が捨てる）。
// ロック取得順は Java -> Rust の一方向。read は binder の getter で Looper の制約が無く、egui スレッドから
// 同期で呼べる。
//
// ライフサイクル: onPause / onStop では止めない（画面を消すと Activity が onPause / onStop するため、
// 止めると消灯を受けられない）。onDestroy（Activity 再生成を含む）はレシーバーを解除する。停止を
// 失敗として送らない。新インスタンスは購読を持たない状態から始まる。id・例外の内容はログへ出さない。
// 守らない範囲: Android 14 以上でキャッシュ状態の間に溜められた通知の順序と畳み込み、画面を開く前・
// 止めた後の出来事。
final class SleepLockEventsHelper {
    static final int START_OK = 0;
    static final int START_FAILED = 1;

    static final int EVENT_SCREEN_OFF = 0;
    static final int EVENT_SCREEN_ON = 1;
    static final int EVENT_USER_PRESENT = 2;

    private static final long NONE = 0L;
    private static final int FIELD_UNKNOWN = 2;

    private static native void nativeOnSleepLockEvent(long id, int code);

    private final ComponentActivity activity;
    private final Object lock = new Object();
    private long current = NONE;
    private BroadcastReceiver receiver;
    private boolean destroyed;

    SleepLockEventsHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    @SuppressLint("UnspecifiedRegisterReceiverFlag")
    int start(long id) {
        synchronized (lock) {
            if (destroyed) {
                return START_FAILED;
            }
            // 古い id のレシーバーを残さないための片付け（通常は購読が 1 本なので起きない）。
            unregisterLocked();
            IntentFilter filter = new IntentFilter();
            filter.addAction(Intent.ACTION_SCREEN_OFF);
            filter.addAction(Intent.ACTION_SCREEN_ON);
            filter.addAction(Intent.ACTION_USER_PRESENT);
            BroadcastReceiver r = new BroadcastReceiver() {
                @Override
                public void onReceive(Context context, Intent intent) {
                    onBroadcast(this, intent);
                }
            };
            try {
                // システムのブロードキャストだけを受けるのでフラグは付けない。
                activity.getApplicationContext()
                        .registerReceiver(r, filter, null, new Handler(Looper.getMainLooper()));
            } catch (RuntimeException e) {
                return START_FAILED;
            }
            receiver = r;
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
            Context context = activity.getApplicationContext();
            int screen = FIELD_UNKNOWN;
            int locked = FIELD_UNKNOWN;
            PowerManager pm = context.getSystemService(PowerManager.class);
            if (pm != null) {
                screen = pm.isInteractive() ? 1 : 0;
            }
            KeyguardManager km = context.getSystemService(KeyguardManager.class);
            if (km != null) {
                locked = km.isKeyguardLocked() ? 1 : 0;
            }
            return screen | (locked << 2);
        } catch (RuntimeException e) {
            return -1;
        }
    }

    private void onBroadcast(BroadcastReceiver self, Intent intent) {
        if (intent == null || intent.getAction() == null) {
            return;
        }
        int code;
        switch (intent.getAction()) {
            case Intent.ACTION_SCREEN_OFF:
                code = EVENT_SCREEN_OFF;
                break;
            case Intent.ACTION_SCREEN_ON:
                code = EVENT_SCREEN_ON;
                break;
            case Intent.ACTION_USER_PRESENT:
                code = EVENT_USER_PRESENT;
                break;
            default:
                return;
        }
        long id;
        synchronized (lock) {
            if (receiver != self || current == NONE) {
                return;
            }
            id = current;
        }
        nativeOnSleepLockEvent(id, code);
    }

    // lock を持った状態で呼ぶ。解除の失敗（既に解除済み等）は無視して状態を空にする。
    private void unregisterLocked() {
        BroadcastReceiver r = receiver;
        receiver = null;
        current = NONE;
        if (r != null) {
            try {
                activity.getApplicationContext().unregisterReceiver(r);
            } catch (RuntimeException e) {
                // 既に解除済み。
            }
        }
    }
}
