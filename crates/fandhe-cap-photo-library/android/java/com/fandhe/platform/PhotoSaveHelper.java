package com.fandhe.platform;

import android.content.ContentResolver;
import android.content.Context;
import android.content.ContentValues;
import android.net.Uri;
import android.os.Build;
import android.os.Environment;
import android.provider.MediaStore;

import java.io.IOException;
import java.io.OutputStream;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

// 写真ライブラリ保存（Issue #1256）の Java 側ヘルパー。Rust 側 `fandhe-cap-photo-library/src/android.rs` の
// `begin_save` が static の `save(Context, long, byte[])` を JNI で呼び、結果を `nativeOnPhotoSaveResult` で返す
// （`driver.rs::bridge` と pull 型の橋渡し）。Activity を握らないため provider は作らない（再生成時の破棄は
// Rust 側の `reset_pending_tracking` と対）。
//
// 結果コード表の SSOT（Rust 側 `outcome_from_android_code` が写像する）:
//   0 = IS_PENDING=0 で確定 / 1 = API 28 以下（非対応）/ 2 = insert・書き込み・確定の失敗（片付け済み）
//   / 3 = 失敗後の片付け（delete）の失敗。
//
// API 29+ のみ対応する。権限は不要（自アプリが作る行だけを書く）。API 24〜28 は
// WRITE_EXTERNAL_STORAGE の実行時権限と別の保存先が要るため非対応（コード 1）。
// I/O は単一スレッドの直列キューに集約し、描画スレッドを止めない。Activity は握らない
// （ContentResolver のみ）ため再生成で参照が切れない。
// 守る範囲: 1 プロセス内の順序、嘘の成功の禁止、完了通知を 1 回だけ反映。
// 守らない範囲: 複数プロセスの同時保存、保存後の編集・削除、再生成やアプリ終了と重なった保存の表示。
// 画像バイト列・uri・パスはログへ出さない（security.md A09）。
final class PhotoSaveHelper {
    static final int RESULT_SAVED = 0;
    static final int RESULT_UNSUPPORTED = 1;
    static final int RESULT_FAILED = 2;
    static final int RESULT_CLEANUP_FAILED = 3;

    private static final RequestTracker TRACKER = new RequestTracker();
    private static final ExecutorService EXECUTOR = Executors.newSingleThreadExecutor();

    private PhotoSaveHelper() {}

    static native void nativeOnPhotoSaveResult(long requestId, int code);

    /** 保存を直列キューへ積んで即 return する。結果は `nativeOnPhotoSaveResult` で届く。 */
    static void save(Context activity, long requestId, byte[] png) {
        final ContentResolver resolver = activity.getApplicationContext().getContentResolver();
        TRACKER.track(requestId);
        EXECUTOR.execute(() -> {
            int code = doSave(resolver, png);
            // 新しい要求に差し替えられた旧要求は通知しない（track は上書きなので last-wins。Rust 側 RequestSlot.begin と整合）。
            if (TRACKER.complete(requestId)) {
                nativeOnPhotoSaveResult(requestId, code);
            }
        });
    }

    private static int doSave(ContentResolver resolver, byte[] png) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return RESULT_UNSUPPORTED;
        }
        Uri uri = null;
        try {
            ContentValues values = new ContentValues();
            values.put(MediaStore.Images.Media.DISPLAY_NAME,
                    "fandhe-photo-" + System.currentTimeMillis() + ".png");
            values.put(MediaStore.Images.Media.MIME_TYPE, "image/png");
            values.put(MediaStore.Images.Media.RELATIVE_PATH, Environment.DIRECTORY_PICTURES);
            values.put(MediaStore.Images.Media.IS_PENDING, 1);
            uri = resolver.insert(
                    MediaStore.Images.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY), values);
            if (uri == null) {
                return RESULT_FAILED;
            }
            try (OutputStream out = resolver.openOutputStream(uri)) {
                if (out == null) {
                    return cleanup(resolver, uri);
                }
                out.write(png);
            }
            ContentValues done = new ContentValues();
            done.put(MediaStore.Images.Media.IS_PENDING, 0);
            if (resolver.update(uri, done, null, null) == 1) {
                return RESULT_SAVED;
            }
            return cleanup(resolver, uri);
        } catch (IOException | RuntimeException e) {
            return uri == null ? RESULT_FAILED : cleanup(resolver, uri);
        }
    }

    /** 挿入済みの行を消す。消せたら 2（失敗）、消せなければ 3（片付け失敗）。 */
    private static int cleanup(ContentResolver resolver, Uri uri) {
        try {
            return resolver.delete(uri, null, null) >= 1 ? RESULT_FAILED : RESULT_CLEANUP_FAILED;
        } catch (RuntimeException e) {
            return RESULT_CLEANUP_FAILED;
        }
    }
}
