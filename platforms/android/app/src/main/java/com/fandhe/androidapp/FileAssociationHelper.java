package com.fandhe.androidapp;

import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;

import android.database.Cursor;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

// ファイルの関連付け（Issue #418）の Java 側ヘルパー。`MainActivity.deliverUrlIntent`
// （`onCreate` の `getIntent()` / `onNewIntent`）が `offer` を呼び、`ACTION_VIEW` の
// `content:` URI を開いた fd と名前・MIME を Rust 側 `device/android_file_association.rs` の
// `nativeOnFileOpened` へ渡す。継続配信型（要求 id なし）で、受け口は Rust の static。
// 検査（拡張子・MIME・名前）と読み取りの上限は Rust 側の共通層 `FileInbox` に一本化している。
// MIME の正は `fandhe_device::file_association::MIME`（manifest の intent-filter と一致させる）。
//
// 結果コード表の SSOT（Rust 側 `deliver` が写像する）:
//   0 = FILE（fd 有効）/ 1 = REJECTED（`file:` など）/ 2 = UNREADABLE / 3 = FOLDER。
//
// I/O は単一の直列キューに集約し UI スレッドを止めない。Activity は握らない（ApplicationContext
// の ContentResolver のみ）ため再生成で参照が切れず、`onDestroy` でも止めない。
// `takePersistableUriPermission` は呼ばない（送り手が付けた一時許可の範囲だけで読む）。
// 守る範囲: 1 プロセス内の受信順、結果コードの嘘の成功の禁止。
// 守らない範囲: プロセス死後の再表示、読み取り許可の無い URI（UNREADABLE になる）、複数プロセス。
// URI・名前・例外メッセージ・中身はログへ出さない（security.md A09）。
final class FileAssociationHelper {
    static final int CODE_FILE = 0;
    static final int CODE_REJECTED = 1;
    static final int CODE_UNREADABLE = 2;
    static final int CODE_FOLDER = 3;

    private static final ExecutorService EXECUTOR = Executors.newSingleThreadExecutor();

    private FileAssociationHelper() {}

    static native void nativeOnFileOpened(int code, String name, String mime, int fd);

    /**
     * `ACTION_VIEW` で scheme が `content` / `file` の intent だけを引き受けて true を返す。
     * それ以外（`fandhe://` や MAIN 起動）は false で、呼び出し元が url-scheme へ流す。
     */
    static boolean offer(Context context, Intent intent) {
        if (intent == null || !Intent.ACTION_VIEW.equals(intent.getAction())) {
            return false;
        }
        Uri uri = intent.getData();
        String scheme = uri == null ? null : uri.getScheme();
        if ("file".equals(scheme)) {
            // 受信順を保つため同期では送らず、同じキューに積む。
            EXECUTOR.execute(() -> nativeOnFileOpened(CODE_REJECTED, null, null, -1));
            return true;
        }
        if (!"content".equals(scheme)) {
            return false;
        }
        final ContentResolver resolver = context.getApplicationContext().getContentResolver();
        final String intentType = intent.getType();
        EXECUTOR.execute(() -> open(resolver, uri, intentType));
        return true;
    }

    private static void open(ContentResolver resolver, Uri uri, String intentType) {
        String mime = intentType;
        try {
            String type = resolver.getType(uri);
            if (DocumentsContract.Document.MIME_TYPE_DIR.equals(type)) {
                nativeOnFileOpened(CODE_FOLDER, null, null, -1);
                return;
            }
            if (type != null) {
                mime = type;
            }
        } catch (RuntimeException e) {
            nativeOnFileOpened(CODE_UNREADABLE, null, null, -1);
            return;
        }
        // 名前は DISPLAY_NAME だけ（末尾パスへの fallback はしない。取れなければ Rust 側が拒否に数える）。
        String name = null;
        try (Cursor c = resolver.query(uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                int i = c.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (i >= 0 && !c.isNull(i)) {
                    name = c.getString(i);
                }
            }
        } catch (RuntimeException e) {
            // 名前なしとして続行する。
        }
        int fd;
        try (ParcelFileDescriptor pfd = resolver.openFileDescriptor(uri, "r")) {
            if (pfd == null) {
                nativeOnFileOpened(CODE_UNREADABLE, null, null, -1);
                return;
            }
            fd = pfd.detachFd();
        } catch (Exception e) {
            nativeOnFileOpened(CODE_UNREADABLE, null, null, -1);
            return;
        }
        // detach 済みの fd の所有は Rust 側へ移る（Java 側は close しない）。
        nativeOnFileOpened(CODE_FILE, name, mime, fd);
    }
}
