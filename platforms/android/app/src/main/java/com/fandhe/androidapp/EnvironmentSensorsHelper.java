package com.fandhe.androidapp;

import android.content.Context;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.os.SystemClock;

// 気圧・照度・近接センサー画面（Issue #611、親 #123）の Android 実装ヘルパー。MainActivity から
// 生成・委譲される（Rust の `device/android_environment_sensors.rs` が egui スレッドから
// `MainActivity.environmentSensorsStart` / `Read` / `Stop` を同期 JNI で呼ぶ。継続配信型のため
// 要求 id・RequestSlot は使わない）。
//
// 結果コード・配列配置の SSOT はここで、Rust 側が同じ値で解釈する。
//   kind : 0=気圧 / 1=照度 / 2=近接（未知の値は START_FAILED・READ_LOST）
//   start: 0=OK / 1=センサー無し / 2=登録失敗・破棄済み・不正な kind
//   read : long[5] = [state, valueBits, extraBits, receivedAtMs, nowMs]
//          state: -1=購読喪失 / 0=値なし / 1=値あり
//          valueBits: Double.doubleToRawLongBits(event.values[0])（hPa / lx / 距離 cm）
//          extraBits: 近接だけ Sensor.getMaximumRange() の double 生ビット。他は 0
//          receivedAtMs / nowMs: SystemClock.elapsedRealtime()（単調時計）
//
// スレッド: start/read/stop は egui スレッド、onSensorChanged は main looper。状態の判定と更新は
// すべて `lock` の内側で行い、main への同期待ちを作らない。権限は不要。センサー値はログに出さない。
//
// ライフサイクル: onPause は解除して値を捨てる（購読の意思は残す）。onResume で新世代のリスナーを
// 再登録する。onDestroy（再生成を含む）は全購読を破棄し、新インスタンスの read は購読喪失を返す。
// 背景遷移は Rust の driver へ nativeOnForegroundChanged で無条件に通知する（再生成後の新インスタンスは
// 未購読で始まるため、条件付きだと note_resumed が呼ばれず Rust 側の背景フラグが残る）。
final class EnvironmentSensorsHelper {
    static final int KIND_PRESSURE = 0;
    static final int KIND_LIGHT = 1;
    static final int KIND_PROXIMITY = 2;
    private static final int KIND_COUNT = 3;

    static final int START_OK = 0;
    static final int START_NOT_PRESENT = 1;
    static final int START_FAILED = 2;

    static final long STATE_LOST = -1L;
    static final long STATE_NONE = 0L;
    static final long STATE_VALUE = 1L;

    // start 成功〜stop までの購読意思（プロセス内で Activity 再生成をまたぐ）。新インスタンスの
    // read が「未開始（値なし）」か「破棄で購読喪失」かの区別に使う。
    private static final boolean[] subscribed = new boolean[KIND_COUNT];

    // 署名 `(Z)V`。実体は Rust の android_environment_sensors.rs、feature `device` 無効ビルドは
    // android.rs の no-op スタブ。3 か所で一致させる。
    private static native void nativeOnForegroundChanged(boolean foreground);

    private static final class Slot {
        final int type;
        boolean wanted;
        boolean paused;
        boolean resumeFailed;
        Listener current;
        boolean hasValue;
        long valueBits;
        long extraBits;
        long receivedAt;

        Slot(int type) {
            this.type = type;
        }
    }

    private final Context context;
    private final Object lock = new Object();
    private SensorManager manager; // lock の内側
    private final Slot[] slots = {
        new Slot(Sensor.TYPE_PRESSURE),
        new Slot(Sensor.TYPE_LIGHT),
        new Slot(Sensor.TYPE_PROXIMITY),
    };
    private boolean foreground = true;
    private boolean destroyed;

    EnvironmentSensorsHelper(Context context) {
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
        long now = SystemClock.elapsedRealtime();
        if (!validKind(kind)) return result(STATE_LOST, 0L, 0L, 0L, now);
        synchronized (lock) {
            Slot s = slots[kind];
            if (!s.wanted) {
                return result((subscribed[kind] || destroyed) ? STATE_LOST : STATE_NONE, 0L, 0L, 0L, now);
            }
            if (s.resumeFailed) return result(STATE_LOST, 0L, 0L, 0L, now);
            if (s.paused || !s.hasValue) return result(STATE_NONE, 0L, 0L, 0L, now);
            return result(STATE_VALUE, s.valueBits, s.extraBits, s.receivedAt, now);
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
        nativeOnForegroundChanged(false);
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
        nativeOnForegroundChanged(true);
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

    private static long[] result(long state, long value, long extra, long receivedAt, long now) {
        return new long[] {state, value, extra, receivedAt, now};
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
        s.valueBits = 0L;
        s.extraBits = 0L;
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
                if (event.values == null || event.values.length < 1) return;
                slot.valueBits = Double.doubleToRawLongBits((double) event.values[0]);
                slot.extraBits = slot.type == Sensor.TYPE_PROXIMITY
                        ? Double.doubleToRawLongBits((double) event.sensor.getMaximumRange())
                        : 0L;
                slot.receivedAt = SystemClock.elapsedRealtime();
                slot.hasValue = true;
            }
        }

        @Override
        public void onAccuracyChanged(Sensor sensor, int accuracy) {}
    }
}
