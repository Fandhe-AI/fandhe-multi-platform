package com.fandhe.androidapp;

import android.Manifest;
import android.content.Context;
import android.content.pm.PackageManager;
import android.graphics.ImageFormat;
import android.hardware.camera2.CameraAccessException;
import android.hardware.camera2.CameraCaptureSession;
import android.hardware.camera2.CameraCharacteristics;
import android.hardware.camera2.CameraDevice;
import android.hardware.camera2.CameraManager;
import android.hardware.camera2.CaptureRequest;
import android.hardware.camera2.CaptureResult;
import android.hardware.camera2.TotalCaptureResult;
import android.hardware.camera2.params.OutputConfiguration;
import android.hardware.camera2.params.SessionConfiguration;
import android.hardware.camera2.params.StreamConfigurationMap;
import android.media.Image;
import android.media.ImageReader;
import android.os.Build;
import android.os.Handler;
import android.os.HandlerThread;
import android.util.Range;
import android.util.Size;
import android.view.Surface;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;

import java.nio.ByteBuffer;
import java.util.Arrays;
import java.util.Collections;

// カメラのライブ映像画面（Issue #814、親 #168）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_camera_preview_stream.rs` が `MainActivity.cameraPreviewStart` /
// `cameraPreviewStop` / `cameraPreviewZoomRange` / `cameraPreviewSetZoom` を JNI で呼び、結果を
// `nativeOnCameraPreviewEvent` / `nativeOnCameraPreviewFrame` で受ける）。
//
// 方式: Camera2（`openCamera` → `createCaptureSession` → `setRepeatingRequest(TEMPLATE_PREVIEW)`）+
// `ImageReader`（`YUV_420_888`、`acquireLatestImage`）。縮小は Rust の `downscale_yuv420` が行う。
// CameraX は Gradle 依存の追加が要るため使わない。
//
// スレッド: 許可・開始・ライフサイクルは UI スレッド、open・フレーム受信・ズーム適用・解放は
// ヘルパー専用の 1 本のカメラスレッドに直列化する（前の解放が次の open より先に終わる）。
// 状態は `lock` で守り、native 通知は必ず `lock` の外で行う（Rust 側のロックとの取り合いを避ける）。
//
// 数値コード: `nativeOnCameraPreviewEvent` の code / detail の SSOT は Rust の
// `fandhe_device::camera_preview_stream::event_from_code`。下の `EV_*` はその表と同じ値で並べる。
//
// ライフサイクル: `onPause` で映像を止める（EV_STOPPED_BACKGROUND）。許可ダイアログ待ちと、許可が
// 非前景中に届いた保留（`startPendingResume`）は止めない（ダイアログ自体が `onPause` を起こすため。
// 正常な許可を中断扱いにせず、背景での開始も防ぐ）。`onDestroy`（Activity 再生成を含む）では
// 保留中・実行中の要求を EV_STOPPED_LEFT で確定して解放する。`request` はインスタンスフィールドで、
// 再生成後の新インスタンスは NONE から始まるため旧結果が新しい要求の id を消費しない（id は Rust 側で
// プロセス内単調増加。旧インスタンスから遅れて届く通知は Rust の登録表が捨てる）。
// 他 OS と違い「中断」は送らない。OS が止めた（他アプリに取られた等）ら終端にし、自動では再開しない。
//
// 画素・例外メッセージ・id はログに出さない。
final class CameraPreviewHelper {
    // `cameraPreviewStart` の戻り値（SSOT は Rust 側 `START_OK`）。
    static final int START_OK = 0;
    static final int START_FAILED = 1;
    // `cameraPreviewSetZoom` の戻り値（Rust 側 `decode_set_zoom`）。
    static final int ZOOM_OK = 0;
    static final int ZOOM_UNSUPPORTED = 1;
    static final int ZOOM_FAILED = 2;

    // イベントコード。値の表は Rust の `event_from_code`（上記コメント参照）。
    static final int EV_RUNNING = 0;
    static final int EV_STOPPED_BACKGROUND = 2;
    static final int EV_STOPPED_LEFT = 3;
    static final int EV_START_FAILED = 4;
    static final int EV_RUNTIME_FAILED = 5;
    static final int EV_ZOOM_FAILED = 6;
    // EV_START_FAILED の detail。
    static final int DETAIL_PERMISSION = 0;
    static final int DETAIL_NO_CAMERA = 1;
    static final int DETAIL_IN_USE = 2;
    static final int DETAIL_OTHER = 3;
    // EV_ZOOM_FAILED の detail（0 は非対応、他は失敗）。
    static final int DETAIL_ZOOM_FAILED = 1;

    // 選ぶ映像寸法の目標と長辺の上限（Rust 側 `MAX_SOURCE_SIDE` = 8192 を超えない）。
    private static final int TARGET_W = 640;
    private static final int TARGET_H = 480;
    private static final int PREFERRED_MAX_SIDE = 1280;

    private static native void nativeOnCameraPreviewEvent(long id, int code, int detail);

    private static native void nativeOnCameraPreviewFrame(
            long id, int width, int height,
            ByteBuffer y, int yRowStride,
            ByteBuffer u, ByteBuffer v, int uvRowStride, int uvPixelStride,
            float zoom);

    /** 1 回の映像。`lock` の内側で読み書きする（`lastZoom` は volatile）。 */
    private static final class Session {
        final long id;
        final Handler handler;
        CameraDevice device;
        CameraCaptureSession captureSession;
        ImageReader reader;
        CaptureRequest.Builder builder;
        CameraCaptureSession.CaptureCallback captureCallback;
        float zoomMin = Float.NaN; // NaN = ズーム非対応。
        float zoomMax = Float.NaN;
        boolean configured;
        boolean running;
        boolean closed;
        volatile float lastZoom = Float.NaN;

        Session(long id, Handler handler) {
            this.id = id;
            this.handler = handler;
        }
    }

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    private final Object lock = new Object();
    // 以下 3 つは lock の内側。
    private Session session;
    private HandlerThread thread;
    private Handler handler;
    private boolean destroyedHelper;
    // 取り消し済みの最大 id（lock の内側）。stop が start より先に走っても古い要求を開始しない。
    private long cancelledId = RequestTracker.NONE;
    // 以下は UI スレッド専用。
    private long permissionId = RequestTracker.NONE; // 許可ダイアログを起動した要求 id。
    private long latestId = RequestTracker.NONE; // UI で最後に受理した id（追い越された古い要求を捨てる）。
    private boolean startPendingResume; // 許可が非前景中に届いたため次の onResume まで保留。
    private boolean resumed;

    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity の
    // フィールド初期化子から本クラスを生成してここで登録する。
    private final ActivityResultLauncher<String> permissionLauncher;

    CameraPreviewHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.RequestPermission(), this::onPermissionResult);
    }

    // ---- JNI スレッドから呼ばれる入口 ----

    // 開くのは非同期。UI スレッドへ渡して受け付けたことだけ返す。
    int start(long id) {
        activity.runOnUiThread(() -> startOnUi(id));
        return START_OK;
    }

    // Rust 側の session drop から任意のスレッドで呼ばれる（冪等。利用者の停止なのでイベントは送らない）。
    void stop(long id) {
        Session s = null;
        synchronized (lock) {
            cancelledId = Math.max(cancelledId, id);
            if (request.complete(id)) {
                s = session;
                session = null;
                if (s != null) {
                    s.closed = true;
                }
            }
        }
        if (s != null) {
            release(s);
        }
    }

    // 現在の映像のズーム範囲。null = 失敗 / 長さ 0 = 非対応 / 長さ 2 = {min, max}。
    float[] zoomRange(long id) {
        synchronized (lock) {
            Session s = session;
            if (s == null || s.id != id || s.closed || !s.configured) {
                return null;
            }
            if (Float.isNaN(s.zoomMin) || Float.isNaN(s.zoomMax)) {
                return new float[0];
            }
            return new float[] {s.zoomMin, s.zoomMax};
        }
    }

    // ズームを変える。受け付けたら 0。適用はカメラスレッドで非同期に行い、失敗は EV_ZOOM_FAILED で返す。
    int setZoom(long id, float ratio) {
        final Session s;
        synchronized (lock) {
            s = session;
            if (s == null || s.id != id || s.closed || !s.configured) {
                return ZOOM_FAILED;
            }
            if (Float.isNaN(s.zoomMin) || Float.isNaN(s.zoomMax)) {
                return ZOOM_UNSUPPORTED;
            }
            if (Float.isNaN(ratio) || Float.isInfinite(ratio) || ratio < s.zoomMin || ratio > s.zoomMax) {
                return ZOOM_FAILED;
            }
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            return ZOOM_UNSUPPORTED;
        }
        s.handler.post(() -> applyZoom(s, ratio));
        return ZOOM_OK;
    }

    // ---- MainActivity のライフサイクルから UI スレッドで呼ばれる ----

    void onResume() {
        resumed = true;
        if (startPendingResume) {
            startPendingResume = false;
            long id = request.current();
            if (id != RequestTracker.NONE) {
                openAsync(id);
            }
        }
    }

    void onPause() {
        resumed = false;
        // 許可ダイアログ待ちと許可後の保留中は止めない（ダイアログ自体が onPause を起こす）。
        if (permissionId != RequestTracker.NONE || startPendingResume) {
            return;
        }
        finishRequest(EV_STOPPED_BACKGROUND, 0);
    }

    void onDestroy() {
        finishRequest(EV_STOPPED_LEFT, 0);
        final HandlerThread t;
        synchronized (lock) {
            destroyedHelper = true;
            t = thread;
            thread = null;
            handler = null;
        }
        if (t != null) {
            // 先に post した解放は quitSafely が処理してから終わる。
            t.quitSafely();
        }
    }

    // ---- 開始・許可（UI スレッド） ----

    private boolean destroyed() {
        return activity.isFinishing() || activity.isDestroyed();
    }

    private void startOnUi(long id) {
        if (id <= latestId) {
            return;
        }
        latestId = id;
        if (destroyed()) {
            nativeOnCameraPreviewEvent(id, EV_STOPPED_LEFT, 0);
            return;
        }
        final Session old;
        synchronized (lock) {
            if (id <= cancelledId) {
                return;
            }
            old = session;
            session = null;
            if (old != null) {
                old.closed = true;
            }
            request.track(id);
        }
        if (old != null) {
            release(old);
        }
        startPendingResume = false;
        checkPermissionAndOpen(id);
    }

    private void checkPermissionAndOpen(long id) {
        if (activity.checkSelfPermission(Manifest.permission.CAMERA)
                != PackageManager.PERMISSION_GRANTED) {
            // ダイアログは同時に 1 つだけ。出ている間に差し替わった要求は結果到着時に再判定する。
            if (permissionId == RequestTracker.NONE) {
                permissionId = id;
                permissionLauncher.launch(Manifest.permission.CAMERA);
            }
            return;
        }
        openWhenResumed(id);
    }

    private void openWhenResumed(long id) {
        if (resumed) {
            openAsync(id);
        } else {
            startPendingResume = true; // 非前景では開かない。次の onResume で開く。
        }
    }

    // 許可ダイアログ自体が Activity を pause させるため、ここで onPause 扱いにはしない。
    private void onPermissionResult(boolean granted) {
        long launched = permissionId;
        permissionId = RequestTracker.NONE;
        long current = request.current();
        if (current == RequestTracker.NONE) {
            return; // 取り消し・破棄済み・再生成後のインスタンス。
        }
        if (destroyed()) {
            finishRequest(EV_STOPPED_LEFT, 0);
        } else if (launched != current) {
            // 起動時の要求は差し替え済み。古い結果は捨て、現在の要求で許可を再判定する。
            checkPermissionAndOpen(current);
        } else if (!granted) {
            finishRequest(EV_START_FAILED, DETAIL_PERMISSION);
        } else {
            openWhenResumed(current);
        }
    }

    // ---- カメラスレッド ----

    private Handler ensureHandler() {
        synchronized (lock) {
            if (destroyedHelper) {
                return null;
            }
            if (handler == null) {
                thread = new HandlerThread("fandhe-camera-preview");
                thread.start();
                handler = new Handler(thread.getLooper());
            }
            return handler;
        }
    }

    private void openAsync(long id) {
        Handler h = ensureHandler();
        if (h != null) {
            h.post(() -> openOnCameraThread(id, h));
        }
    }

    private void openOnCameraThread(long id, Handler h) {
        final Session s;
        synchronized (lock) {
            if (request.current() != id || destroyedHelper) {
                return; // 取り消し・差し替え・破棄済み。
            }
            s = new Session(id, h);
            session = s;
        }
        try {
            CameraManager manager = (CameraManager) activity.getSystemService(Context.CAMERA_SERVICE);
            if (manager == null) {
                finishSession(s, EV_START_FAILED, DETAIL_OTHER);
                return;
            }
            String cameraId = pickCamera(manager);
            if (cameraId == null) {
                finishSession(s, EV_START_FAILED, DETAIL_NO_CAMERA);
                return;
            }
            CameraCharacteristics chars = manager.getCameraCharacteristics(cameraId);
            StreamConfigurationMap map =
                    chars.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP);
            Size size = map == null ? null : pickSize(map.getOutputSizes(ImageFormat.YUV_420_888));
            if (size == null) {
                finishSession(s, EV_START_FAILED, DETAIL_OTHER);
                return;
            }
            float zmin = Float.NaN;
            float zmax = Float.NaN;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                Range<Float> range = chars.get(CameraCharacteristics.CONTROL_ZOOM_RATIO_RANGE);
                if (range != null) {
                    zmin = range.getLower();
                    zmax = range.getUpper();
                }
            }
            ImageReader reader = ImageReader.newInstance(
                    size.getWidth(), size.getHeight(), ImageFormat.YUV_420_888, 2);
            boolean stale;
            synchronized (lock) {
                stale = s.closed;
                if (!stale) {
                    s.reader = reader;
                    s.zoomMin = zmin;
                    s.zoomMax = zmax;
                }
            }
            if (stale) {
                reader.close();
                return;
            }
            reader.setOnImageAvailableListener(r -> onImageAvailable(s, r), h);
            manager.openCamera(cameraId, new DeviceCallback(s), h);
        } catch (SecurityException e) {
            finishSession(s, EV_START_FAILED, DETAIL_PERMISSION);
        } catch (CameraAccessException e) {
            int reason = e.getReason();
            boolean inUse = reason == CameraAccessException.CAMERA_IN_USE
                    || reason == CameraAccessException.MAX_CAMERAS_IN_USE;
            finishSession(s, EV_START_FAILED, inUse ? DETAIL_IN_USE : DETAIL_OTHER);
        } catch (RuntimeException e) {
            finishSession(s, EV_START_FAILED, DETAIL_OTHER);
        }
    }

    // 背面カメラ、無ければ先頭。空なら null。
    private static String pickCamera(CameraManager manager) throws CameraAccessException {
        String[] ids = manager.getCameraIdList();
        if (ids.length == 0) {
            return null;
        }
        for (String id : ids) {
            Integer facing = manager.getCameraCharacteristics(id).get(CameraCharacteristics.LENS_FACING);
            if (facing != null && facing == CameraCharacteristics.LENS_FACING_BACK) {
                return id;
            }
        }
        return ids[0];
    }

    // 640x480 に面積が近い寸法。長辺 1280 以下を優先し、無ければ最小の寸法。
    private static Size pickSize(Size[] sizes) {
        if (sizes == null || sizes.length == 0) {
            return null;
        }
        Size best = null;
        long bestDiff = Long.MAX_VALUE;
        for (Size s : sizes) {
            if (s.getWidth() <= 0 || s.getHeight() <= 0
                    || Math.max(s.getWidth(), s.getHeight()) > PREFERRED_MAX_SIDE) {
                continue;
            }
            long diff = Math.abs((long) s.getWidth() * s.getHeight() - (long) TARGET_W * TARGET_H);
            if (diff < bestDiff) {
                bestDiff = diff;
                best = s;
            }
        }
        if (best != null) {
            return best;
        }
        for (Size s : sizes) {
            if (s.getWidth() <= 0 || s.getHeight() <= 0) {
                continue;
            }
            if (best == null || (long) s.getWidth() * s.getHeight()
                    < (long) best.getWidth() * best.getHeight()) {
                best = s;
            }
        }
        return best;
    }

    private final class DeviceCallback extends CameraDevice.StateCallback {
        private final Session s;

        DeviceCallback(Session s) {
            this.s = s;
        }

        @Override
        public void onOpened(CameraDevice device) {
            boolean stale;
            synchronized (lock) {
                // 停止済みなら即 close する（開始と取り消しを排他にする。契約表 3）。
                stale = s.closed || request.current() != s.id;
                if (!stale) {
                    s.device = device;
                }
            }
            if (stale) {
                device.close();
                return;
            }
            createSession(device);
        }

        @Override
        public void onDisconnected(CameraDevice device) {
            device.close();
            boolean running = isRunning(s);
            // 開始前の切断は他で使用中として扱う。
            finishSession(s, running ? EV_RUNTIME_FAILED : EV_START_FAILED, DETAIL_IN_USE);
        }

        @Override
        public void onError(CameraDevice device, int error) {
            device.close();
            if (isRunning(s)) {
                finishSession(s, EV_RUNTIME_FAILED, 0);
            } else {
                boolean inUse = error == ERROR_CAMERA_IN_USE || error == ERROR_MAX_CAMERAS_IN_USE;
                finishSession(s, EV_START_FAILED, inUse ? DETAIL_IN_USE : DETAIL_OTHER);
            }
        }
    }

    private boolean isRunning(Session s) {
        synchronized (lock) {
            return s.running;
        }
    }

    private void createSession(CameraDevice device) {
        final Session s;
        final Surface surface;
        synchronized (lock) {
            s = session;
            if (s == null || s.closed || s.reader == null) {
                return;
            }
            surface = s.reader.getSurface();
        }
        CameraCaptureSession.StateCallback cb = new CameraCaptureSession.StateCallback() {
            @Override
            public void onConfigured(CameraCaptureSession cs) {
                onSessionConfigured(s, device, cs, surface);
            }

            @Override
            public void onConfigureFailed(CameraCaptureSession cs) {
                cs.close();
                finishSession(s, EV_START_FAILED, DETAIL_OTHER);
            }
        };
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                SessionConfiguration config = new SessionConfiguration(
                        SessionConfiguration.SESSION_REGULAR,
                        Collections.singletonList(new OutputConfiguration(surface)),
                        r -> s.handler.post(r), cb);
                device.createCaptureSession(config);
            } else {
                device.createCaptureSession(Arrays.asList(surface), cb, s.handler);
            }
        } catch (CameraAccessException | RuntimeException e) {
            finishSession(s, EV_START_FAILED, DETAIL_OTHER);
        }
    }

    private void onSessionConfigured(Session s, CameraDevice device, CameraCaptureSession cs,
            Surface surface) {
        boolean stale;
        synchronized (lock) {
            stale = s.closed;
            if (!stale) {
                s.captureSession = cs;
            }
        }
        if (stale) {
            cs.close();
            return;
        }
        try {
            CaptureRequest.Builder b = device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW);
            b.addTarget(surface);
            CameraCaptureSession.CaptureCallback cc = new CameraCaptureSession.CaptureCallback() {
                @Override
                public void onCaptureStarted(CameraCaptureSession session, CaptureRequest req,
                        long timestamp, long frameNumber) {
                    onFirstCapture(s);
                }

                @Override
                public void onCaptureCompleted(CameraCaptureSession session, CaptureRequest req,
                        TotalCaptureResult result) {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                        Float z = result.get(CaptureResult.CONTROL_ZOOM_RATIO);
                        s.lastZoom = z == null ? Float.NaN : z;
                    }
                }
                // onCaptureFailed は単発の取りこぼしなので失敗にしない。
            };
            synchronized (lock) {
                if (s.closed) {
                    return; // 解放はカメラスレッドの release が済ませる。
                }
                s.builder = b;
                s.captureCallback = cc;
                s.configured = true;
            }
            cs.setRepeatingRequest(b.build(), cc, s.handler);
        } catch (CameraAccessException | RuntimeException e) {
            finishSession(s, EV_START_FAILED, DETAIL_OTHER);
        }
    }

    private void onFirstCapture(Session s) {
        synchronized (lock) {
            if (s.closed || s.running) {
                return;
            }
            s.running = true;
        }
        nativeOnCameraPreviewEvent(s.id, EV_RUNNING, 0);
    }

    // ズームの適用（カメラスレッド）。失敗は EV_ZOOM_FAILED。
    private void applyZoom(Session s, float ratio) {
        final CaptureRequest.Builder b;
        final CameraCaptureSession cs;
        final CameraCaptureSession.CaptureCallback cc;
        synchronized (lock) {
            if (s.closed || s.builder == null || s.captureSession == null) {
                return;
            }
            b = s.builder;
            cs = s.captureSession;
            cc = s.captureCallback;
        }
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                b.set(CaptureRequest.CONTROL_ZOOM_RATIO, ratio);
                cs.setRepeatingRequest(b.build(), cc, s.handler);
            }
        } catch (CameraAccessException | RuntimeException e) {
            nativeOnCameraPreviewEvent(s.id, EV_ZOOM_FAILED, DETAIL_ZOOM_FAILED);
        }
    }

    // フレーム受信（カメラスレッド）。Image は全経路で close する。
    private void onImageAvailable(Session s, ImageReader reader) {
        Image image = null;
        try {
            image = reader.acquireLatestImage();
            if (image == null) {
                return;
            }
            synchronized (lock) {
                if (s.closed || request.current() != s.id) {
                    return;
                }
            }
            Image.Plane[] planes = image.getPlanes();
            if (planes.length != 3) {
                return;
            }
            Image.Plane py = planes[0];
            Image.Plane pu = planes[1];
            Image.Plane pv = planes[2];
            if (py.getPixelStride() != 1
                    || pu.getPixelStride() != pv.getPixelStride()
                    || pu.getRowStride() != pv.getRowStride()) {
                return; // 想定外の並びは捨てる（嘘の画を出さない）。
            }
            nativeOnCameraPreviewFrame(s.id, image.getWidth(), image.getHeight(),
                    py.getBuffer(), py.getRowStride(),
                    pu.getBuffer(), pv.getBuffer(), pu.getRowStride(), pu.getPixelStride(),
                    s.lastZoom);
        } catch (IllegalStateException e) {
            // reader が解放済み・取得上限。次のフレームを待つ。
        } finally {
            if (image != null) {
                image.close();
            }
        }
    }

    // ---- 確定・解放 ----

    // 現在の要求を 1 回だけ確定して解放する（許可拒否・背景・破棄）。要求が無ければ何もしない。
    private void finishRequest(int code, int detail) {
        final long id;
        final Session s;
        synchronized (lock) {
            id = request.take();
            s = session;
            session = null;
            if (s != null) {
                s.closed = true;
            }
        }
        startPendingResume = false;
        if (s != null) {
            release(s);
        }
        if (id != RequestTracker.NONE) {
            nativeOnCameraPreviewEvent(id, code, detail);
        }
    }

    // 1 つの映像を確定して解放する（カメラのコールバックから）。差し替え済み・停止済みなら何もしない。
    private void finishSession(Session s, int code, int detail) {
        final boolean mine;
        synchronized (lock) {
            if (s.closed) {
                return;
            }
            s.closed = true;
            mine = request.complete(s.id);
            if (session == s) {
                session = null;
            }
        }
        release(s);
        if (mine) {
            nativeOnCameraPreviewEvent(s.id, code, detail);
        }
    }

    // 解放をカメラスレッドへ直列化する（capture session → device → reader の順）。
    // スレッドが既に終わっていれば、競合する相手がいないのでその場で行う。
    private void release(Session s) {
        Runnable r = () -> {
            final CameraCaptureSession cs;
            final CameraDevice d;
            final ImageReader reader;
            synchronized (lock) {
                cs = s.captureSession;
                d = s.device;
                reader = s.reader;
                s.captureSession = null;
                s.device = null;
                s.reader = null;
                s.builder = null;
            }
            if (cs != null) {
                try {
                    cs.close();
                } catch (RuntimeException ignored) {
                    // 解放は続ける。
                }
            }
            if (d != null) {
                try {
                    d.close();
                } catch (RuntimeException ignored) {
                    // 解放は続ける。
                }
            }
            if (reader != null) {
                try {
                    reader.close();
                } catch (RuntimeException ignored) {
                    // 解放は続ける。
                }
            }
        };
        if (!s.handler.post(r)) {
            r.run();
        }
    }
}
