// OS 管理のバックグラウンド送信（Issue #837、親 #170）の Worker。`BackgroundUploadHelper.start` が
// `enqueueUniqueWork(REPLACE)` で予約する唯一の Worker クラス。
//
// プロセスが冷えた状態から WorkManager に起動されうるため、Rust と JNI には依存しない
// （`BackgroundTaskWorker` と同じ立場）。送るのは `BackgroundUploadHelper` が用意した固定内容の
// ファイル 1 つだけで、利用者のデータは含まない（A01）。リダイレクトは追わず、3xx は結果の
// `err=1` として Rust 側が `RedirectNotAllowed` の失敗にする。URL・例外の文言はログに出さない（A09）。
package com.fandhe.androidapp;

import android.content.Context;
import androidx.annotation.NonNull;
import androidx.work.Data;
import androidx.work.Worker;
import androidx.work.WorkerParameters;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;

public final class BackgroundUploadWorker extends Worker {
    static final String KEY_URL = "url";
    static final String KEY_SENT = "sent";
    static final String KEY_TOTAL = "total";
    static final String KEY_HTTP = "http";
    static final String KEY_ERR = "err";

    // `err` の値（`BackgroundUploadHelper` の表と同じ）。
    static final int ERR_NONE = 0;
    static final int ERR_REDIRECT = 1;
    static final int ERR_NETWORK = 2;
    static final int ERR_FILE = 3;
    static final int ERR_OTHER = 4;

    private static final int CHUNK = 64 * 1024;
    // 応答しないサーバーで WorkInfo が RUNNING のまま止まらないよう有限にする。超過は IOException（SocketTimeoutException）
    // として ERR_NETWORK の失敗に確定する。
    private static final int TIMEOUT_MS = 30_000;

    // 取り消し（onStopped）がブロック中の書き込み・読み込みを解くために握る。
    private volatile HttpURLConnection connection;

    public BackgroundUploadWorker(@NonNull Context context, @NonNull WorkerParameters params) {
        super(context, params);
    }

    private static Result finish(boolean ok, long sent, int http, int err) {
        Data out = new Data.Builder()
                .putLong(KEY_SENT, sent)
                .putInt(KEY_HTTP, http)
                .putInt(KEY_ERR, err)
                .build();
        return ok ? Result.success(out) : Result.failure(out);
    }

    @NonNull
    @Override
    public Result doWork() {
        File f = BackgroundUploadHelper.file(getApplicationContext());
        long total = BackgroundUploadHelper.FILE_SIZE;
        if (!f.exists() || f.length() != total) return finish(false, 0, -1, ERR_FILE);
        String url = getInputData().getString(KEY_URL);
        if (url == null) return finish(false, 0, -1, ERR_OTHER);
        long sent = 0;
        HttpURLConnection c = null;
        try {
            c = (HttpURLConnection) new URL(url).openConnection();
            connection = c;
            c.setConnectTimeout(TIMEOUT_MS);
            c.setReadTimeout(TIMEOUT_MS);
            c.setRequestMethod("PUT");
            c.setDoOutput(true);
            c.setFixedLengthStreamingMode(total);
            c.setInstanceFollowRedirects(false);
            c.setRequestProperty("Content-Type", "application/octet-stream");
            byte[] buf = new byte[CHUNK];
            try (InputStream in = new FileInputStream(f); OutputStream os = c.getOutputStream()) {
                int n;
                while ((n = in.read(buf)) > 0) {
                    if (isStopped()) return finish(false, sent, -1, ERR_OTHER);
                    os.write(buf, 0, n);
                    sent += n;
                    setProgressAsync(new Data.Builder()
                            .putLong(KEY_SENT, sent)
                            .putLong(KEY_TOTAL, total)
                            .build());
                }
            }
            int code = c.getResponseCode();
            boolean redirect = code >= 300 && code <= 399;
            return finish(true, sent, code, redirect ? ERR_REDIRECT : ERR_NONE);
        } catch (IOException e) {
            return finish(false, sent, -1, ERR_NETWORK);
        } catch (RuntimeException e) {
            return finish(false, sent, -1, ERR_OTHER);
        } finally {
            if (c != null) c.disconnect();
        }
    }

    @Override
    public void onStopped() {
        HttpURLConnection c = connection;
        if (c != null) c.disconnect();
    }
}
