package com.fandhe.androidapp;

import android.content.Context;
import android.content.pm.PackageManager;
import android.media.midi.MidiDevice;
import android.media.midi.MidiDeviceInfo;
import android.media.midi.MidiManager;
import android.media.midi.MidiOutputPort;
import android.media.midi.MidiReceiver;
import android.os.Handler;
import android.os.HandlerThread;

import androidx.activity.ComponentActivity;

import java.io.IOException;
import java.util.Arrays;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Iterator;
import java.util.Map;
import java.util.Set;

// MIDI 入力画面（Issue #1400、親 #1137）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_midi.rs` が `MainActivity.midiStart` / `midiStop` を JNI で呼び、
// 結果を `nativeOnMidi*` で受ける）。範囲は USB 等で `getDevices` に出る入力（出力ポートを持つ
// デバイス）の受信のみ。BLE の接続（`openBluetoothDevice`）、出力、デバイス名は扱わない。
//
// 結果コード（`start` の戻り値）。SSOT はここで、Rust 側 `android_midi.rs::decode_start` が同じ値で写像する。
//   0 = 受け付けた（確定は非同期の nativeOnMidiStarted）/ 1 = 非対応 / 2 = 失敗
//
// 世代（epoch）: Rust が採番した値を `start` で受け、全 native コールバックへそのまま添える。
// 停止・再生成後に遅れて届く open 完了や受信は epoch が `current` と違うため、ここで捨てる
// （Rust 側でも照合する）。
//
// スレッドとロック: OS のコールバックは専用の HandlerThread で受け、状態の変更は `lock` の
// 内側で行う（制御ロックはこの 1 本だけ）。`stop` / `onDestroy` は呼び出しスレッドでロックを取り、
// 同期で close と登録解除を済ませてから戻る。ロック取得順は Java → Rust の一方向
// （Rust は自分のロックを手放してから本クラスを呼ぶ）。受信（`Receiver.onSend`）はロックを
// 取らず、`current` の volatile 読みで判定する。
//
// ライフサイクル: `onPause` では何もしない（背景での受信継続は守らない範囲）。`onDestroy`
// （Activity 再生成を含む）は購読を破棄する。新インスタンスは epoch を持たない状態から始まる。
// 演奏内容・デバイス名・例外メッセージはログへ出さない。
final class MidiHelper {
    static final int START_OK = 0;
    static final int START_UNSUPPORTED = 1;
    static final int START_FAILED = 2;

    private static final long NONE = 0L;

    private static native void nativeOnMidiStarted(long epoch, int sources, int failed);

    private static native void nativeOnMidiSources(long epoch, int sources, int failed);

    private static native void nativeOnMidiData(long epoch, long key, byte[] bytes);

    // 入力ソース key の running status・未完成メッセージを捨てる（onFlush とポート抜去で呼ぶ）。
    private static native void nativeOnMidiFlush(long epoch, long key);

    private final ComponentActivity activity;
    private final Object lock = new Object();
    // 以下 lock の内側でのみ読み書きする（current は受信スレッドから volatile で読む）。
    private volatile long current = NONE;
    private boolean destroyed;
    // openDevice を出して完了通知がまだ来ていない数。0 になるまで HandlerThread を終了しない
    // （終了後に届く完了通知は配送先がなく、開いたデバイスが解放されないため）。
    private int inflight;
    private boolean startedSent;
    private boolean callbackRegistered;
    private HandlerThread thread;
    private Handler handler;
    private MidiManager manager;
    // 開いている最中のデバイス id。
    private final Set<Integer> pending = new HashSet<>();
    private final Map<Integer, MidiDevice> devices = new HashMap<>();
    private final Map<Long, MidiOutputPort> ports = new HashMap<>();
    // デバイス id ごとの接続できなかった出力ポート数。
    private final Map<Integer, Integer> failedByDevice = new HashMap<>();

    MidiHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    private final MidiManager.DeviceCallback deviceCallback = new MidiManager.DeviceCallback() {
        @Override
        public void onDeviceAdded(MidiDeviceInfo info) {
            synchronized (lock) {
                if (current == NONE || destroyed) return;
                openLocked(info, current);
                // 同期の open 失敗は onOpened が届かないため、ここで確定した failed を通知する。
                // 非同期 open 中は pending が残り reportLocked が見送る。start の列挙は同じ lock の
                // 内側で済むため、ここで Started が早出しされることはない。
                reportLocked(current);
            }
        }

        @Override
        public void onDeviceRemoved(MidiDeviceInfo info) {
            synchronized (lock) {
                if (current == NONE || destroyed) return;
                int id = info.getId();
                pending.remove(id);
                failedByDevice.remove(id);
                closeDeviceLocked(id);
                reportLocked(current);
            }
        }
    };

    // 0 = 受け付けた / 1 = 非対応 / 2 = 失敗。
    @SuppressWarnings("deprecation")
    int start(long epoch) {
        synchronized (lock) {
            if (destroyed) return START_FAILED;
            if (!activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_MIDI)) {
                return START_UNSUPPORTED;
            }
            if (manager == null) {
                manager = (MidiManager) activity.getApplicationContext()
                        .getSystemService(Context.MIDI_SERVICE);
            }
            if (manager == null) return START_UNSUPPORTED;
            try {
                closeAllLocked();
                if (thread == null) {
                    thread = new HandlerThread("fandhe-midi");
                    thread.start();
                    handler = new Handler(thread.getLooper());
                }
                current = epoch;
                startedSent = false;
                manager.registerDeviceCallback(deviceCallback, handler);
                callbackRegistered = true;
                for (MidiDeviceInfo info : manager.getDevices()) {
                    openLocked(info, epoch);
                }
                reportLocked(epoch);
                return START_OK;
            } catch (RuntimeException e) {
                // 確保した分を解放してから失敗で返す。
                current = NONE;
                closeAllLocked();
                return START_FAILED;
            }
        }
    }

    // 冪等。戻った後は native を呼ばない（Rust 側も epoch を進めて捨てる）。
    void stop() {
        synchronized (lock) {
            current = NONE;
            closeAllLocked();
        }
    }

    void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            current = NONE;
            closeAllLocked();
            quitThreadIfIdleLocked();
        }
    }

    // lock の内側で呼ぶ。破棄済みで未完了の open が無ければスレッドを終了する。
    // ponytail: 完了通知が永久に来ない場合はスレッドが残る。タイムアウトが要るなら別途。
    private void quitThreadIfIdleLocked() {
        if (destroyed && inflight == 0 && thread != null) {
            thread.quitSafely();
            thread = null;
            handler = null;
        }
    }

    // lock の内側で呼ぶ。出力ポートを持つデバイスだけを開く。
    private void openLocked(MidiDeviceInfo info, long epoch) {
        final int id = info.getId();
        final int portCount = info.getOutputPortCount();
        if (portCount <= 0 || pending.contains(id) || devices.containsKey(id)) return;
        pending.add(id);
        inflight++;
        try {
            manager.openDevice(info, device -> onOpened(epoch, id, portCount, device), handler);
        } catch (RuntimeException e) {
            inflight--;
            pending.remove(id);
            failedByDevice.put(id, portCount);
        }
    }

    // OS の open 完了（HandlerThread）。
    private void onOpened(long epoch, int id, int portCount, MidiDevice device) {
        synchronized (lock) {
            inflight--;
            // 停止・再生成・抜去済みなら開いたものをそのまま閉じる。
            if (epoch != current || destroyed || !pending.remove(id)) {
                closeQuietly(device);
                quitThreadIfIdleLocked();
                return;
            }
            if (device == null) {
                failedByDevice.put(id, portCount);
            } else {
                devices.put(id, device);
                int failed = 0;
                for (int i = 0; i < portCount; i++) {
                    try {
                        MidiOutputPort port = device.openOutputPort(i);
                        if (port == null) {
                            failed++;
                            continue;
                        }
                        long key = ((long) id << 32) | (i & 0xFFFFFFFFL);
                        port.connect(new Receiver(epoch, key));
                        ports.put(key, port);
                    } catch (RuntimeException e) {
                        failed++;
                    }
                }
                if (failed > 0) failedByDevice.put(id, failed);
            }
            reportLocked(epoch);
        }
    }

    // lock の内側で呼ぶ。open が全部済んでいれば入力数を通知する（初回は Started、以降は Sources）。
    private void reportLocked(long epoch) {
        if (!pending.isEmpty()) return;
        int failed = 0;
        for (int n : failedByDevice.values()) failed += n;
        if (!startedSent) {
            startedSent = true;
            nativeOnMidiStarted(epoch, ports.size(), failed);
        } else {
            nativeOnMidiSources(epoch, ports.size(), failed);
        }
    }

    // lock の内側で呼ぶ。1 デバイスのポートとデバイスを閉じる。
    private void closeDeviceLocked(int id) {
        for (Iterator<Map.Entry<Long, MidiOutputPort>> it = ports.entrySet().iterator(); it.hasNext(); ) {
            Map.Entry<Long, MidiOutputPort> e = it.next();
            if ((e.getKey() >> 32) == id) {
                closeQuietly(e.getValue());
                nativeOnMidiFlush(current, e.getKey());
                it.remove();
            }
        }
        closeQuietly(devices.remove(id));
    }

    // lock の内側で呼ぶ。全ポート・デバイスを閉じ、抜き差しの登録を解除する。
    @SuppressWarnings("deprecation")
    private void closeAllLocked() {
        for (MidiOutputPort p : ports.values()) closeQuietly(p);
        for (MidiDevice d : devices.values()) closeQuietly(d);
        ports.clear();
        devices.clear();
        pending.clear();
        failedByDevice.clear();
        if (callbackRegistered && manager != null) {
            try {
                manager.unregisterDeviceCallback(deviceCallback);
            } catch (RuntimeException ignored) {
                // 登録済みでなければ何もしない。
            }
        }
        callbackRegistered = false;
    }

    private static void closeQuietly(java.io.Closeable c) {
        if (c == null) return;
        try {
            c.close();
        } catch (IOException | RuntimeException ignored) {
            // 閉じ失敗は解放の試行以上のことをしない。
        }
    }

    // 1 つの出力ポートの受信。OS がバッファを使い回すため切り出して渡す。
    private final class Receiver extends MidiReceiver {
        private final long epoch;
        private final long key;

        Receiver(long epoch, long key) {
            this.epoch = epoch;
            this.key = key;
        }

        @Override
        public void onSend(byte[] msg, int offset, int count, long timestamp) {
            if (epoch != current || count <= 0) return;
            nativeOnMidiData(epoch, key, Arrays.copyOfRange(msg, offset, offset + count));
        }

        @Override
        public void onFlush() {
            if (epoch != current) return;
            nativeOnMidiFlush(epoch, key);
        }
    }
}
