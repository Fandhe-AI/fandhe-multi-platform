package com.fandhe.platform;

import android.content.Context;
import android.os.Build;
import android.os.storage.StorageManager;
import android.os.storage.StorageVolume;

import android.app.Activity;
import androidx.activity.ComponentActivity;
import androidx.annotation.RequiresApi;

import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.WeakHashMap;

// 外部ストレージの着脱画面（Issue #859、親 #175）の Android 実装ヘルパー。利用者の Activity には何も書かせない
// （Rust の `fandhe-cap-removable-storage/src/android.rs` が static の `support(Activity)` / `start(Activity, long)` /
// `stop(Activity, long)` / `list(Activity)` を JNI で呼び、出来事を `nativeOnRemovableStorageEvent` で受ける）。
// Activity の生成・破棄は `RemovableStorageLifecycle`（非公開 ContentProvider、`FeatureLifecycleProvider` 継承）が
// `attach` / `detach` で知らせる。Activity ごとの helper は `HELPERS` に持つ（Rust のワーカーと UI スレッドの両方から
// 触るので同期化し、Activity が GC されれば項目も消える）。helper が無い（生成前・破棄後）の start / list は失敗で確定する。
// 一覧は `StorageManager.getStorageVolumes()`（API 24）、通知は `registerStorageVolumeCallback`（API 30）。
//
// コードの SSOT はここで、Rust 側 `android.rs` の decode_* / from_android が同じ値で写像する。
//   support の戻り値: 1 = 通知が取れる（API 30 以上） / 0 = 取れない
//   start の戻り値: 0 = 受け付けた / 1 = 失敗 / 2 = 非対応（API 30 未満）
//   list の戻り値: 失敗は null。成功は "<件数>" の後に項目ごと U+001E + flags + U+001F + state + U+001F + description
//     flags: bit0 = isPrimary / bit1 = isRemovable。state は getState()（null は空）。description は getDescription（null は空）
//     state・description 内の U+001E / U+001F は取り除く。UUID・getDirectory・パスは読まない
//   通知: nativeOnRemovableStorageEvent(id, state)。state は onStateChanged の getState()（null は空）。
//     一覧は通知の中身から組み立てず、Rust のドライバが list で読み直す
//
// スレッドとロック: 状態（current / callback / destroyed）は `lock` の内側だけで読み書きする。コールバックは
// main executor で動き、native 呼び出しはロックの外で行う（古い id は Rust の登録表が捨てる）。ロック取得順は
// Java -> Rust の一方向。BroadcastReceiver・権限は使わない。
//
// ライフサイクル: onPause では止めない（背景でも着脱は受けてよい。画面を離れたときの停止は Rust 側が行う）。
// onDestroy（Activity 再生成を含む）はコールバックを解除し、停止を失敗として送らない。新インスタンスは購読を
// 持たない状態から始まる。名前・状態・id・例外の内容はログへ出さない。
// 守らない範囲: 画面を開く前・止めた後の出来事、通知と一覧の読み直しの間の時間差、同じ着脱での複数回通知、
// 一覧の読み取りの期限。
final class RemovableStorageHelper {
    static final int START_OK = 0;
    static final int START_FAILED = 1;
    static final int START_UNSUPPORTED = 2;

    private static final long NONE = 0L;
    private static final char REC = '\u001E';
    private static final char SEP = '\u001F';

    private static native void nativeOnRemovableStorageEvent(long id, String state);

    // Activity ごとの helper（上記クラス冒頭コメント参照）。
    private static final Map<Activity, RemovableStorageHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private final ComponentActivity activity;
    private final Object lock = new Object();
    private long current = NONE;
    private Object callback;
    private boolean destroyed;

    private RemovableStorageHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // Activity の生成時（`RemovableStorageLifecycle` から、UI スレッド）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new RemovableStorageHelper(activity));
    }

    // Activity の破棄時（同上）。コールバックを解除し、以後の開始を拒否する。
    static void detach(Activity activity) {
        RemovableStorageHelper helper = HELPERS.remove(activity);
        if (helper != null) {
            helper.onDestroy();
        }
    }

    // Rust から呼ぶ入口。SDK のバージョンだけで決まるので helper は要らない。
    static int support(Activity activity) {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.R ? 1 : 0;
    }

    static int start(Activity activity, long id) {
        RemovableStorageHelper helper = HELPERS.get(activity);
        return helper == null ? START_FAILED : helper.startInternal(id);
    }

    static void stop(Activity activity, long id) {
        RemovableStorageHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.stopInternal(id);
        }
    }

    // helper が無ければ失敗（null）で確定する。
    static String list(Activity activity) {
        RemovableStorageHelper helper = HELPERS.get(activity);
        return helper == null ? null : helper.listInternal();
    }

    private int startInternal(long id) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            return START_UNSUPPORTED;
        }
        synchronized (lock) {
            if (destroyed) {
                return START_FAILED;
            }
            unregisterLocked();
            try {
                Context app = activity.getApplicationContext();
                StorageManager sm = app.getSystemService(StorageManager.class);
                if (sm == null) {
                    return START_FAILED;
                }
                Api30Callback cb = new Api30Callback(this);
                sm.registerStorageVolumeCallback(app.getMainExecutor(), cb);
                callback = cb;
                current = id;
                return START_OK;
            } catch (RuntimeException e) {
                return START_FAILED;
            }
        }
    }

    // 現在の id と一致するときだけ解除する（再生成後の新インスタンスへ届く古い id の stop は何もしない）。
    private void stopInternal(long id) {
        synchronized (lock) {
            if (id == current) {
                unregisterLocked();
            }
        }
    }

    private void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            unregisterLocked();
        }
    }

    private String listInternal() {
        try {
            Context app = activity.getApplicationContext();
            StorageManager sm = app.getSystemService(StorageManager.class);
            if (sm == null) {
                return null;
            }
            List<StorageVolume> vols = sm.getStorageVolumes();
            StringBuilder sb = new StringBuilder();
            sb.append(vols.size());
            for (StorageVolume v : vols) {
                int flags = (v.isPrimary() ? 1 : 0) | (v.isRemovable() ? 2 : 0);
                String state = v.getState();
                String desc = v.getDescription(app);
                sb.append(REC).append(flags).append(SEP)
                        .append(state == null ? "" : clean(state)).append(SEP)
                        .append(desc == null ? "" : clean(desc));
            }
            return sb.toString();
        } catch (RuntimeException e) {
            return null;
        }
    }

    private static String clean(String s) {
        return s.replace(String.valueOf(REC), "").replace(String.valueOf(SEP), "");
    }

    private void onChanged(Object self, String state) {
        long id;
        synchronized (lock) {
            if (callback != self || current == NONE) {
                return;
            }
            id = current;
        }
        nativeOnRemovableStorageEvent(id, state == null ? "" : state);
    }

    // lock を持った状態で呼ぶ。解除の失敗（既に解除済み等）は無視して状態を空にする。
    private void unregisterLocked() {
        Object cb = callback;
        callback = null;
        current = NONE;
        if (cb != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            try {
                StorageManager sm = activity.getApplicationContext().getSystemService(StorageManager.class);
                if (sm != null) {
                    sm.unregisterStorageVolumeCallback((StorageManager.StorageVolumeCallback) cb);
                }
            } catch (RuntimeException e) {
                // 既に解除済み。
            }
        }
    }

    // API 30 のクラスを API 24 で解決させないため、参照はこの入れ子クラスに閉じる。
    @RequiresApi(Build.VERSION_CODES.R)
    private static final class Api30Callback extends StorageManager.StorageVolumeCallback {
        private final RemovableStorageHelper owner;

        Api30Callback(RemovableStorageHelper owner) {
            this.owner = owner;
        }

        @Override
        public void onStateChanged(StorageVolume volume) {
            owner.onChanged(this, volume == null ? null : volume.getState());
        }
    }
}
