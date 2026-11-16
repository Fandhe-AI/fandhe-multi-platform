package com.fandhe.platform;

import android.app.Activity;
import android.content.Context;
import android.content.SharedPreferences;

import androidx.activity.ComponentActivity;
import androidx.lifecycle.LiveData;
import androidx.lifecycle.Observer;
import androidx.work.Constraints;
import androidx.work.Data;
import androidx.work.ExistingWorkPolicy;
import androidx.work.NetworkType;
import androidx.work.OneTimeWorkRequest;
import androidx.work.WorkInfo;
import androidx.work.WorkManager;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.WeakHashMap;
import java.util.concurrent.ExecutionException;

// OS 管理のバックグラウンド送信（Issue #837、親 #170）の Android 実装ヘルパー。
// `BackgroundUploadLifecycle`（非公開 provider、Issue #1333）が Activity ごとに生成して `HELPERS` に持つ（Rust の
// `crates/fandhe-cap-background-upload/src/android.rs` がドライバの直列ワーカースレッドから static の `start` / `cancel` /
// `status` / `observe` を同期 JNI で呼ぶ。利用者の `MainActivity` に受け口は要らない）。Helper が無い（生成前・破棄後）要求は
// 成功にも待ちにもせず失敗（start=3・cancel=1・status=null・observe=1）で確定する。
//
// 戻り値のコードの SSOT はここで、Rust 側が同じ値で解釈する。
//   start  : 0=OK / 2=ファイルを用意できない / 3=開始失敗
//   cancel : 0=OK / 1=失敗
//   status : long[5] = {kind, sent, total, http, err}。読み出しの例外では null
//     kind: 0=無し（UUID 未保存、または今の要求の WorkInfo が無い）/ 1=待機（ENQUEUED・BLOCKED）
//           / 2=送信中（RUNNING。sent・total は getProgress、未設定なら 0 / FILE_SIZE）
//           / 3=終了（SUCCEEDED・FAILED。outputData の sent・http・err）/ 4=取り消し
//     http: 無ければ -1
//     err（kind 3 のときだけ意味を持つ）: 0=応答あり / 1=3xx（リダイレクト）/ 2=接続の失敗
//           / 3=ファイルを用意できない / 4=その他（出力の欠落を含む）
//   observe: 0=受け付けた / 1=失敗
//
// 状態の SSOT は WorkManager。アプリが持つのは SharedPreferences の「今の要求の UUID」1 つだけで、
// `getWorkInfosForUniqueWork` が REPLACE で置き換えた前の世代の履歴も返すため、UUID の一致する
// WorkInfo だけを使う（契約表 2。PR #1052 の教訓）。転送の持ち主は OS で、Activity の再生成を
// またいで続けるのが目的なので onDestroy では取り消さない。
//
// 合図: LiveData（getWorkInfosForUniqueWorkLiveData）の変化で Rust の「読み直せ」を呼ぶだけで、
// 状態は運ばない。Activity の作り直しでは新しいインスタンスが onCreate で登録し直す（Rust の
// set_signal はプロセスにつき 1 回のため）。observe は LifecycleOwner に結び付くので DESTROYED で
// OS が外し、登録トークンを握り続けない。LiveData は active になった時点で現在値を必ず配るので、
// 登録までの間の変化も落とさない。native コールバックは Rust が observe を呼んだ後でしか
// 呼ばれないため no-op スタブは置かない（SleepLockEventsHelper と同じ根拠）。
//
// ロックは static。status の `Future.get()` はドライバのワーカースレッド上で待ち、main looper
// を待たないのでデッドロックしない。URL・パス・例外の文言はログに出さない（A09）。
final class BackgroundUploadHelper {
    // `fandhe_cap_background_upload::FILE_NAME` / `FILE_SIZE` と同じ値。
    static final String FILE_NAME = "background-upload";
    static final long FILE_SIZE = 1024L * 1024L;

    private static final String WORK_NAME = "fandhe.background-upload";
    private static final String PREFS = "background_upload";
    private static final String KEY_ID = "id";
    private static final Object LOCK = new Object();

    private static volatile boolean signalWanted = false;

    private static final Map<Activity, BackgroundUploadHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private final ComponentActivity activity;
    private Observer<List<WorkInfo>> observer; // main thread だけ
    private LiveData<List<WorkInfo>> liveData; // main thread だけ

    private BackgroundUploadHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // Activity の生成時（`BackgroundUploadLifecycle.onHostCreated`、`Activity.onCreate` の中・main thread）。
    // 作り直し後の新しいインスタンスで、合図が要る状態なら登録し直す（Rust の `set_signal` はプロセスにつき 1 回のため）。
    // 従来は `MainActivity.onCreate` が `super.onCreate` の後に呼んでいたが、本経路は `super.onCreate` の中で届く。
    static void attach(ComponentActivity activity) {
        BackgroundUploadHelper helper = new BackgroundUploadHelper(activity);
        HELPERS.put(activity, helper);
        helper.onCreate();
    }

    // Activity の破棄時。LiveData の observe は LifecycleOwner の DESTROYED で OS が外すため、ここは表から外すだけ。
    static void detach(Activity activity) {
        HELPERS.remove(activity);
    }

    static int start(Activity activity, String url) {
        BackgroundUploadHelper helper = HELPERS.get(activity);
        return helper == null ? 3 : helper.start(url);
    }

    static int cancel(Activity activity) {
        BackgroundUploadHelper helper = HELPERS.get(activity);
        return helper == null ? 1 : helper.cancel();
    }

    static long[] status(Activity activity) {
        BackgroundUploadHelper helper = HELPERS.get(activity);
        return helper == null ? null : helper.status();
    }

    static int observe(Activity activity) {
        BackgroundUploadHelper helper = HELPERS.get(activity);
        return helper == null ? 1 : helper.observe();
    }

    // WorkManager の Operation の確定を待つ（ワーカースレッドから呼ぶため、無期限に待っても画面は固まらない）。
    // 割込みでも中断せず確定まで待ち、割込み状態は最後に戻す。戻り値: 0 = 成功、1 = 失敗。
    // ponytail: background-task の `MainActivity.awaitOperation`（#1347 で `BackgroundTaskHelper` へ移る）と同じ内容の写し。
    // WorkManager の共通部品が要る機能が 3 つ目になったとき一本化する。
    private static int awaitOperation(androidx.work.Operation operation) {
        boolean interrupted = false;
        try {
            while (true) {
                try {
                    operation.getResult().get();
                    return 0;
                } catch (InterruptedException e) {
                    interrupted = true;
                } catch (ExecutionException | RuntimeException e) {
                    return 1;
                }
            }
        } finally {
            if (interrupted) {
                Thread.currentThread().interrupt();
            }
        }
    }

    static File file(Context context) {
        return new File(context.getCacheDir(), FILE_NAME);
    }

    private SharedPreferences prefs() {
        return activity.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    private WorkManager wm() {
        return WorkManager.getInstance(activity.getApplicationContext());
    }

    // `fandhe_cap_background_upload::file_contents`（i % 251 を FILE_SIZE バイト）と同じ中身を
    // 一時ファイルへ書いて rename する。止まりきっていない前の Worker の読み込みと衝突しない。
    private boolean writeFile() {
        File dst = file(activity);
        File tmp = new File(activity.getCacheDir(), FILE_NAME + ".tmp");
        // 長さが 251 の倍数なので、先頭から繰り返し書けば位相が合う。
        byte[] buf = new byte[251 * 256];
        for (int i = 0; i < buf.length; i++) buf[i] = (byte) (i % 251);
        try (OutputStream os = new FileOutputStream(tmp)) {
            long left = FILE_SIZE;
            while (left > 0) {
                int n = (int) Math.min(buf.length, left);
                os.write(buf, 0, n);
                left -= n;
            }
        } catch (IOException e) {
            tmp.delete();
            return false;
        }
        if (!tmp.renameTo(dst)) {
            tmp.delete();
            return false;
        }
        return true;
    }

    int start(String url) {
        synchronized (LOCK) {
            try {
                if (awaitOperation(wm().cancelUniqueWork(WORK_NAME)) != 0) return 3;
                if (!writeFile()) return 2;
                OneTimeWorkRequest req = new OneTimeWorkRequest.Builder(BackgroundUploadWorker.class)
                        .setConstraints(new Constraints.Builder()
                                .setRequiredNetworkType(NetworkType.CONNECTED)
                                .build())
                        .setInputData(new Data.Builder()
                                .putString(BackgroundUploadWorker.KEY_URL, url)
                                .build())
                        .build();
                if (awaitOperation(
                                wm().enqueueUniqueWork(WORK_NAME, ExistingWorkPolicy.REPLACE, req)) != 0) {
                    return 3;
                }
                if (!prefs().edit().putString(KEY_ID, req.getId().toString()).commit()) {
                    wm().cancelWorkById(req.getId()); // 追えない転送を残さない
                    return 3;
                }
                return 0;
            } catch (RuntimeException e) {
                return 3;
            }
        }
    }

    int cancel() {
        synchronized (LOCK) {
            try {
                // UUID は残す。status が CANCELLED を読んで「取り消し」を出せる（失敗に畳まない）。
                return awaitOperation(wm().cancelUniqueWork(WORK_NAME)) == 0 ? 0 : 1;
            } catch (RuntimeException e) {
                return 1;
            }
        }
    }

    long[] status() {
        synchronized (LOCK) {
            try {
                String id = prefs().getString(KEY_ID, null);
                if (id == null) return new long[] {0, 0, FILE_SIZE, -1, 0};
                UUID mine = UUID.fromString(id);
                List<WorkInfo> infos = wm().getWorkInfosForUniqueWork(WORK_NAME).get();
                for (WorkInfo wi : infos) {
                    if (mine.equals(wi.getId())) return toRaw(wi);
                }
                return new long[] {0, 0, FILE_SIZE, -1, 0};
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return null;
            } catch (ExecutionException | RuntimeException e) {
                return null;
            }
        }
    }

    private static long[] toRaw(WorkInfo wi) {
        switch (wi.getState()) {
            case ENQUEUED:
            case BLOCKED:
                return new long[] {1, 0, FILE_SIZE, -1, 0};
            case RUNNING: {
                Data p = wi.getProgress();
                long total = p.getLong(BackgroundUploadWorker.KEY_TOTAL, FILE_SIZE);
                return new long[] {2, p.getLong(BackgroundUploadWorker.KEY_SENT, 0), total, -1, 0};
            }
            case CANCELLED:
                return new long[] {4, 0, FILE_SIZE, -1, 0};
            default: { // SUCCEEDED・FAILED
                Data o = wi.getOutputData();
                // 出力が無い（Worker が例外で落ちた等）は err=4。成功にしない。
                int err = o.getInt(BackgroundUploadWorker.KEY_ERR, BackgroundUploadWorker.ERR_OTHER);
                return new long[] {
                    3,
                    o.getLong(BackgroundUploadWorker.KEY_SENT, 0),
                    FILE_SIZE,
                    o.getInt(BackgroundUploadWorker.KEY_HTTP, -1),
                    err
                };
            }
        }
    }

    int observe() {
        signalWanted = true;
        try {
            activity.runOnUiThread(this::registerIfNeeded);
            return 0;
        } catch (RuntimeException e) {
            return 1;
        }
    }

    // Activity 作り直し後の新しいインスタンスで登録し直す（`attach` から）。
    private void onCreate() {
        if (signalWanted) registerIfNeeded();
    }

    // main thread だけで動く。インスタンスごとに 1 回だけ登録する。
    private void registerIfNeeded() {
        if (observer != null) return;
        observer = list -> nativeOnBackgroundUploadChanged();
        liveData = wm().getWorkInfosForUniqueWorkLiveData(WORK_NAME);
        liveData.observe(activity, observer);
    }

    private static native void nativeOnBackgroundUploadChanged();
}
