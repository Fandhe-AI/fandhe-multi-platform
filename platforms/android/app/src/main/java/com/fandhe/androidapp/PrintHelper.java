package com.fandhe.androidapp;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Typeface;
import android.graphics.pdf.PdfDocument;
import android.os.Bundle;
import android.os.CancellationSignal;
import android.os.Handler;
import android.os.Looper;
import android.os.ParcelFileDescriptor;
import android.os.SystemClock;
import android.print.PageRange;
import android.print.PrintAttributes;
import android.print.PrintDocumentAdapter;
import android.print.PrintDocumentInfo;
import android.print.PrintJob;
import android.print.PrintManager;
import android.print.pdf.PrintedPdfDocument;

import androidx.activity.ComponentActivity;
import androidx.lifecycle.Lifecycle;

import java.io.FileOutputStream;
import java.io.IOException;

// 印刷ダイアログ（Issue #328、親 #94）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_print.rs` が `MainActivity.requestPrint` を JNI で呼び、結果を
// `nativeOnPrintResult` で受けて `print::outcome_from_android_code` で契約の結果へ写像する）。
//
// `PrintManager.print` に `PrintDocumentAdapter` を渡し、`PrintedPdfDocument` で 1 ページ描く。
// 印刷 UI は別 Activity なので提示直後に必ず `onPause` が来る。よって `onPause` では取り消さない。
// 結果は `onFinish` の後に `PrintJob` の状態で決める。CREATED のまま残る場合に備え、`onFinish` から
// 2 秒（単調時計）だけ再確認し、それでも CREATED なら UNCONFIRMED（嘘の成功にしない）。
// 利用者が操作するダイアログなので固定の期限は掛けない。
// ライフサイクル: `request` はインスタンスフィールド。`onDestroy` で結果は届けず破棄し、Rust 側の
// `reset_pending_tracking()` と id 照合で旧要求を捨てる。破棄後に届く `onFinish` は `complete` が
// false になり何も送らない。
// 守る範囲: 1 プロセス内の要求順序・取り消しを失敗にしないこと・嘘の成功の禁止・資源の解放。
// 守らない範囲: 紙に出たか、2 秒を超えてからの状態変化、プリンタ側の失敗。
// 権限・`<queries>` は宣言しない。ジョブ名は固定で、見出し・本文・例外はログにも OS のキューにも出さない。
// no-op スタブは置かない: native は Rust の `requestPrint` を経た後でしか呼ばれない。
final class PrintHelper {
    // 結果コードの SSOT。Rust 側 `print::outcome_from_android_code` が同じ値で写像する。
    static final int RESULT_SUBMITTED = 0;
    static final int RESULT_CANCELLED = 1;
    static final int RESULT_UNCONFIRMED = 2;
    static final int RESULT_FAILED = 3;
    static final int RESULT_PRESENT_FAILED = 4;

    private static final long RECHECK_WINDOW_MS = 2000L;
    private static final long RECHECK_INTERVAL_MS = 100L;

    private static native void nativeOnPrintResult(long requestId, int code);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    private final Handler handler = new Handler(Looper.getMainLooper());
    // 以下は UI スレッドだけが触る。
    private PrintJob job;
    private boolean writeFailed;
    private long finishedAt;

    PrintHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // JNI スレッドから呼ばれる。
    void print(long id, String title, String body) {
        request.track(id);
        activity.runOnUiThread(() -> start(id, title, body));
    }

    // UI スレッド。
    void onDestroy() {
        handler.removeCallbacksAndMessages(null);
        request.take();
        job = null;
    }

    private void finish(long id, int code) {
        if (request.complete(id)) {
            job = null;
            nativeOnPrintResult(id, code);
        }
    }

    private void start(long id, String title, String body) {
        // 差し替え済みの旧要求は何もしない。
        if (request.current() != id) {
            return;
        }
        if (activity.isFinishing() || activity.isDestroyed()
                || !activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            finish(id, RESULT_PRESENT_FAILED);
            return;
        }
        PrintManager manager = (PrintManager) activity.getSystemService(Context.PRINT_SERVICE);
        if (manager == null) {
            finish(id, RESULT_PRESENT_FAILED);
            return;
        }
        writeFailed = false;
        finishedAt = 0L;
        try {
            job = manager.print("fandhe", new Adapter(id, title, body), null);
        } catch (RuntimeException e) {
            finish(id, RESULT_PRESENT_FAILED);
        }
    }

    // `onFinish` の後に結果を確定する。CREATED のままなら 2 秒まで再確認する。
    private void evaluate(long id) {
        if (request.current() != id) {
            return;
        }
        PrintJob j = job;
        if (writeFailed || (j != null && j.isFailed())) {
            finish(id, RESULT_FAILED);
        } else if (j != null && j.isCancelled()) {
            finish(id, RESULT_CANCELLED);
        } else if (j != null && (j.isQueued() || j.isStarted() || j.isBlocked()
                || j.isCompleted())) {
            finish(id, RESULT_SUBMITTED);
        } else if (SystemClock.elapsedRealtime() - finishedAt < RECHECK_WINDOW_MS) {
            handler.postDelayed(() -> evaluate(id), RECHECK_INTERVAL_MS);
        } else {
            finish(id, RESULT_UNCONFIRMED);
        }
    }

    // 見出し（太字・大きめ）と本文（`\n` 区切りの行）を 1 ページに描く。
    private static void draw(PdfDocument.Page page, String title, String body) {
        Canvas canvas = page.getCanvas();
        Paint head = new Paint();
        head.setAntiAlias(true);
        head.setTypeface(Typeface.DEFAULT_BOLD);
        head.setTextSize(20f);
        Paint text = new Paint();
        text.setAntiAlias(true);
        text.setTextSize(12f);
        float x = 36f;
        float y = 56f;
        canvas.drawText(title, x, y, head);
        y += 32f;
        for (String line : body.split("\n", -1)) {
            canvas.drawText(line, x, y, text);
            y += 18f;
        }
    }

    // UI スレッドで呼ばれる（コールバックのハンドラ既定）。
    private final class Adapter extends PrintDocumentAdapter {
        private final long id;
        private final String title;
        private final String body;
        private PrintAttributes attrs;

        Adapter(long id, String title, String body) {
            this.id = id;
            this.title = title;
            this.body = body;
        }

        @Override
        public void onLayout(PrintAttributes oldAttrs, PrintAttributes newAttrs,
                CancellationSignal signal, LayoutResultCallback callback, Bundle extras) {
            if (signal.isCanceled()) {
                callback.onLayoutCancelled();
                return;
            }
            attrs = newAttrs;
            PrintDocumentInfo info = new PrintDocumentInfo.Builder("fandhe-print.pdf")
                    .setContentType(PrintDocumentInfo.CONTENT_TYPE_DOCUMENT)
                    .setPageCount(1)
                    .build();
            callback.onLayoutFinished(info, !newAttrs.equals(oldAttrs));
        }

        @Override
        public void onWrite(PageRange[] pages, ParcelFileDescriptor destination,
                CancellationSignal signal, WriteResultCallback callback) {
            if (signal.isCanceled()) {
                callback.onWriteCancelled();
                return;
            }
            PrintedPdfDocument pdf = new PrintedPdfDocument(activity, attrs);
            try {
                PdfDocument.Page page = pdf.startPage(0);
                draw(page, title, body);
                pdf.finishPage(page);
                // destination の fd は framework の持ち物なので閉じない（公式サンプルと同じ）。
                pdf.writeTo(new FileOutputStream(destination.getFileDescriptor()));
                callback.onWriteFinished(new PageRange[] {PageRange.ALL_PAGES});
            } catch (IOException | RuntimeException e) {
                writeFailed = true;
                callback.onWriteFailed(null);
            } finally {
                pdf.close();
            }
        }

        @Override
        public void onFinish() {
            finishedAt = SystemClock.elapsedRealtime();
            evaluate(id);
        }
    }
}
