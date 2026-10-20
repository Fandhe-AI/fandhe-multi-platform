package com.fandhe.androidapp;

import android.Manifest;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.os.Build;
import android.os.CancellationSignal;
import android.os.OperationCanceledException;
import android.provider.MediaStore;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;

import java.util.ArrayList;
import java.util.List;

// 音楽ライブラリ画面（Issue #485）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_music_library.rs` が `MainActivity.requestMusicLibraryList` /
// `cancelMusicLibraryList` を JNI で呼び、結果を `nativeOnMusicLibraryResult` で受ける）。
//
// 1 要求 = 実行時許可の確認 → `MediaStore.Audio.Media` を OS の返す順に最大 21 行
// （契約の MAX_TRACKS + 1。21 行目の有無で Rust 側が truncated を決める）読む。
// 曲の整形・切り詰めは Rust の `tracks_from` に一本化し、ここでは行を送るだけ。
// 開始・許可結果は UI スレッドで直列化し、クエリは名前付きスレッドで実行する。
// `cancel` は UI キューの停滞に依存しないよう呼び出しスレッドで即時に処理する。
//
// ライフサイクル: `onPause` では破棄しない（許可ダイアログ自体が pause を起こすため。クエリは
// Rust 側の期限後の `cancel` で必ず止まる）。`onDestroy`（Activity 再生成を含む）では保留中の
// 要求を FAILED で確定して破棄する。`request` はインスタンスフィールドなので、再生成後の
// 新インスタンスは NONE から始まり、旧スレッドの結果は `complete(id)` に失敗して捨てられる。
final class MusicLibraryHelper {
    // 結果コード。SSOT は Rust 側 `fandhe_device::music_library::decode_code`。
    // 3（Timeout）は Rust の期限だけが出すので Java からは送らない。
    static final int RESULT_OK = 0;
    static final int RESULT_UNSUPPORTED = 1;
    static final int RESULT_PERMISSION_DENIED = 2;
    static final int RESULT_FAILED = 4;

    // 契約の MAX_TRACKS(20) + 1。
    private static final int MAX_ROWS = 21;

    // rows は [title0, artist0, title1, artist1, ...]（値が無い項目は null）。失敗時は null。
    private static native void nativeOnMusicLibraryResult(long requestId, int code, String[] rows);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    private final Object lock = new Object();
    // 取り消し済みの最大 id（lock の内側）。cancel が start より先に走っても古い要求を開始しない。
    private long cancelledId = RequestTracker.NONE;
    // 実行中クエリの取り消し用（lock の内側）。
    private CancellationSignal signal;
    // 許可ダイアログを起動した要求 id（UI スレッド専用）。
    private long permissionId = RequestTracker.NONE;
    // UI スレッドで最後に受理した id。これ以下の開始要求は追い越された古い要求として捨てる。
    private long latestId = RequestTracker.NONE;

    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity の
    // フィールド初期化子から本クラスを生成してここで登録する。
    private final ActivityResultLauncher<String> permissionLauncher;

    MusicLibraryHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.RequestPermission(), this::onPermissionResult);
    }

    // JNI スレッドから呼ばれる。
    void read(long id) {
        activity.runOnUiThread(() -> start(id));
    }

    // Rust 側の期限切れから呼ばれる。戻る前にクエリを取り消す（UI キューへ post しない）。
    void cancel(long id) {
        synchronized (lock) {
            cancelledId = Math.max(cancelledId, id);
            if (request.complete(id) && signal != null) {
                signal.cancel();
            }
        }
    }

    // MainActivity.onDestroy から UI スレッドで呼ぶ。
    void onDestroy() {
        long id;
        synchronized (lock) {
            id = request.take();
            if (signal != null) {
                signal.cancel();
            }
        }
        if (id != RequestTracker.NONE) {
            nativeOnMusicLibraryResult(id, RESULT_FAILED, null);
        }
    }

    private boolean destroyed() {
        return activity.isFinishing() || activity.isDestroyed();
    }

    private static String permission() {
        // API 33 以上は READ_MEDIA_AUDIO、それ未満は READ_EXTERNAL_STORAGE（minSdk 24 の全域）。
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
                ? Manifest.permission.READ_MEDIA_AUDIO
                : Manifest.permission.READ_EXTERNAL_STORAGE;
    }

    private void start(long id) {
        if (id <= latestId) {
            return;
        }
        latestId = id;
        if (destroyed()) {
            nativeOnMusicLibraryResult(id, RESULT_FAILED, null);
            return;
        }
        synchronized (lock) {
            if (id <= cancelledId) {
                return;
            }
            request.track(id);
        }
        checkPermissionAndQuery(id);
    }

    private void checkPermissionAndQuery(long id) {
        if (activity.checkSelfPermission(permission()) != PackageManager.PERMISSION_GRANTED) {
            // ダイアログは同時に 1 つだけ。出ている間に差し替わった要求は結果到着時に再判定する。
            if (permissionId == RequestTracker.NONE) {
                permissionId = id;
                permissionLauncher.launch(permission());
            }
            return;
        }
        startQuery(id);
    }

    // 許可ダイアログ自体が Activity を pause させるため、resumed では分岐しない。
    private void onPermissionResult(boolean granted) {
        long launched = permissionId;
        permissionId = RequestTracker.NONE;
        long current = request.current();
        if (current == RequestTracker.NONE) {
            return; // 期限切れ・破棄済み・再生成後のインスタンス。
        }
        if (destroyed()) {
            finishIf(current, RESULT_FAILED, null);
        } else if (launched != current) {
            checkPermissionAndQuery(current);
        } else if (!granted) {
            finishIf(current, RESULT_PERMISSION_DENIED, null);
        } else {
            startQuery(current);
        }
    }

    private void startQuery(long id) {
        CancellationSignal sig = new CancellationSignal();
        synchronized (lock) {
            if (request.current() != id) {
                return; // 取り消し・差し替え済み。
            }
            signal = sig;
        }
        new Thread(() -> runQuery(id, sig), "music-library-query").start();
    }

    private void runQuery(long id, CancellationSignal sig) {
        int code;
        String[] rows = null;
        try (Cursor c = activity.getContentResolver().query(
                MediaStore.Audio.Media.EXTERNAL_CONTENT_URI,
                new String[] {MediaStore.Audio.Media.TITLE, MediaStore.Audio.Media.ARTIST},
                MediaStore.Audio.Media.IS_MUSIC + " != 0",
                null,
                null, // 並び順は OS 任せ（契約: OS の返す順）。
                sig)) {
            if (c == null) {
                code = RESULT_FAILED;
            } else {
                List<String> out = new ArrayList<>();
                for (int n = 0; n < MAX_ROWS && c.moveToNext(); n++) {
                    out.add(emptyToNull(c.getString(0), false));
                    out.add(emptyToNull(c.getString(1), true));
                }
                rows = out.toArray(new String[0]);
                code = RESULT_OK;
            }
        } catch (SecurityException e) {
            code = RESULT_PERMISSION_DENIED; // 許可を取り消された等。
        } catch (OperationCanceledException e) {
            return; // 取り消し済み。何も送らない。
        } catch (RuntimeException e) {
            code = RESULT_FAILED; // 曲名を含みうるため内容はログに出さない。
        }
        finishIf(id, code, code == RESULT_OK ? rows : null);
    }

    // アーティスト未設定は MediaStore が "<unknown>" で返すため未設定として扱う。
    private static String emptyToNull(String s, boolean artist) {
        if (s == null || s.isEmpty() || (artist && MediaStore.UNKNOWN_STRING.equals(s))) {
            return null;
        }
        return s;
    }

    // 現在の要求のときだけ 1 回確定する。古いスレッドの結果は照合で捨てる。
    private void finishIf(long id, int code, String[] rows) {
        synchronized (lock) {
            if (!request.complete(id)) {
                return;
            }
            signal = null;
        }
        nativeOnMusicLibraryResult(id, code, rows);
    }
}
