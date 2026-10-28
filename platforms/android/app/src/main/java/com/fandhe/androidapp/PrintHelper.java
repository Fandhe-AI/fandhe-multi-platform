package com.fandhe.androidapp;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Rect;
import android.graphics.RectF;
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
// 画面の内容の印刷（#1012）は `printImage`。ARGB の int[] を受け、`onWrite` の試行ごとに Bitmap を作って
// 印刷可能領域へ縦横比を保って描き、必ず recycle する（色は不透明前提。色の正確さは守らない範囲）。
// ダイアログ・結果の写像・ライフサイクルは文章の印刷と共用する。画素・例外はログに出さない。
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

    // 1 ページの描画を差し替える（文章 / 画像）。
    private interface PageDrawer {
        void draw(PdfDocument.Page page);
    }

    // JNI スレッドから呼ばれる。
    void print(long id, String title, String body) {
        request.track(id);
        activity.runOnUiThread(() -> start(id, page -> draw(page, title, body)));
    }

    // JNI スレッドから呼ばれる。
    void printImage(long id, int[] argb, int width, int height) {
        request.track(id);
        activity.runOnUiThread(() -> start(id, page -> drawImage(page, argb, width, height)));
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

    private void start(long id, PageDrawer drawer) {
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
            job = manager.print("fandhe", new Adapter(id, drawer), null);
        } catch (RuntimeException e) {
            finish(id, RESULT_PRESENT_FAILED);
            return;
        }
        // 印刷ジョブを作れない・ダイアログを開けないと null が返り、adapter の onFinish も来ない。
        if (job == null) {
            finish(id, RESULT_PRESENT_FAILED);
        }
    }

    // `onFinish` の後に結果を確定する。CREATED のままなら 2 秒まで再確認する。
    private void evaluate(long id) {
        if (request.current() != id) {
            return;
        }
        PrintJob j = job;
        // 取り消しを失敗より先に見る（利用者の取り消しを失敗に化けさせない。契約表 10）。
        // writeFailed は直近の onWrite 試行の結果で、再試行が成功すれば false に戻っている。
        if (j != null && j.isCancelled()) {
            finish(id, RESULT_CANCELLED);
        } else if (writeFailed || (j != null && j.isFailed())) {
            finish(id, RESULT_FAILED);
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

    // 画像を印刷可能領域へ縦横比を保って中央寄せで収める。Bitmap はこの呼び出しの中だけで生かす。
    private static void drawImage(PdfDocument.Page page, int[] argb, int w, int h) {
        Bitmap bmp = Bitmap.createBitmap(argb, w, h, Bitmap.Config.ARGB_8888);
        try {
            // Canvas の原点は contentRect の左上（ページの角ではない）。根拠: AOSP の
            // PdfDocument.cpp は contentRect の幅・高さで原点 (0,0) から記録を始め、書き出し時に
            // beginPage(.., &contentRect) で印刷可能領域へ配置する。よって left/top は足さず、
            // area の幅・高さ内の余白だけで中央寄せする（足すと余白の二重加算）。
            Rect area = page.getInfo().getContentRect();
            float s = Math.min(area.width() / (float) w, area.height() / (float) h);
            float dw = w * s;
            float dh = h * s;
            float left = (area.width() - dw) / 2f;
            float top = (area.height() - dh) / 2f;
            page.getCanvas().drawBitmap(bmp, null, new RectF(left, top, left + dw, top + dh),
                    new Paint(Paint.FILTER_BITMAP_FLAG));
        } finally {
            bmp.recycle();
        }
    }

    // UI スレッドで呼ばれる（コールバックのハンドラ既定）。
    private final class Adapter extends PrintDocumentAdapter {
        private final long id;
        private PageDrawer drawer;
        private PrintAttributes attrs;

        Adapter(long id, PageDrawer drawer) {
            this.id = id;
            this.drawer = drawer;
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
            // 試行ごとに状態を作り直す。
            writeFailed = false;
            PageDrawer d = drawer;
            if (d == null) {
                writeFailed = true;
                callback.onWriteFailed(null);
                return;
            }
            PrintedPdfDocument pdf = new PrintedPdfDocument(activity, attrs);
            boolean ok = false;
            try {
                PdfDocument.Page page = pdf.startPage(0);
                try {
                    d.draw(page);
                } finally {
                    // 描画の成否によらずページを終える（未終了だと close が IllegalStateException）。
                    pdf.finishPage(page);
                }
                // destination の fd は framework の持ち物なので閉じない（公式サンプルと同じ）。
                pdf.writeTo(new FileOutputStream(destination.getFileDescriptor()));
                ok = true;
            } catch (IOException | RuntimeException | OutOfMemoryError e) {
                ok = false;
            } finally {
                try {
                    pdf.close();
                } catch (RuntimeException e) {
                    ok = false;
                }
            }
            if (ok) {
                callback.onWriteFinished(new PageRange[] {PageRange.ALL_PAGES});
            } else {
                writeFailed = true;
                callback.onWriteFailed(null);
            }
        }

        @Override
        public void onFinish() {
            finishedAt = SystemClock.elapsedRealtime();
            // 印刷フレームワークが adapter を握り続けても画素配列を解放させる。
            drawer = null;
            evaluate(id);
        }
    }
}
