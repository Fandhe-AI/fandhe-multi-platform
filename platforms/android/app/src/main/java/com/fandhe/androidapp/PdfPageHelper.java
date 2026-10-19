package com.fandhe.androidapp;

import android.graphics.Bitmap;
import android.graphics.Color;
import android.graphics.pdf.PdfRenderer;
import android.os.ParcelFileDescriptor;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.ByteBuffer;

// PDF 表示画面（Issue #472、親 #107 の b-android）の Android 実装ヘルパー。
// Rust の `device/android_pdf_viewer.rs` がドライバのワーカースレッドから
// `MainActivity.pdfRenderFirstPage` 経由で同期 JNI で呼ぶ。Rust への native コールバックは無く、
// JNI 署名の照合先は Java の 1 か所だけ。
// 扱う範囲は「PDF のバイト列の 1 ページ目を `PdfRenderer` で長辺 maxSide 以内の RGBA に描く」ことだけ。
//
// コード表の SSOT は Rust の `fandhe_device::pdf_viewer::decode_code`。同じ値で持つ（1=非対応は Android では返さない）。
//   戻り値 byte[]: 成功 = [0][pageCount:int32 BE][width:int32 BE][height:int32 BE][RGBA]
//                  失敗 = コード 1 バイトだけ / 想定外の例外 = null（Rust 側で Internal）
//   成功の妥当性は Rust の `check_page` が決める（Java は成功を判定しない）。
//
// スレッド: 呼び出し元スレッドで同期実行する（main Looper は使わない）。状態を持たない static のみ。
// 期限は Rust のドライバだけが持ち、ここは走り切る。ライフサイクル: 短い処理なので onPause / onDestroy では中断しない。
// 守る範囲: 全経路での資源解放・失敗を成功に畳まないこと。守らない範囲: 任意の PDF、端末ごとの描画差。
// PDF・画素・例外メッセージはログに出さない。
final class PdfPageHelper {
    private static final int OK = 0;
    private static final int BROKEN = 2;
    private static final int ENCRYPTED = 3;
    private static final int EMPTY = 4;
    private static final int SIZE = 5;
    private static final int MAX_SIDE = 1024;
    private static final int MAX_PDF_BYTES = 1 << 20;

    private PdfPageHelper() {}

    static byte[] renderFirstPage(File cacheDir, byte[] pdf, int maxSide) {
        if (pdf == null || pdf.length == 0 || pdf.length > MAX_PDF_BYTES
                || maxSide < 1 || maxSide > MAX_SIDE || cacheDir == null) {
            return code(SIZE);
        }
        File tmp = null;
        ParcelFileDescriptor pfd = null;
        PdfRenderer renderer = null;
        PdfRenderer.Page page = null;
        Bitmap bmp = null;
        try {
            // PdfRenderer はシーク可能な fd を要るため、アプリ専用のキャッシュへ一時ファイルとして書く。
            tmp = File.createTempFile("pdf", ".pdf", cacheDir);
            try (FileOutputStream out = new FileOutputStream(tmp)) {
                out.write(pdf);
            }
            pfd = ParcelFileDescriptor.open(tmp, ParcelFileDescriptor.MODE_READ_ONLY);
            renderer = new PdfRenderer(pfd);
            int pages = renderer.getPageCount();
            if (pages <= 0) {
                return code(EMPTY);
            }
            page = renderer.openPage(0);
            int w = page.getWidth();
            int h = page.getHeight();
            if (w <= 0 || h <= 0) {
                return code(SIZE);
            }
            // 式は Rust の `render_size` と同じ（長辺を maxSide に合わせ、四捨五入して 1..=maxSide）。
            double scale = (double) maxSide / Math.max(w, h);
            int fw = (int) Math.min(maxSide, Math.max(1, Math.round(w * scale)));
            int fh = (int) Math.min(maxSide, Math.max(1, Math.round(h * scale)));
            bmp = Bitmap.createBitmap(fw, fh, Bitmap.Config.ARGB_8888);
            // PdfRenderer は背景を塗らないため、白で埋めてから描く。
            bmp.eraseColor(Color.WHITE);
            page.render(bmp, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY);
            // ARGB_8888 のメモリ上の並びは R,G,B,A。背景が不透明なので premultiplied の影響は無い。
            ByteBuffer px = ByteBuffer.allocate(fw * fh * 4);
            bmp.copyPixelsToBuffer(px);
            ByteBuffer res = ByteBuffer.allocate(13 + fw * fh * 4); // 既定は BE
            res.put((byte) OK).putInt(pages).putInt(fw).putInt(fh).put(px.array());
            return res.array();
        } catch (SecurityException e) {
            return code(ENCRYPTED);
        } catch (IOException | IllegalArgumentException e) {
            return code(BROKEN);
        } catch (Exception | OutOfMemoryError e) {
            return null;
        } finally {
            // 各ステップを個別に囲み、どれかが失敗しても残りを解放する。
            if (page != null) {
                try { page.close(); } catch (Exception ignored) { }
            }
            if (renderer != null) {
                try { renderer.close(); } catch (Exception ignored) { }
            }
            if (pfd != null) {
                try { pfd.close(); } catch (Exception ignored) { }
            }
            if (bmp != null) {
                bmp.recycle();
            }
            if (tmp != null) {
                //noinspection ResultOfMethodCallIgnored
                tmp.delete();
            }
        }
    }

    private static byte[] code(int c) {
        return new byte[] {(byte) c};
    }
}
