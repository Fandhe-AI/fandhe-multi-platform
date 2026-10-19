package com.fandhe.androidapp;

import android.content.Context;
import android.hardware.camera2.CameraAccessException;
import android.hardware.camera2.CameraCharacteristics;
import android.hardware.camera2.CameraManager;
import android.os.Handler;
import android.os.Looper;

// ライト（トーチ）（Issue #461、親 #106）の Android 実装ヘルパー。MainActivity から生成・委譲され、
// Rust の `device/android_flashlight.rs` が egui スレッドから `MainActivity.flashlightSet` /
// `flashlightRead` を JNI で同期呼び出しする。
//
// 結果コードの SSOT は Rust の `fandhe_device::flashlight`（SET_* / READ_*）で、ここはその写し。
// SET_FAILED / READ_FAILED は Rust 側で未知値として `Internal` になる。
//
// `setTorchMode` は呼び出しスレッドを問わないため UI スレッドへは post しない
// （GameActivity の onPause/onStop との相互待ちと、期限後に遅れて適用される経路を作らない）。
// 点灯状態は OS から読み戻す: main Looper で届く TorchCallback の値を `cached` に保持し、
// 最初のコールバックまでだけ READ_UNKNOWN（Rust 側は「まだ分からない」）。
// トーチが使えない通知（onTorchModeUnavailable）は READ_FAILED（Rust 側で Internal）。
//
// ライフサイクル: `onStop` で自分が点けた分を消灯し、背景中の点灯要求は拒否する。前面へ戻っても
// 点け直さない。`onDestroy` で消灯とコールバック解除を済ませ、以後は全要求を拒否する。
// 外部アプリが点けたライトは消さない（`requestedOn` で自分が点けた分だけ片付ける）。
// 同期の一発呼び出し + pull の読み戻しなので、要求 id や保留要求は持たない。
// 守らない範囲: 外部からの点け消しへの追従遅れ、他アプリのカメラ使用中は「分からない」を返す、
// 熱による減光、プロセス終了時の後片付け（OS に任せる）。
final class FlashlightHelper {
    static final int SET_OK = 0;
    static final int SET_UNSUPPORTED = 1;
    static final int SET_FAILED = 2;
    static final int READ_OFF = 0;
    static final int READ_ON = 1;
    static final int READ_UNSUPPORTED = -1;
    static final int READ_UNKNOWN = -2;
    static final int READ_FAILED = 2;

    private final Context context;
    private final Object lock = new Object();
    private CameraManager manager;
    private String cameraId;
    private boolean noFlash;
    private int cached = READ_UNKNOWN;
    private boolean requestedOn;
    private boolean stopped;
    private boolean destroyed;
    private CameraManager.TorchCallback callback;

    FlashlightHelper(Context context) {
        this.context = context;
    }

    // `lock` を保持して呼ぶ。初回だけ搭載確認とコールバック登録を行う。失敗時は false（再試行可）。
    private boolean ensureInit() {
        if (cameraId != null || noFlash) {
            return true;
        }
        try {
            CameraManager m = (CameraManager) context.getSystemService(Context.CAMERA_SERVICE);
            if (m == null) {
                return false;
            }
            String found = null;
            for (String id : m.getCameraIdList()) {
                CameraCharacteristics c = m.getCameraCharacteristics(id);
                if (!Boolean.TRUE.equals(c.get(CameraCharacteristics.FLASH_INFO_AVAILABLE))) {
                    continue;
                }
                if (found == null) {
                    found = id;
                }
                Integer facing = c.get(CameraCharacteristics.LENS_FACING);
                if (facing != null && facing == CameraCharacteristics.LENS_FACING_BACK) {
                    found = id;
                    break;
                }
            }
            if (found == null) {
                noFlash = true;
                return true;
            }
            final String target = found;
            CameraManager.TorchCallback cb = new CameraManager.TorchCallback() {
                @Override
                public void onTorchModeChanged(String id, boolean enabled) {
                    synchronized (lock) {
                        if (target.equals(id)) {
                            cached = enabled ? READ_ON : READ_OFF;
                        }
                    }
                }

                @Override
                public void onTorchModeUnavailable(String id) {
                    synchronized (lock) {
                        if (target.equals(id)) {
                            // 使用中・過熱などで使えない状態。「まだ分からない」とは区別して失敗にする。
                            cached = READ_FAILED;
                        }
                    }
                }
            };
            // egui スレッドには Looper が無いので main Looper を明示する。
            m.registerTorchCallback(cb, new Handler(Looper.getMainLooper()));
            callback = cb;
            manager = m;
            cameraId = target;
            return true;
        } catch (CameraAccessException | RuntimeException e) {
            return false;
        }
    }

    // egui スレッドから呼ぶ。
    int set(boolean on) {
        synchronized (lock) {
            if (destroyed || (on && stopped)) {
                return SET_FAILED;
            }
            if (!ensureInit()) {
                return SET_FAILED;
            }
            if (noFlash) {
                return SET_UNSUPPORTED;
            }
            try {
                manager.setTorchMode(cameraId, on);
                requestedOn = on;
                return SET_OK;
            } catch (CameraAccessException | RuntimeException e) {
                // 例外の文言・カメラ ID は出さない。
                return SET_FAILED;
            }
        }
    }

    // egui スレッドから呼ぶ。失敗時に前回の値を返し続けない。
    int read() {
        synchronized (lock) {
            if (destroyed || !ensureInit()) {
                return READ_FAILED;
            }
            return noFlash ? READ_UNSUPPORTED : cached;
        }
    }

    // `MainActivity.onStart`（UI スレッド）から呼ぶ。点け直しはしない。
    void onStart() {
        synchronized (lock) {
            stopped = false;
        }
    }

    // `MainActivity.onStop`（UI スレッド）から呼ぶ。自分が点けた分だけ消す。
    void onStop() {
        synchronized (lock) {
            stopped = true;
            turnOffIfOwned();
        }
    }

    // `MainActivity.onDestroy`（UI スレッド）から呼ぶ。
    void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            turnOffIfOwned();
            if (manager != null && callback != null) {
                try {
                    manager.unregisterTorchCallback(callback);
                } catch (RuntimeException e) {
                    // 解除失敗でも以後の要求は destroyed で拒否される。
                }
            }
            callback = null;
            cached = READ_UNKNOWN;
        }
    }

    // `lock` を保持して呼ぶ。消灯に失敗したら所有状態を保持し、後続の経路（onDestroy 等）で再試行する。
    private void turnOffIfOwned() {
        if (requestedOn && manager != null && cameraId != null) {
            try {
                manager.setTorchMode(cameraId, false);
                requestedOn = false;
            } catch (CameraAccessException | RuntimeException e) {
                // requestedOn を残す。
            }
        }
    }
}
