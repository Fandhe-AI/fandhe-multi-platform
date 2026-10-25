package com.fandhe.androidapp;

import android.Manifest;
import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.media.MediaMetadataRetriever;
import android.net.Uri;
import android.provider.MediaStore;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResult;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;
import androidx.core.content.FileProvider;
import androidx.lifecycle.Lifecycle;

import java.io.File;
import java.security.SecureRandom;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

// 動画の撮影（Issue #823、親 #169）の Android 実装ヘルパー。MainActivity から生成・委譲される。
// Rust の `device/android_video_capture.rs` が `MainActivity.requestVideoCapture` を JNI で呼び、
// 表示の直前に `nativeTryBeginPresent`、結果を `nativeOnVideoCaptureResult` で返す。
// 契約の SSOT は `fandhe_device::video_capture`（結果コード表 `result_from_code`）。
//
// 流れ: CAMERA の実行時許可 → `MediaStore.ACTION_VIDEO_CAPTURE`（EXTRA_OUTPUT は FileProvider の
// `cache/video_capture/`、EXTRA_DURATION_LIMIT は上限秒）→ Activity の結果で完了。
// 撮れたかは OS の完了通知だけで決め、期限・タイマーは持たない。取り消しは失敗にしない。
// 長さ・寸法・回転は MediaMetadataRetriever、サイズはファイルの大きさから読み、読めなければ
// 「読めない」として返す（推測しない）。`resolveActivity` で事前判定せず、
// ActivityNotFoundException を NO_CAMERA_APP へ写す（`<queries>` は足さない）。
// `RECORD_AUDIO` も足さない。
//
// 結果コード（Rust の `result_from_code` と同じ値）:
// 0=撮れた／1=取り消し／3=権限拒否／4=カメラアプリなし／5=前面でない／6=中断／
// 7=空のファイル／8=読めない／9=その他。2（非対応）は Rust 側の値で、ここでは返さない。
//
// スレッド: 状態遷移は UI スレッドで直列化する。メタデータの読み取りだけ単一スレッドの
// executor で行い、そこへ移した要求の報告は作業スレッドの 1 回だけ。
// 一時ファイル: 成功・取り消し・失敗・破棄の全経路で消す。撮影の前に置き場を空にして、
// プロセス死などで残ったものも回収する。Rust へパスは渡さない。
// ライフサイクル: `request` と `permissionId` はインスタンスフィールド。`onPause` では何もしない
// （許可ダイアログとカメラアプリの起動自体が onPause を起こす）。`onDestroy` で保留中の要求を
// 中断で確定する。再生成後の新インスタンスは NONE から始まり、旧インスタンス宛ての結果を捨てる。
// 旧要求の結果は Rust 側 `reset_pending_tracking()` と id 照合でも捨てられる。
// 守る範囲: 1 プロセス内の順序・嘘の成功の禁止・一時ファイルの解放。守らない範囲:
// EXTRA_OUTPUT を無視して別の場所へ保存するカメラアプリ（空のファイルとして扱う）、
// 撮影中の背景遷移の扱い（OS 任せ）、撮影中のプロセス死。
// パス・動画の中身・例外メッセージはログに出さない。
final class VideoCaptureHelper {
    static final int RESULT_CAPTURED = 0;
    static final int RESULT_CANCELLED = 1;
    static final int RESULT_PERMISSION_DENIED = 3;
    static final int RESULT_NO_CAMERA_APP = 4;
    static final int RESULT_NOT_FOREGROUND = 5;
    static final int RESULT_INTERRUPTED = 6;
    static final int RESULT_EMPTY = 7;
    static final int RESULT_UNREADABLE = 8;
    static final int RESULT_INTERNAL = 9;

    private static final String DIR_NAME = "video_capture";

    private static native boolean nativeTryBeginPresent(long requestId);

    private static native void nativeOnVideoCaptureResult(long requestId, int kind, long sizeBytes,
            long durationMs, int width, int height, int rotation);

    private static final SecureRandom RANDOM = new SecureRandom();

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    private final ExecutorService reader = Executors.newSingleThreadExecutor();
    // 以下は UI スレッド専用。
    private long permissionId = RequestTracker.NONE; // 許可ダイアログ待ちの要求 id。
    private int permissionMaxSecs;
    private File launchedFile; // 撮影中の出力先。結果で必ず消す。
    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity の
    // フィールド初期化子から本クラスを生成してここで登録する。
    private final ActivityResultLauncher<String> permissionLauncher;
    private final ActivityResultLauncher<Intent> videoLauncher;

    VideoCaptureHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.RequestPermission(), this::onPermissionResult);
        this.videoLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.StartActivityForResult(), this::onVideoResult);
    }

    // JNI スレッドから呼ばれる。
    void capture(long id, int maxSecs) {
        activity.runOnUiThread(() -> start(id, maxSecs));
    }

    // MainActivity.onDestroy から UI スレッドで呼ぶ。作業スレッドへ移った要求は、そちらが 1 回だけ報告する。
    void onDestroy() {
        long pending = request.take();
        permissionId = RequestTracker.NONE;
        File f = launchedFile;
        launchedFile = null;
        deleteQuietly(f);
        reader.shutdown();
        if (pending != RequestTracker.NONE) {
            report(pending, RESULT_INTERRUPTED);
        }
    }

    private boolean destroyed() {
        return activity.isFinishing() || activity.isDestroyed();
    }

    private void report(long id, int kind) {
        nativeOnVideoCaptureResult(id, kind, 0L, -1L, -1, -1, 0);
    }

    // 追跡を外せた（所有権を取れた）ときだけ報告する。取れなければ誰かが確定済みで何もしない。
    private void finishWith(long id, int kind) {
        if (request.complete(id)) {
            report(id, kind);
        }
    }

    private static void deleteQuietly(File f) {
        if (f != null) {
            //noinspection ResultOfMethodCallIgnored
            f.delete();
        }
    }

    // 置き場を空にして作り直す。前回のプロセス死・再生成で残ったファイルを回収する。
    private File prepareDir() {
        File dir = new File(activity.getCacheDir(), DIR_NAME);
        File[] old = dir.listFiles();
        if (old != null) {
            for (File o : old) {
                deleteQuietly(o);
            }
        }
        if (!dir.isDirectory() && !dir.mkdirs()) {
            return null;
        }
        return dir;
    }

    private static String randomName() {
        byte[] b = new byte[16];
        RANDOM.nextBytes(b);
        StringBuilder sb = new StringBuilder(36);
        for (byte x : b) {
            sb.append(String.format("%02x", x));
        }
        return sb.append(".mp4").toString();
    }

    private void start(long id, int maxSecs) {
        if (destroyed()) {
            report(id, RESULT_INTERRUPTED);
            return;
        }
        if (request.current() != RequestTracker.NONE) {
            // Rust 側の保留判定が二重発送を防ぐため通常は来ない。来ても既存の要求は壊さない。
            report(id, RESULT_INTERNAL);
            return;
        }
        if (!activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            report(id, RESULT_NOT_FOREGROUND);
            return;
        }
        request.track(id);
        if (ContextCompat.checkSelfPermission(activity, Manifest.permission.CAMERA)
                == PackageManager.PERMISSION_GRANTED) {
            launch(id, maxSecs);
            return;
        }
        permissionId = id;
        permissionMaxSecs = maxSecs;
        try {
            permissionLauncher.launch(Manifest.permission.CAMERA);
        } catch (RuntimeException e) {
            permissionId = RequestTracker.NONE;
            finishWith(id, RESULT_INTERNAL);
        }
    }

    private void onPermissionResult(Boolean granted) {
        long id = permissionId;
        int maxSecs = permissionMaxSecs;
        permissionId = RequestTracker.NONE;
        // 現在の要求の許可でなければ（再生成後の新インスタンス・破棄済み）捨てる。
        if (id == RequestTracker.NONE || id != request.current()) {
            return;
        }
        if (!Boolean.TRUE.equals(granted)) {
            finishWith(id, RESULT_PERMISSION_DENIED);
            return;
        }
        // 許可ダイアログが閉じた直後の結果は STARTED に戻った後、RESUMED より前に届く。
        // RESUMED で見ると正常な許可を中断扱いにするため STARTED で判定する。STARTED 未満の間は
        // ActivityResultRegistry が配送を保留するので、背景でカメラアプリを起動することもない。
        if (destroyed() || !activity.getLifecycle().getCurrentState()
                .isAtLeast(Lifecycle.State.STARTED)) {
            finishWith(id, RESULT_INTERRUPTED);
            return;
        }
        launch(id, maxSecs);
    }

    private void launch(long id, int maxSecs) {
        File dir = prepareDir();
        if (dir == null) {
            finishWith(id, RESULT_INTERNAL);
            return;
        }
        File file = new File(dir, randomName());
        Uri uri;
        try {
            uri = FileProvider.getUriForFile(activity,
                    activity.getPackageName() + ".compose", file);
        } catch (IllegalArgumentException e) {
            finishWith(id, RESULT_INTERNAL);
            return;
        }
        // 表示の直前に Rust へ確かめる。false は古い要求などで、結果は Rust が捨てるため報告しない。
        if (!nativeTryBeginPresent(id)) {
            request.complete(id);
            return;
        }
        Intent intent = new Intent(MediaStore.ACTION_VIDEO_CAPTURE)
                .putExtra(MediaStore.EXTRA_OUTPUT, uri)
                .putExtra(MediaStore.EXTRA_DURATION_LIMIT, maxSecs)
                .addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                        | Intent.FLAG_GRANT_READ_URI_PERMISSION);
        intent.setClipData(ClipData.newRawUri("", uri));
        launchedFile = file;
        int kind;
        try {
            videoLauncher.launch(intent);
            return;
        } catch (ActivityNotFoundException e) {
            kind = RESULT_NO_CAMERA_APP;
        } catch (SecurityException e) {
            kind = RESULT_PERMISSION_DENIED;
        } catch (RuntimeException e) {
            kind = RESULT_INTERNAL;
        }
        launchedFile = null;
        deleteQuietly(file);
        finishWith(id, kind);
    }

    private void onVideoResult(ActivityResult result) {
        File file = launchedFile;
        launchedFile = null;
        long id = request.current();
        // 所有権を取れなければ捨てる（再生成後の新インスタンス・onDestroy で確定済み）。
        if (id == RequestTracker.NONE || !request.complete(id)) {
            if (file == null) {
                clearStale();
            } else {
                deleteQuietly(file);
            }
            return;
        }
        if (result.getResultCode() != Activity.RESULT_OK) {
            deleteQuietly(file);
            report(id, RESULT_CANCELLED);
            return;
        }
        if (file == null) {
            report(id, RESULT_INTERNAL);
            return;
        }
        try {
            reader.execute(() -> readAndReport(id, file));
        } catch (RuntimeException e) {
            deleteQuietly(file);
            report(id, RESULT_INTERNAL);
        }
    }

    // 出力先が分からない旧結果のとき、置き場に残ったファイルを回収する。
    private void clearStale() {
        File[] old = new File(activity.getCacheDir(), DIR_NAME).listFiles();
        if (old != null) {
            for (File o : old) {
                deleteQuietly(o);
            }
        }
    }

    // 数値にできなければ -1。
    private static long parseLong(String s) {
        if (s == null) {
            return -1L;
        }
        try {
            return Long.parseLong(s.trim());
        } catch (NumberFormatException e) {
            return -1L;
        }
    }

    // 作業スレッド。成功・失敗とも finally で一時ファイルを消し、報告は 1 回だけ。
    private void readAndReport(long id, File file) {
        MediaMetadataRetriever mmr = null;
        try {
            long size = file.length();
            if (size <= 0L) {
                report(id, RESULT_EMPTY);
                return;
            }
            mmr = new MediaMetadataRetriever();
            try {
                mmr.setDataSource(file.getAbsolutePath());
            } catch (RuntimeException e) {
                report(id, RESULT_UNREADABLE);
                return;
            }
            long durationMs = parseLong(
                    mmr.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION));
            long w = parseLong(
                    mmr.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH));
            long h = parseLong(
                    mmr.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT));
            // 回転情報が無ければ 0。あるのに数値にできなければ読めない。
            String rawRotation = mmr.extractMetadata(
                    MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION);
            long rotation = rawRotation == null ? 0L : parseLong(rawRotation);
            if (w < 0 || h < 0 || w > Integer.MAX_VALUE || h > Integer.MAX_VALUE
                    || (rawRotation != null && rotation < 0)) {
                report(id, RESULT_UNREADABLE);
                return;
            }
            int rot = (int) (((rotation % 360) + 360) % 360);
            nativeOnVideoCaptureResult(id, RESULT_CAPTURED, size, durationMs, (int) w, (int) h,
                    rot);
        } catch (RuntimeException e) {
            report(id, RESULT_UNREADABLE);
        } finally {
            if (mmr != null) {
                try {
                    // minSdk 24 のため close()（API 29 以上）は使わない。
                    mmr.release();
                } catch (Exception e) {
                    // 解放の失敗は結果に影響しない。
                }
            }
            deleteQuietly(file);
        }
    }
}
