package com.fandhe.androidapp;

import android.app.Activity;
import android.content.Intent;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.net.Uri;
import android.os.Handler;
import android.os.Looper;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResult;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.IntentSenderRequest;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;
import androidx.lifecycle.Lifecycle;

import com.google.android.gms.common.ConnectionResult;
import com.google.android.gms.common.GoogleApiAvailability;
import com.google.mlkit.common.MlKitException;
import com.google.mlkit.vision.documentscanner.GmsDocumentScannerOptions;
import com.google.mlkit.vision.documentscanner.GmsDocumentScanning;
import com.google.mlkit.vision.documentscanner.GmsDocumentScanningResult;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.lang.ref.WeakReference;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

// 書類スキャン（Issue #1125、親 #145 の b-android）の Android 実装ヘルパー。範囲は ML Kit Document Scanner
// （Google Play services 提供の撮影 UI）を出し、返った 1 ページ目を長辺 2048 以下の JPEG にして Rust へ返すこと。
// MainActivity から生成・委譲される。Rust の `device/android_document_scan.rs` が
// `MainActivity.requestDocumentScan(id, maxBytes)` を JNI で呼び、結果を `nativeOnDocumentScanResult` で受けて
// `document_scan::complete_from_os` へ渡す。撮影 UI を起動する直前に `nativeTryBeginPresent` で Rust 側の
// 期限切れ・差し替えと排他にする。
//
// 流れ: scan（JNI スレッド）→ start（UI スレッド。前面確認・GMS 判定）→ getStartScanIntent（Task を
// main executor で受ける）→ onScanIntent で IntentSender を起動 → Activity の結果 → worker で読み込み・
// 縮小・JPEG 化・ページファイルの削除 → 結果を Rust へ。Task の完了を worker で待たない。
//
// 結果コードの SSOT は Rust 側 `fandhe_device::document_scan` の契約で、KIND_* は `android_document_scan.rs`
// の定数と同じ値。上限バイト数は Rust の `MAX_PAGE_BYTES` を引数で受ける（ここに複製しない）。
// 長辺 2048・JPEG 品質は trait doc が OS 層に課す値で、iOS 実装（kFandheDocScanMaxLongSide）と揃える。
// 期限（表示開始まで）は Rust の単調時計だけが持つ。GMS 判定と getStartScanIntent（初回のモジュール
// ダウンロード）は期限内に入る既知の天井（trait doc 記載済み）。
//
// ライフサイクル: Activity は WeakReference で持つ。onPause は中断扱いにしない（撮影 UI を出すと自身が
// pause する）。onDestroy で保留中の要求を KIND_INTERRUPTED で確定し、worker を shutdown する（投入済みの
// ファイル削除は走り切る）。再生成後の新インスタンスは NONE から始まり、Rust 側 reset_pending_tracking() と
// 対で旧要求を捨てる。旧インスタンス宛の結果が新ランチャーへ届いても、起動を記録した要求が無いため
// ページファイルを消して捨て、新しい要求の id は消費しない。
// 一時ファイル: ML Kit が返したページの file URI のうち、正規化パスが自アプリの cacheDir 配下のものだけを消す。
// 共有領域の走査はしない。消せずに残った場合は画像を返さず KIND_INTERNAL。
// 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止・結果に入っていたページファイルの削除。
// 守らない範囲: 2 ページ目以降の画像・PDF・撮影 UI の表示中のプロセス死・複数プロセス・Google Play services
// の更新ダイアログ（出さない）。
// ロック順: this のロックを持ったまま nativeTryBeginPresent を呼ぶ。Rust 側は自分のロックの内側から Java を
// 呼ばない（begin() でロックを放してから scan を呼ぶ）ため循環しない。結果の送信は lock の外で行う
// （first-wins の complete だけ lock 内）。画像・URI・例外メッセージはログに出さない。
final class DocumentScanHelper {
    static final int KIND_SCANNED = 0;
    static final int KIND_CANCELLED = 1;
    static final int KIND_UNSUPPORTED = 2;
    static final int KIND_EMPTY = 3;
    static final int KIND_TOO_LARGE = 4;
    static final int KIND_UNDECODABLE = 5;
    static final int KIND_NOT_FOREGROUND = 6;
    static final int KIND_INTERRUPTED = 7;
    static final int KIND_INTERNAL = 8;

    // trait doc（fandhe_device::document_scan）の「OS 層の義務」。iOS の kFandheDocScanMaxLongSide と揃える。
    private static final int MAX_SIDE = 2048;
    private static final int JPEG_QUALITY = 80;

    private static native boolean nativeTryBeginPresent(long requestId);

    private static native void nativeOnDocumentScanResult(long requestId, int kind, int pageCount, byte[] jpeg);

    private final WeakReference<ComponentActivity> activityRef;
    private final RequestTracker request = new RequestTracker();
    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity のフィールド初期化子で生成する。
    private final ActivityResultLauncher<IntentSenderRequest> launcher;
    private final Handler uiHandler = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Executors.newSingleThreadExecutor();

    // 以下は this のロックで守る。
    private long launchedId = RequestTracker.NONE;
    private int maxBytes;
    private boolean destroyed;

    DocumentScanHelper(ComponentActivity activity) {
        this.activityRef = new WeakReference<>(activity);
        this.launcher = activity.registerForActivityResult(
                new ActivityResultContracts.StartIntentSenderForResult(), this::onActivityResult);
    }

    // JNI スレッドから呼ばれる。
    void scan(long id, int maxBytes) {
        synchronized (this) {
            request.track(id);
            launchedId = RequestTracker.NONE;
            this.maxBytes = maxBytes;
        }
        uiHandler.post(() -> start(id));
    }

    private synchronized void start(long id) {
        if (request.current() != id || destroyed) {
            return;
        }
        ComponentActivity activity = activityRef.get();
        if (activity == null || activity.isFinishing() || activity.isDestroyed()) {
            finish(id, KIND_INTERRUPTED, 0, null);
            return;
        }
        // 背景で撮影 UI を出さない。
        if (!activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            finish(id, KIND_NOT_FOREGROUND, 0, null);
            return;
        }
        try {
            // 更新ダイアログは出さない。SUCCESS 以外（無い・古い・無効）は非対応。
            if (GoogleApiAvailability.getInstance().isGooglePlayServicesAvailable(activity)
                    != ConnectionResult.SUCCESS) {
                finish(id, KIND_UNSUPPORTED, 0, null);
                return;
            }
            GmsDocumentScannerOptions options = new GmsDocumentScannerOptions.Builder()
                    .setGalleryImportAllowed(false)
                    .setPageLimit(1)
                    .setResultFormats(GmsDocumentScannerOptions.RESULT_FORMAT_JPEG)
                    .build();
            GmsDocumentScanning.getClient(options).getStartScanIntent(activity)
                    .addOnCompleteListener(ContextCompat.getMainExecutor(activity), task -> {
                        if (task.isSuccessful() && task.getResult() != null) {
                            onScanIntent(id, task.getResult());
                        } else {
                            Exception e = task.getException();
                            boolean unsupported = e instanceof MlKitException
                                    && ((MlKitException) e).getErrorCode() == MlKitException.UNSUPPORTED;
                            onScanIntentFailed(id, unsupported ? KIND_UNSUPPORTED : KIND_INTERNAL);
                        }
                    });
        } catch (RuntimeException e) {
            finish(id, KIND_INTERNAL, 0, null);
        }
    }

    private synchronized void onScanIntentFailed(long id, int kind) {
        if (request.current() == id && !destroyed) {
            finish(id, kind, 0, null);
        }
    }

    private synchronized void onScanIntent(long id, android.content.IntentSender sender) {
        if (request.current() != id || destroyed) {
            return;
        }
        ComponentActivity activity = activityRef.get();
        if (activity == null) {
            finish(id, KIND_INTERRUPTED, 0, null);
            return;
        }
        if (!activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            finish(id, KIND_NOT_FOREGROUND, 0, null);
            return;
        }
        // false は Rust 側が期限切れを確定済み、または古い要求。結果は送らず追跡だけ外す。
        if (!nativeTryBeginPresent(id)) {
            request.complete(id);
            return;
        }
        try {
            launcher.launch(new IntentSenderRequest.Builder(sender).build());
            launchedId = id;
        } catch (RuntimeException e) {
            finish(id, KIND_INTERNAL, 0, null);
        }
    }

    // Activity の結果（UI スレッド）。
    private synchronized void onActivityResult(ActivityResult result) {
        long id = launchedId;
        List<Uri> uris = extractUris(result.getData());
        if (id == RequestTracker.NONE || request.current() != id) {
            // 起動を記録した要求が無い（作り直し後の旧結果など）。ページファイルを消して捨てる。
            deleteAsync(uris);
            return;
        }
        launchedId = RequestTracker.NONE;
        int code = result.getResultCode();
        if (code == Activity.RESULT_CANCELED) {
            finish(id, KIND_CANCELLED, 0, null);
        } else if (code != Activity.RESULT_OK) {
            deleteAsync(uris);
            finish(id, KIND_INTERNAL, 0, null);
        } else if (uris == null || uris.isEmpty()) {
            finish(id, KIND_EMPTY, 0, null);
        } else {
            int limit = maxBytes;
            try {
                worker.execute(() -> convert(id, uris, limit));
            } catch (RuntimeException e) {
                // worker が shutdown 済み（破棄後）。
                finish(id, KIND_INTERRUPTED, 0, null);
            }
        }
    }

    // 結果 Intent から全ページの URI を取る。ページ無し・読めない結果は空リスト、Intent なしは null。
    private static List<Uri> extractUris(Intent data) {
        if (data == null) {
            return null;
        }
        List<Uri> uris = new ArrayList<>();
        try {
            GmsDocumentScanningResult r = GmsDocumentScanningResult.fromActivityResultIntent(data);
            if (r != null && r.getPages() != null) {
                for (GmsDocumentScanningResult.Page p : r.getPages()) {
                    uris.add(p.getImageUri());
                }
            }
        } catch (RuntimeException e) {
            // 読めない結果は 0 ページ扱い。
        }
        return uris;
    }

    // worker スレッド。1 ページ目を JPEG 化し、全ページのファイルを消してから結果を送る。
    private void convert(long id, List<Uri> uris, int limit) {
        int kind;
        byte[] jpeg = null;
        try {
            Object[] r = toJpeg(uris.get(0), limit);
            kind = (Integer) r[0];
            jpeg = (byte[]) r[1];
        } catch (RuntimeException e) {
            kind = KIND_INTERNAL;
        }
        // 取得済みの画像があっても、ファイルを消せなければ返さない。
        if (!deleteAll(uris)) {
            kind = KIND_INTERNAL;
            jpeg = null;
        }
        synchronized (this) {
            if (!request.complete(id)) {
                return;
            }
        }
        nativeOnDocumentScanResult(id, kind, kind == KIND_SCANNED ? uris.size() : 0, jpeg);
    }

    // 長辺 MAX_SIDE 以下の JPEG にする。{kind, bytes} を返す。復号不可は UNDECODABLE、上限超過・OOM は TOO_LARGE。
    private Object[] toJpeg(Uri uri, int limit) {
        Bitmap bmp = null;
        Bitmap scaled = null;
        try {
            BitmapFactory.Options bounds = new BitmapFactory.Options();
            bounds.inJustDecodeBounds = true;
            try (InputStream in = open(uri)) {
                BitmapFactory.decodeStream(in, null, bounds);
            }
            if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
                return new Object[] {KIND_UNDECODABLE, null};
            }
            int longSide = Math.max(bounds.outWidth, bounds.outHeight);
            int sample = 1;
            while (longSide / (sample * 2) >= MAX_SIDE) {
                sample *= 2;
            }
            BitmapFactory.Options opts = new BitmapFactory.Options();
            opts.inSampleSize = sample;
            try (InputStream in = open(uri)) {
                bmp = BitmapFactory.decodeStream(in, null, opts);
            }
            if (bmp == null) {
                return new Object[] {KIND_UNDECODABLE, null};
            }
            scaled = bmp;
            int side = Math.max(bmp.getWidth(), bmp.getHeight());
            if (side > MAX_SIDE) {
                float k = (float) MAX_SIDE / side;
                scaled = Bitmap.createScaledBitmap(bmp,
                        Math.max(1, Math.round(bmp.getWidth() * k)),
                        Math.max(1, Math.round(bmp.getHeight() * k)), true);
            }
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            if (!scaled.compress(Bitmap.CompressFormat.JPEG, JPEG_QUALITY, out)) {
                return new Object[] {KIND_INTERNAL, null};
            }
            if (out.size() > limit) {
                return new Object[] {KIND_TOO_LARGE, null};
            }
            return new Object[] {KIND_SCANNED, out.toByteArray()};
        } catch (IOException e) {
            return new Object[] {KIND_UNDECODABLE, null};
        } catch (OutOfMemoryError e) {
            return new Object[] {KIND_TOO_LARGE, null};
        } finally {
            if (scaled != null && scaled != bmp) {
                scaled.recycle();
            }
            if (bmp != null) {
                bmp.recycle();
            }
        }
    }

    private InputStream open(Uri uri) throws IOException {
        ComponentActivity activity = activityRef.get();
        InputStream in = activity == null ? null : activity.getContentResolver().openInputStream(uri);
        if (in == null) {
            throw new IOException();
        }
        return in;
    }

    // 自アプリの cacheDir 配下の file URI だけを消す。他所のファイルは消さない（成功扱い）。
    // 消そうとして残ったら false。
    private boolean deleteAll(List<Uri> uris) {
        ComponentActivity activity = activityRef.get();
        boolean ok = true;
        for (Uri u : uris) {
            try {
                if (!"file".equals(u.getScheme()) || u.getPath() == null || activity == null) {
                    continue;
                }
                File f = new File(u.getPath()).getCanonicalFile();
                String root = activity.getCacheDir().getCanonicalPath() + File.separator;
                if (!f.getPath().startsWith(root)) {
                    continue;
                }
                if (f.exists() && !f.delete() && f.exists()) {
                    ok = false;
                }
            } catch (IOException | RuntimeException e) {
                ok = false;
            }
        }
        return ok;
    }

    private void deleteAsync(List<Uri> uris) {
        if (uris == null || uris.isEmpty()) {
            return;
        }
        try {
            worker.execute(() -> deleteAll(uris));
        } catch (RuntimeException e) {
            // shutdown 済み。onDestroy 後の旧結果は取り残しうる（守らない範囲）。
        }
    }

    // first-wins: 現在の要求だけ確定して Rust へ返す。
    private void finish(long id, int kind, int pageCount, byte[] jpeg) {
        if (request.complete(id)) {
            launchedId = RequestTracker.NONE;
            nativeOnDocumentScanResult(id, kind, pageCount, jpeg);
        }
    }

    synchronized void onDestroy() {
        destroyed = true;
        long id = request.current();
        if (id != RequestTracker.NONE) {
            finish(id, KIND_INTERRUPTED, 0, null);
        }
        worker.shutdown();
    }
}
