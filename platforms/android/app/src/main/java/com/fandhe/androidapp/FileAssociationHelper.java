package com.fandhe.androidapp;

import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;

import android.database.Cursor;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.concurrent.Callable;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;

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
// 読み取りは期限つき（10 秒）で別スレッドから一時ファイルへ写し、止まった送り手が直列キューを
// 塞がないようにする。期限切れは UNREADABLE に確定する。
// 守る範囲: 1 プロセス内の受信順、結果コードの嘘の成功の禁止。
// 守らない範囲: プロセス死後の再表示、読み取り許可の無い URI（UNREADABLE になる）、複数プロセス。
// URI・名前・例外メッセージ・中身はログへ出さない（security.md A09）。
final class FileAssociationHelper {
    static final int CODE_FILE = 0;
    static final int CODE_REJECTED = 1;
    static final int CODE_UNREADABLE = 2;
    static final int CODE_FOLDER = 3;

    private static final ExecutorService EXECUTOR = Executors.newSingleThreadExecutor();

    // 送り手の ContentProvider が書き込み端を閉じない等で読み取りが止まっても、直列キュー
    // （EXECUTOR）を塞がないための期限と上限。期限の起点は `open` の開始（単調時計）。
    // 上限は Rust 側 `MAX_FILE_BYTES`（8 MiB）+ 1 で、超過の判定（拒否）は Rust 側に任せる。
    private static final long READ_TIMEOUT_MS = 10_000L;
    private static final long READ_LIMIT_BYTES = 8L * 1024 * 1024 + 1;
    // 止まった読み取りは閉じても解除されないことがあるため、専用の daemon スレッドで行い
    // EXECUTOR とは切り離す（止まったスレッドは残るが後続の配送は進む）。
    private static final ExecutorService READER = Executors.newCachedThreadPool(r -> {
        Thread t = new Thread(r, "fandhe-file-assoc-read");
        t.setDaemon(true);
        return t;
    });

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
        final File cacheDir = context.getApplicationContext().getCacheDir();
        EXECUTOR.execute(() -> open(resolver, cacheDir, uri, intentType));
        return true;
    }

    private static void open(ContentResolver resolver, File cacheDir, Uri uri, String intentType) {
        final long deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(READ_TIMEOUT_MS);
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
        ParcelFileDescriptor pfd;
        try {
            pfd = resolver.openFileDescriptor(uri, "r");
        } catch (Exception e) {
            nativeOnFileOpened(CODE_UNREADABLE, null, null, -1);
            return;
        }
        if (pfd == null) {
            nativeOnFileOpened(CODE_UNREADABLE, null, null, -1);
            return;
        }
        // 読み取りは期限つきで一時ファイルへ写し、その fd（通常ファイル）を Rust へ渡す。
        // 期限切れの結果は成功として受け取らず UNREADABLE に確定する。
        int fd = copyWithDeadline(pfd, cacheDir, deadline);
        if (fd < 0) {
            nativeOnFileOpened(CODE_UNREADABLE, null, null, -1);
            return;
        }
        // detach 済みの fd の所有は Rust 側へ移る（Java 側は close しない）。
        nativeOnFileOpened(CODE_FILE, name, mime, fd);
    }

    /** `pfd` を `READ_LIMIT_BYTES` まで一時ファイルへ写し、読み取り専用 fd を detach して返す。失敗・期限切れは -1。 */
    private static int copyWithDeadline(ParcelFileDescriptor pfd, File cacheDir, long deadline) {
        final File tmp;
        try {
            tmp = File.createTempFile("fa-", ".tmp", cacheDir);
        } catch (Exception e) {
            closeQuietly(pfd);
            return -1;
        }
        Future<Boolean> job = READER.submit((Callable<Boolean>) () -> {
            try (InputStream in = new FileInputStream(pfd.getFileDescriptor());
                    OutputStream out = new FileOutputStream(tmp)) {
                byte[] buf = new byte[16 * 1024];
                long total = 0;
                while (total < READ_LIMIT_BYTES) {
                    int n = in.read(buf, 0, (int) Math.min(buf.length, READ_LIMIT_BYTES - total));
                    if (n < 0) {
                        break;
                    }
                    out.write(buf, 0, n);
                    total += n;
                }
                return Boolean.TRUE;
            } finally {
                closeQuietly(pfd);
            }
        });
        try {
            boolean ok = job.get(Math.max(0L, deadline - System.nanoTime()), TimeUnit.NANOSECONDS);
            if (ok) {
                // unlink しても開いた fd は読める。以後 Java 側にファイル名は残らない。
                try (ParcelFileDescriptor out = ParcelFileDescriptor.open(tmp, ParcelFileDescriptor.MODE_READ_ONLY)) {
                    return out.detachFd();
                }
            }
        } catch (Exception e) {
            // 期限切れ・読み取り失敗・割り込み。止まった読み取りの後始末は下で行う。
            job.cancel(true);
            closeQuietly(pfd);
        } finally {
            tmp.delete();
        }
        return -1;
    }

    private static void closeQuietly(ParcelFileDescriptor pfd) {
        try {
            pfd.close();
        } catch (Exception e) {
            // 閉じ済み・失敗は無視する。
        }
    }
}
