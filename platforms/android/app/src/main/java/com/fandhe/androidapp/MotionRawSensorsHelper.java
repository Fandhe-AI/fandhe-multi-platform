package com.fandhe.androidapp;

import android.content.Context;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;

// ジャイロ・磁気・線形加速度の生値画面（Issue #901、親 #180）の Android 実装ヘルパー。MainActivity から
// 生成・委譲される（Rust の `device/android_motion_raw_sensors.rs` が egui スレッドから
// `MainActivity.motionRawSensorsStart` / `Read` / `Stop` を同期 JNI で呼ぶ。継続配信型のため
// 要求 id・RequestSlot は使わない）。構造は EnvironmentSensorsHelper と同型。
//
// 結果コード・配列配置の SSOT はここで、Rust 側が同じ値で解釈する。
//   kind : 0=角速度(TYPE_GYROSCOPE) / 1=磁場(TYPE_MAGNETIC_FIELD) / 2=線形加速度(TYPE_LINEAR_ACCELERATION)
//   start: 0=OK / 1=センサー無し / 2=登録失敗・破棄済み・不正な kind
//   read : long[4] = [state, xBits, yBits, zBits]
//          state: -1=購読喪失 / 0=値なし / 1=値あり
//          各 bits: Double.doubleToRawLongBits((double) event.values[i])（rad/s・µT・m/s²）
//
// スレッド: start/read/stop は egui スレッド、onSensorChanged は main looper。状態の判定と更新は
// すべて `lock` の内側で行い、main への同期待ちを作らない。権限は不要。センサー値はログに出さない。
//
// ライフサイクル: onPause は解除して値を捨てる（購読の意思は残す）。onResume で新世代のリスナーを
// 再登録する。onDestroy（再生成を含む）は全購読を破棄し、新インスタンスの read は購読喪失を返す。
// Rust の driver は背景フラグを持たないため、前景・背景の native 通知は無い。
final class MotionRawSensorsHelper {
    static final int START_OK = 0;
    static final int START_NOT_PRESENT = 1;
    static final int START_FAILED = 2;

    static final long STATE_LOST = -1L;
    static final long STATE_NONE = 0L;
    static final long STATE_VALUE = 1L;

    private static final int KIND_COUNT = 3;

    // start 成功〜stop までの購読意思（プロセス内で Activity 再生成をまたぐ）。新インスタンスの
    // read が「未開始（値なし）」か「破棄で購読喪失」かの区別に使う。
    private static final boolean[] subscribed = new boolean[KIND_COUNT];

    private static final class Slot {
        final int type;
        boolean wanted;
        boolean paused;
        boolean resumeFailed;
        Listener current;
        boolean hasValue;
        long x;
        long y;
        long z;

        Slot(int type) {
            this.type = type;
        }
    }

    private final Context context;
    private final Object lock = new Object();
    private SensorManager manager; // lock の内側
    private final Slot[] slots = {
        new Slot(Sensor.TYPE_GYROSCOPE),
        new Slot(Sensor.TYPE_MAGNETIC_FIELD),
        new Slot(Sensor.TYPE_LINEAR_ACCELERATION),
    };
    private boolean foreground = true;
    private boolean destroyed;

    MotionRawSensorsHelper(Context context) {
        this.context = context;
    }

    private static boolean validKind(int kind) {
        return kind >= 0 && kind < KIND_COUNT;
    }

    int start(int kind) {
        if (!validKind(kind)) return START_FAILED;
        synchronized (lock) {
            if (destroyed) return START_FAILED;
            if (manager == null) {
                manager = (SensorManager) context.getSystemService(Context.SENSOR_SERVICE);
            }
            if (manager == null) return START_FAILED;
            Slot s = slots[kind];
            if (s.wanted) {
                subscribed[kind] = true;
                return START_OK;
            }
            if (manager.getDefaultSensor(s.type) == null) return START_NOT_PRESENT;
            s.wanted = true;
            s.resumeFailed = false;
            if (!foreground) {
                // 背景中の start は登録を onResume まで延期する。
                s.paused = true;
                subscribed[kind] = true;
                return START_OK;
            }
            if (!register(s)) {
                s.wanted = false;
                return START_FAILED;
            }
            s.paused = false;
            subscribed[kind] = true;
            return START_OK;
        }
    }

    long[] read(int kind) {
        if (!validKind(kind)) return result(STATE_LOST, 0L, 0L, 0L);
        synchronized (lock) {
            Slot s = slots[kind];
            if (!s.wanted) {
                return result((subscribed[kind] || destroyed) ? STATE_LOST : STATE_NONE, 0L, 0L, 0L);
            }
            if (s.resumeFailed) return result(STATE_LOST, 0L, 0L, 0L);
            if (s.paused || !s.hasValue) return result(STATE_NONE, 0L, 0L, 0L);
            return result(STATE_VALUE, s.x, s.y, s.z);
        }
    }

    void stop(int kind) {
        if (!validKind(kind)) return;
        synchronized (lock) {
            subscribed[kind] = false;
            Slot s = slots[kind];
            s.wanted = false;
            s.resumeFailed = false;
            unregister(s);
        }
    }

    void onPause() {
        synchronized (lock) {
            foreground = false;
            for (Slot s : slots) {
                if (s.wanted && !s.paused) {
                    s.paused = true;
                    unregister(s);
                }
            }
        }
    }

    void onResume() {
        synchronized (lock) {
            foreground = true;
            if (!destroyed) {
                for (Slot s : slots) {
                    if (s.wanted && s.paused) {
                        // 失敗は resumeFailed として保持し、read が購読喪失を返す。
                        if (register(s)) s.paused = false;
                        else s.resumeFailed = true;
                    }
                }
            }
        }
    }

    void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            for (Slot s : slots) {
                s.wanted = false;
                unregister(s);
            }
        }
    }

    private static long[] result(long state, long x, long y, long z) {
        return new long[] {state, x, y, z};
    }

    // lock の内側で呼ぶ。新世代のリスナーで登録し、失敗なら解除する。
    private boolean register(Slot s) {
        unregister(s);
        Sensor sensor = manager.getDefaultSensor(s.type);
        if (sensor == null) return false;
        Listener l = new Listener(s);
        if (!manager.registerListener(l, sensor, SensorManager.SENSOR_DELAY_UI)) {
            manager.unregisterListener(l);
            return false;
        }
        s.current = l;
        return true;
    }

    // lock の内側で呼ぶ。値も捨てる。
    private void unregister(Slot s) {
        if (s.current != null && manager != null) {
            manager.unregisterListener(s.current);
        }
        s.current = null;
        s.hasValue = false;
        s.x = 0L;
        s.y = 0L;
        s.z = 0L;
    }

    private final class Listener implements SensorEventListener {
        private final Slot slot;

        Listener(Slot slot) {
            this.slot = slot;
        }

        @Override
        public void onSensorChanged(SensorEvent event) {
            synchronized (lock) {
                if (slot.current != this) return; // 解除後・旧世代の値は捨てる
                if (event.sensor.getType() != slot.type) return;
                if (event.values == null || event.values.length < 3) return;
                slot.x = Double.doubleToRawLongBits((double) event.values[0]);
                slot.y = Double.doubleToRawLongBits((double) event.values[1]);
                slot.z = Double.doubleToRawLongBits((double) event.values[2]);
                slot.hasValue = true;
            }
        }

        @Override
        public void onAccuracyChanged(Sensor sensor, int accuracy) {}
    }
}
