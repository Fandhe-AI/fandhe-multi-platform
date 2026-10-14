package com.fandhe.androidapp;

import android.app.Notification;
import android.app.Service;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.hardware.display.DisplayManager;
import android.hardware.display.VirtualDisplay;
import android.media.MediaRecorder;
import android.media.projection.MediaProjection;
import android.media.projection.MediaProjectionManager;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.SystemClock;
import android.util.DisplayMetrics;

import androidx.core.app.NotificationChannelCompat;
import androidx.core.app.NotificationCompat;
import androidx.core.app.NotificationManagerCompat;
import androidx.core.app.ServiceCompat;
import androidx.core.content.IntentCompat;

import java.io.File;

// 画面録画（Issue #1337）の実体。`MediaProjection` は Android 10 以降
// 「型 mediaProjection のフォアグラウンドサービス」からしか取得できないため、
// `ScreenCaptureHelper`（同意ダイアログ・ライフサイクル）から起動される。
// Rust 側 `device/android_screen_capture.rs` へは `ScreenCaptureHelper.report` 経由で結果を返す。
//
// 状態はすべて main looper 上で扱う（`active` も同様）。確定点は `finish` の 1 か所で、
// first-wins（`done`）。`projection.stop()` が `onStop` を再入させても二重確定しない。
// 切り離し時は本クラスと AndroidManifest の service / 権限を削除する。
public final class ScreenCaptureService extends Service {
    static final String EXTRA_ID = "id";
    static final String EXTRA_MAX_MS = "maxMs";
    static final String EXTRA_RESULT_CODE = "resultCode";
    static final String EXTRA_DATA = "data";

    private static final String CHANNEL_ID = "fandhe_screen_capture";
    private static final int NOTIFICATION_ID = 0x5C47;
    // 録画は同時に 1 件だけ。プロセス死で残っても次回に上書きされ、孤児が増えない。
    private static final String FILE_NAME = "fandhe_screen_capture.mp4";
    private static final int MAX_LONG_SIDE = 1280;

    // 録画中のみ非 null（main looper 専用）。停止を Intent で送ると停止済みの Service を
    // 起こしてしまうため、static 参照を直接呼ぶ。
    private static ScreenCaptureService active;

    private final Handler handler = new Handler(Looper.getMainLooper());
    private long id = RequestTracker.NONE;
    private boolean done;
    private MediaProjection projection;
    private MediaProjection.Callback projectionCallback;
    private MediaRecorder recorder;
    private VirtualDisplay display;
    private File file;
    private long startedAt;

    // 明示停止（Rust の stop）。該当 id が録画中でなければ何もしない。
    static void stopActive(long id) {
        if (active != null && active.id == id) {
            active.finish(ScreenCaptureHelper.RESULT_OK);
        }
    }

    // 背景遷移・Activity 破棄: 書き出し済みでも成功にせず中断として確定する。
    static void abortActive() {
        if (active != null) {
            active.finish(ScreenCaptureHelper.RESULT_INTERRUPTED);
        }
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent == null) {
            stopSelf();
            return START_NOT_STICKY;
        }
        long reqId = intent.getLongExtra(EXTRA_ID, RequestTracker.NONE);
        try {
            promoteToForeground();
        } catch (RuntimeException e) {
            reject(reqId, ScreenCaptureHelper.RESULT_FAILED);
            return START_NOT_STICKY;
        }
        // 開始前に停止・破棄されていれば helper 側で確定済み。
        if (!ScreenCaptureHelper.STARTING.complete(reqId)) {
            leave();
            return START_NOT_STICKY;
        }
        if (active != null) {
            ScreenCaptureHelper.report(reqId, ScreenCaptureHelper.RESULT_BUSY, 0, 0);
            return START_NOT_STICKY;
        }
        // finish 済みインスタンスが破棄前に再利用されても、確定点が働くよう状態を初期化する。
        done = false;
        startedAt = 0;
        id = reqId;
        active = this;
        try {
            begin(intent);
        } catch (java.io.IOException | RuntimeException e) {
            finish(ScreenCaptureHelper.RESULT_FAILED);
        }
        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        if (active == this) {
            finish(ScreenCaptureHelper.RESULT_INTERRUPTED);
        }
        super.onDestroy();
    }

    private void promoteToForeground() {
        NotificationManagerCompat.from(this).createNotificationChannel(
                new NotificationChannelCompat.Builder(
                        CHANNEL_ID, NotificationManagerCompat.IMPORTANCE_LOW)
                        .setName("Screen capture")
                        .build());
        Notification notification = new NotificationCompat.Builder(this, CHANNEL_ID)
                .setSmallIcon(android.R.drawable.ic_menu_camera)
                .setContentTitle("Recording screen")
                .setOngoing(true)
                .build();
        // API 34 は getMediaProjection より前にこの呼び出しが必須。
        ServiceCompat.startForeground(this, NOTIFICATION_ID, notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION);
    }

    // 録画前に失敗した要求を確定して自身を畳む（helper 側で未確定のときだけ報告する）。
    private void reject(long reqId, int code) {
        if (ScreenCaptureHelper.STARTING.complete(reqId)) {
            ScreenCaptureHelper.report(reqId, code, 0, 0);
        }
        leave();
    }

    private void leave() {
        if (active == null) {
            ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE);
            stopSelf();
        }
    }

    private void begin(Intent intent) throws java.io.IOException {
        int maxMs = intent.getIntExtra(EXTRA_MAX_MS, 0);
        Intent data = IntentCompat.getParcelableExtra(intent, EXTRA_DATA, Intent.class);
        MediaProjectionManager mpm = (MediaProjectionManager) getSystemService(MEDIA_PROJECTION_SERVICE);
        if (mpm == null || data == null) {
            throw new IllegalStateException("projection unavailable");
        }
        projection = mpm.getMediaProjection(
                intent.getIntExtra(EXTRA_RESULT_CODE, 0), data);
        if (projection == null) {
            throw new IllegalStateException("projection null");
        }
        // API 34 は createVirtualDisplay より前の登録が必須。OS・利用者による停止もここへ来る。
        // 登録時の projection を握り、現在の録画のものと一致するときだけ確定する
        // （録画 A の遅延した onStop が同一インスタンスで始まった録画 B を終えない）。
        final MediaProjection mine = projection;
        projectionCallback = new MediaProjection.Callback() {
            @Override
            public void onStop() {
                if (projection == mine) {
                    finish(ScreenCaptureHelper.RESULT_OK);
                }
            }
        };
        projection.registerCallback(projectionCallback, handler);

        DisplayMetrics m = getResources().getDisplayMetrics();
        float scale = Math.min(1f, (float) MAX_LONG_SIDE / Math.max(m.widthPixels, m.heightPixels));
        int w = Math.max(16, (int) (m.widthPixels * scale) / 16 * 16);
        int h = Math.max(16, (int) (m.heightPixels * scale) / 16 * 16);

        file = new File(getCacheDir(), FILE_NAME);
        // ponytail: 失敗しても次の setOutputFile が上書きする。
        file.delete();
        recorder = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
                ? new MediaRecorder(this) : new MediaRecorder();
        recorder.setVideoSource(MediaRecorder.VideoSource.SURFACE);
        recorder.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4);
        recorder.setVideoEncoder(MediaRecorder.VideoEncoder.H264);
        recorder.setVideoSize(w, h);
        recorder.setVideoFrameRate(30);
        recorder.setVideoEncodingBitRate(4_000_000);
        recorder.setOutputFile(file.getAbsolutePath());
        recorder.prepare();
        display = projection.createVirtualDisplay("fandhe-capture", w, h, m.densityDpi,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, recorder.getSurface(), null, null);
        recorder.start();
        startedAt = SystemClock.elapsedRealtime();
        ScreenCaptureHelper.nativeOnScreenCaptureStarted(id);
        // Rust 側の MAX_CAPTURE_MS と同じ値が maxMs で渡る（独自の期限は持たない）。
        handler.postDelayed(() -> finish(ScreenCaptureHelper.RESULT_OK), Math.max(1, maxMs));
    }

    // 唯一の確定点。code は希望する結果（RESULT_OK なら書き出しを試み、得られなければ失敗）。
    private void finish(int code) {
        if (done) {
            return;
        }
        done = true;
        if (active == this) {
            active = null;
        }
        handler.removeCallbacksAndMessages(null);
        long duration = startedAt == 0 ? 0 : SystemClock.elapsedRealtime() - startedAt;
        if (recorder != null) {
            try {
                recorder.stop();
            } catch (RuntimeException e) {
                // データが 1 つも無いままの停止など。成功には畳まない。
                if (code == ScreenCaptureHelper.RESULT_OK) {
                    code = ScreenCaptureHelper.RESULT_FAILED;
                }
            }
            recorder.release();
            recorder = null;
        }
        if (display != null) {
            display.release();
            display = null;
        }
        if (projection != null) {
            try {
                if (projectionCallback != null) {
                    projection.unregisterCallback(projectionCallback);
                }
                projection.stop();
            } catch (RuntimeException ignored) {
                // 既に OS 側で停止済み。
            }
            projection = null;
            projectionCallback = null;
        }
        long size = 0;
        if (file != null) {
            size = file.length();
            file.delete();
            file = null;
        }
        long reqId = id;
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE);
        stopSelf();
        ScreenCaptureHelper.report(reqId, code, size, duration);
    }
}
