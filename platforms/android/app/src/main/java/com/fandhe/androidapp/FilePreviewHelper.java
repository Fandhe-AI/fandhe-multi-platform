package com.fandhe.androidapp;

import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.Intent;
import android.graphics.Bitmap;
import android.net.Uri;
import android.os.Build;
import android.os.CancellationSignal;
import android.os.OperationCanceledException;
import android.util.Size;

import androidx.activity.ComponentActivity;
import androidx.core.content.FileProvider;
import androidx.lifecycle.Lifecycle;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

// ファイル・動画のプレビューとサムネイル機能（Issue #920、親 #182 の b-android）の Android 実装ヘルパー。
// Rust の `device/android_file_preview.rs` が呼ぶ。MainActivity は呼び出しと委譲だけを持つ。
//
// 扱う範囲は Rust が渡す固定ファイル 2 種（kind 0 = サンプル画像 PNG、1 = 種類不明の .bin）だけ。
// ファイル名・MIME は Java の固定値で決め、外からパス・名前・MIME を受けない。
//
// サムネイル（static・同期 JNI。Rust のワーカースレッドから呼ぶ）:
//   `ThumbnailUtils.createImageThumbnail(File, Size, CancellationSignal)`（API 29+）。API 29 未満は非対応。
//   戻り byte[] のコード表の SSOT は Rust の `fandhe_device::file_preview::thumbnail_from_apple_code`
//   （Apple と同じ表）。成功 = [0][w:int32 BE][h:int32 BE][RGBA]、失敗 = コード 1 バイト
//   （1 非対応 / 2 サムネイルなし / 3 期限切れ / 4 引数・寸法の不正 / 5 その他）。null は Rust 側で Internal。
//   成功の判定は Rust の `check_thumbnail` が行う（Java は OS の結果を契約の形へ合わせるだけ）。
//   期限は専用の daemon スレッドで CancellationSignal を取り消す（main Looper を使わないので UI キューの
//   遅延で OS 側の期限がずれない）。OS 呼び出しは同期なので、戻った時点で取り消しは済んでいる。
//
// プレビュー（インスタンス・非同期）: FileProvider（既存の `${applicationId}.compose` を共用）の content URI を
//   `ACTION_VIEW` + `FLAG_GRANT_READ_URI_PERMISSION` で他アプリへ渡す。結果は `HandedOff`（startActivity が戻った
//   だけで、表示された保証ではない）/ `NoHandler`（ActivityNotFoundException）まで。期限は持たない。
//   `resolveActivity` は呼ばず、`<queries>` と権限は宣言しない。
//   ライフサイクル: `request` はインスタンスフィールド。再生成後の新インスタンスは NONE から始まり、
//   旧インスタンスの結果は Rust 側 `reset_pending_tracking()` と id 照合で捨てられる。
//
// 置き場: `getCacheDir()/file_preview/<固定名>`。一時名へ書いてから rename で置き換える（読み途中の受け手は
// 旧 inode を読み切る）。同名上書きなので溜まらず、削除の予約は置かない。
// 守る範囲: 全経路での資源解放・失敗を成功に畳まないこと。守らない範囲: 受け手アプリが実際に表示したか、
// 色・アルファの正確さ、所要時間。パス・画素・id・結果コード・例外メッセージはログに出さない。
final class FilePreviewHelper {
    // kind コード。Rust 側 `android_file_preview.rs` の KIND_* と対（`FileKind::file_name` とも対）。
    private static final int KIND_SAMPLE_IMAGE = 0;
    private static final int KIND_UNKNOWN = 1;

    // サムネイルのコード表（上記 SSOT と同じ値）。
    private static final int T_OK = 0;
    private static final int T_UNSUPPORTED = 1;
    private static final int T_NONE = 2;
    private static final int T_TIMEOUT = 3;
    private static final int T_SIZE = 4;
    private static final int T_OTHER = 5;

    // プレビューのコード表の SSOT は Rust の `preview_from_android_code`。
    static final int RESULT_HANDED_OFF = 0;
    static final int RESULT_NO_HANDLER = 1;
    static final int RESULT_BACKGROUND = 2;
    static final int RESULT_PRESENT_FAILED = 3;

    private static final int MAX_SIDE = 256;
    private static final int MAX_BYTES = 1 << 20;
    private static final long MAX_TIMEOUT_MS = 60_000L;

    private static final ScheduledExecutorService TIMER =
            Executors.newSingleThreadScheduledExecutor(r -> {
                Thread t = new Thread(r, "file-preview-timer");
                t.setDaemon(true);
                return t;
            });

    private static native void nativeOnFilePreviewResult(long requestId, int code);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();

    FilePreviewHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // ---- サムネイル ----

    static byte[] thumbnail(File cacheDir, int kind, byte[] data, int maxSide, long timeoutMs) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return code(T_UNSUPPORTED);
        }
        if (cacheDir == null || (kind != KIND_SAMPLE_IMAGE && kind != KIND_UNKNOWN)
                || data == null || data.length == 0 || data.length > MAX_BYTES
                || maxSide < 1 || maxSide > MAX_SIDE || timeoutMs < 0) {
            return code(T_SIZE);
        }
        File file = writeFixed(cacheDir, kind, data);
        if (file == null) {
            return code(T_OTHER);
        }
        return thumbnailQ(file, maxSide, Math.min(timeoutMs, MAX_TIMEOUT_MS));
    }

    @android.annotation.TargetApi(Build.VERSION_CODES.Q)
    private static byte[] thumbnailQ(File file, int maxSide, long timeoutMs) {
        CancellationSignal signal = new CancellationSignal();
        ScheduledFuture<?> timer = TIMER.schedule(signal::cancel, timeoutMs, TimeUnit.MILLISECONDS);
        Bitmap bmp = null;
        Bitmap scaled = null;
        Bitmap argb = null;
        try {
            bmp = android.media.ThumbnailUtils.createImageThumbnail(
                    file, new Size(maxSide, maxSide), signal);
            if (bmp == null) {
                return code(T_OTHER);
            }
            Bitmap cur = bmp;
            int w = cur.getWidth();
            int h = cur.getHeight();
            if (w <= 0 || h <= 0) {
                return code(T_SIZE);
            }
            // OS は整数のサンプリング比で縮めるため長辺が maxSide を超えうる。超えたときだけ縮める
            // （式は PdfPageHelper と同じ丸め）。
            if (Math.max(w, h) > maxSide) {
                double scale = (double) maxSide / Math.max(w, h);
                int fw = (int) Math.min(maxSide, Math.max(1, Math.round(w * scale)));
                int fh = (int) Math.min(maxSide, Math.max(1, Math.round(h * scale)));
                scaled = Bitmap.createScaledBitmap(cur, fw, fh, true);
                cur = scaled;
            }
            if (cur.getConfig() != Bitmap.Config.ARGB_8888) {
                argb = cur.copy(Bitmap.Config.ARGB_8888, false);
                if (argb == null) {
                    return code(T_OTHER);
                }
                cur = argb;
            }
            int fw = cur.getWidth();
            int fh = cur.getHeight();
            // ARGB_8888 のメモリ上の並びは R,G,B,A。
            ByteBuffer px = ByteBuffer.allocate(fw * fh * 4);
            cur.copyPixelsToBuffer(px);
            ByteBuffer res = ByteBuffer.allocate(9 + fw * fh * 4); // 既定は BE
            res.put((byte) T_OK).putInt(fw).putInt(fh).put(px.array());
            return res.array();
        } catch (IOException | OperationCanceledException e) {
            // 公式は取り消しも IOException と記すが、実装は OperationCanceledException を投げる経路を持つ。
            return code(signal.isCanceled() ? T_TIMEOUT : T_NONE);
        } catch (RuntimeException | OutOfMemoryError e) {
            return code(T_OTHER);
        } finally {
            timer.cancel(false);
            if (argb != null) {
                argb.recycle();
            }
            if (scaled != null) {
                scaled.recycle();
            }
            if (bmp != null) {
                bmp.recycle();
            }
        }
    }

    // ---- プレビュー ----

    // JNI スレッドから呼ばれる。
    void preview(long id, int kind, byte[] data) {
        request.track(id);
        activity.runOnUiThread(() -> start(id, kind, data));
    }

    private void start(long id, int kind, byte[] data) {
        // 差し替え済みの旧要求は結果を返さず捨てる。
        if (!request.complete(id)) {
            return;
        }
        if (activity.isFinishing() || activity.isDestroyed()
                || !activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            nativeOnFilePreviewResult(id, RESULT_BACKGROUND);
            return;
        }
        if (kind != KIND_SAMPLE_IMAGE || data == null || data.length == 0 || data.length > MAX_BYTES) {
            nativeOnFilePreviewResult(id, RESULT_PRESENT_FAILED);
            return;
        }
        int code;
        try {
            File f = writeFixed(activity.getCacheDir(), kind, data);
            if (f == null) {
                code = RESULT_PRESENT_FAILED;
            } else {
                Uri uri = FileProvider.getUriForFile(activity, activity.getPackageName() + ".compose", f);
                Intent intent = new Intent(Intent.ACTION_VIEW)
                        .setDataAndType(uri, "image/png")
                        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                intent.setClipData(ClipData.newRawUri("", uri));
                activity.startActivity(intent);
                code = RESULT_HANDED_OFF;
            }
        } catch (ActivityNotFoundException e) {
            code = RESULT_NO_HANDLER;
        } catch (RuntimeException e) {
            code = RESULT_PRESENT_FAILED;
        }
        nativeOnFilePreviewResult(id, code);
    }

    // ---- 共用 ----

    // 固定名で書く。一時名へ書いてから rename で置き換える。失敗は null（一時ファイルは消す）。
    private static File writeFixed(File cacheDir, int kind, byte[] data) {
        if (cacheDir == null) {
            return null;
        }
        // Rust の `FileKind::file_name` と同じ名前。
        String name = kind == KIND_SAMPLE_IMAGE ? "fandhe-sample.png" : "fandhe-unknown.bin";
        File dir = new File(cacheDir, "file_preview");
        File tmp = null;
        try {
            if (!dir.isDirectory() && !dir.mkdirs() && !dir.isDirectory()) {
                return null;
            }
            tmp = File.createTempFile("fp", ".tmp", dir);
            try (FileOutputStream out = new FileOutputStream(tmp)) {
                out.write(data);
            }
            File dest = new File(dir, name);
            if (!tmp.renameTo(dest)) {
                return null;
            }
            tmp = null;
            return dest;
        } catch (IOException | RuntimeException e) {
            return null;
        } finally {
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
