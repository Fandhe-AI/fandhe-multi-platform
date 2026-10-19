package com.fandhe.androidapp;

import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructPollfd;

import android.database.Cursor;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.SynchronousQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

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
// ContentProvider 呼び出し全体（getType・query・open・読み取り）を有界の worker プール（同時 4）で
// 行い、配送役（直列キュー）は期限（10 秒）まで待って結果を順に渡す。止まった送り手は配送を塞がず、
// 期限切れは UNREADABLE に確定する。読みは poll で待ち、期限後は worker 自身が fd・一時ファイルを解放する。
// 守る範囲: 1 プロセス内の受信順、結果コードの嘘の成功の禁止。
// 守らない範囲: プロセス死後の再表示、読み取り許可の無い URI（UNREADABLE になる）、複数プロセス。
// URI・名前・例外メッセージ・中身はログへ出さない（security.md A09）。
final class FileAssociationHelper {
    static final int CODE_FILE = 0;
    static final int CODE_REJECTED = 1;
    static final int CODE_UNREADABLE = 2;
    static final int CODE_FOLDER = 3;

    // 受信順を保つ配送役。実処理の待ちは MAX_PENDING 件まで。溢れた要求は処理を持たない
    // 失敗通知として同じキューの末尾に積み、先行要求より先に結果が届かないようにする。
    private static final ExecutorService EXECUTOR =
            new ThreadPoolExecutor(1, 1, 0L, TimeUnit.MILLISECONDS, new LinkedBlockingQueue<>());
    private static final int MAX_PENDING = 32;
    private static final AtomicInteger PENDING = new AtomicInteger();
    // 末尾が溢れ通知のとき、連続する溢れ要求は 1 件の通知にまとめる（キューの肥大を防ぐ）。
    private static boolean tailIsOverflow;

    // ContentProvider 呼び出し（getType・query・openFileDescriptor・読み取り）全体の期限。
    // 期限の起点は配送役が worker を起動した時点（単調時計）。
    private static final long READ_TIMEOUT_MS = 10_000L;
    // 上限は Rust 側 `MAX_FILE_BYTES`（8 MiB）+ 1 で、超過の判定（拒否）は Rust 側に任せる。
    private static final long READ_LIMIT_BYTES = 8L * 1024 * 1024 + 1;
    private static final int POLL_SLICE_MS = 100;
    // provider の呼び出しは中断できないため、止まった worker は戻るまで残る。同時数を 4 に
    // 絞り（SynchronousQueue で待機もさせない）、満杯なら UNREADABLE にして蓄積を防ぐ。
    private static final ExecutorService READER = new ThreadPoolExecutor(0, 4, 30L, TimeUnit.SECONDS,
            new SynchronousQueue<>(), r -> {
                Thread t = new Thread(r, "fandhe-file-assoc-read");
                t.setDaemon(true);
                return t;
            });

    /** worker の結果。配送役が受け取るか、期限切れ後に worker 自身が破棄する。 */
    private static final class Result {
        final int code;
        final String name;
        final String mime;
        final int fd;

        Result(int code, String name, String mime, int fd) {
            this.code = code;
            this.name = name;
            this.mime = mime;
            this.fd = fd;
        }
    }

    private static final int RUNNING = 0;
    private static final int DONE = 1;
    private static final int ABANDONED = 2;

    /** 配送役と worker の受け渡し。状態の遷移は CAS で一度きり。 */
    private static final class Job {
        final AtomicInteger state = new AtomicInteger(RUNNING);
        volatile Result result;
        final CountDownLatch latch = new CountDownLatch(1);
    }

    private FileAssociationHelper() {}

    static native void nativeOnFileOpened(int code, String name, String mime, int fd);

    /** 共通層 `fandhe_device::file_association::accepts` の判定。読み取り前に拒否を確定するために使う。 */
    static native boolean nativeAccepts(String name, String mime);

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
            submit(() -> nativeOnFileOpened(CODE_REJECTED, null, null, -1));
            return true;
        }
        if (!"content".equals(scheme)) {
            return false;
        }
        final ContentResolver resolver = context.getApplicationContext().getContentResolver();
        final String intentType = intent.getType();
        final File cacheDir = context.getApplicationContext().getCacheDir();
        submit(() -> dispatch(resolver, cacheDir, uri, intentType));
        return true;
    }

    private static synchronized void submit(Runnable r) {
        if (PENDING.get() >= MAX_PENDING) {
            if (!tailIsOverflow) {
                tailIsOverflow = true;
                EXECUTOR.execute(() -> nativeOnFileOpened(CODE_UNREADABLE, null, null, -1));
            }
            return;
        }
        tailIsOverflow = false;
        PENDING.incrementAndGet();
        EXECUTOR.execute(() -> {
            try {
                r.run();
            } finally {
                PENDING.decrementAndGet();
            }
        });
    }

    /** 配送役。worker に全処理を任せ、期限内の結果だけを順に Rust へ渡す。 */
    private static void dispatch(ContentResolver resolver, File cacheDir, Uri uri, String intentType) {
        final Job job = new Job();
        final long deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(READ_TIMEOUT_MS);
        try {
            READER.execute(() -> work(job, resolver, cacheDir, uri, intentType, deadline));
        } catch (RejectedExecutionException e) {
            nativeOnFileOpened(CODE_UNREADABLE, null, null, -1);
            return;
        }
        try {
            job.latch.await(Math.max(0L, deadline - System.nanoTime()), TimeUnit.NANOSECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        if (job.state.compareAndSet(RUNNING, ABANDONED)) {
            // 期限切れ。以後の後始末（fd・一時ファイル）は worker が行う。
            nativeOnFileOpened(CODE_UNREADABLE, null, null, -1);
            return;
        }
        Result r = job.result;
        nativeOnFileOpened(r.code, r.name, r.mime, r.fd);
    }

    private static void work(Job job, ContentResolver resolver, File cacheDir, Uri uri, String intentType,
            long deadline) {
        Result r;
        try {
            r = open(job, resolver, cacheDir, uri, intentType, deadline);
        } catch (Throwable t) {
            r = new Result(CODE_UNREADABLE, null, null, -1);
        }
        job.result = r;
        if (job.state.compareAndSet(RUNNING, DONE)) {
            job.latch.countDown();
        } else if (r.fd >= 0) {
            // 期限切れ後に完了した結果。fd の所有者がいないので閉じる。
            closeQuietly(ParcelFileDescriptor.adoptFd(r.fd));
        }
    }

    private static Result open(Job job, ContentResolver resolver, File cacheDir, Uri uri, String intentType,
            long deadline) {
        String mime = intentType;
        try {
            String type = resolver.getType(uri);
            if (DocumentsContract.Document.MIME_TYPE_DIR.equals(type)) {
                return new Result(CODE_FOLDER, null, null, -1);
            }
            if (type != null) {
                mime = type;
            }
        } catch (RuntimeException e) {
            return new Result(CODE_UNREADABLE, null, null, -1);
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
        if (name == null || !nativeAccepts(name, mime)) {
            // 名前なし・受理されない名前や MIME はファイルを開く前に拒否する。読み取りの成否で
            // UNREADABLE に化けると拒否が数えられず、直前の latest が置き換わってしまう。
            return new Result(CODE_REJECTED, null, null, -1);
        }
        ParcelFileDescriptor pfd;
        try {
            pfd = resolver.openFileDescriptor(uri, "r");
        } catch (Exception e) {
            return new Result(CODE_UNREADABLE, null, null, -1);
        }
        if (pfd == null) {
            return new Result(CODE_UNREADABLE, null, null, -1);
        }
        int fd = copyWithDeadline(job, pfd, cacheDir, deadline);
        if (fd < 0) {
            return new Result(CODE_UNREADABLE, null, null, -1);
        }
        // detach 済みの fd の所有は Rust 側へ移る（Java 側は close しない）。
        return new Result(CODE_FILE, name, mime, fd);
    }

    /**
     * `pfd` を `READ_LIMIT_BYTES` まで一時ファイルへ写し、読み取り専用 fd を detach して返す。
     * 失敗・期限切れは -1。読みは poll で待つ非ブロッキング方式で、期限・破棄を見たら自分で終了して
     * fd と一時ファイルを解放する。
     */
    private static int copyWithDeadline(Job job, ParcelFileDescriptor pfd, File cacheDir, long deadline) {
        File tmp = null;
        try {
            tmp = File.createTempFile("fa-", ".tmp", cacheDir);
            try (InputStream in = new FileInputStream(pfd.getFileDescriptor());
                    OutputStream out = new FileOutputStream(tmp)) {
                StructPollfd[] pfds = {new StructPollfd()};
                pfds[0].fd = pfd.getFileDescriptor();
                pfds[0].events = (short) OsConstants.POLLIN;
                byte[] buf = new byte[16 * 1024];
                long total = 0;
                while (total < READ_LIMIT_BYTES) {
                    if (job.state.get() == ABANDONED || System.nanoTime() - deadline >= 0) {
                        return -1;
                    }
                    pfds[0].revents = 0;
                    if (Os.poll(pfds, POLL_SLICE_MS) == 0) {
                        continue;
                    }
                    int n = in.read(buf, 0, (int) Math.min(buf.length, READ_LIMIT_BYTES - total));
                    if (n < 0) {
                        // reliable pipe の書き側が closeWithError した場合も EOF になるため、
                        // 途中切れを成功にしないよう provider のエラーを例外へ変換する。
                        pfd.checkError();
                        break;
                    }
                    out.write(buf, 0, n);
                    total += n;
                }
            }
            // unlink しても開いた fd は読める。以後 Java 側にファイル名は残らない。
            try (ParcelFileDescriptor ro = ParcelFileDescriptor.open(tmp, ParcelFileDescriptor.MODE_READ_ONLY)) {
                return ro.detachFd();
            }
        } catch (Exception e) {
            return -1;
        } finally {
            closeQuietly(pfd);
            if (tmp != null) {
                tmp.delete();
            }
        }
    }

    private static void closeQuietly(ParcelFileDescriptor pfd) {
        try {
            pfd.close();
        } catch (Exception e) {
            // 閉じ済み・失敗は無視する。
        }
    }
}
