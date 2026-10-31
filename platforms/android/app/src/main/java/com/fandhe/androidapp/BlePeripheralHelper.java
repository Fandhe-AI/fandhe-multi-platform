package com.fandhe.androidapp;

import android.Manifest;
import android.annotation.SuppressLint;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothGatt;
import android.bluetooth.BluetoothGattCharacteristic;
import android.bluetooth.BluetoothGattServer;
import android.bluetooth.BluetoothGattServerCallback;
import android.bluetooth.BluetoothGattService;
import android.bluetooth.BluetoothManager;
import android.bluetooth.le.AdvertiseCallback;
import android.bluetooth.le.AdvertiseData;
import android.bluetooth.le.AdvertiseSettings;
import android.bluetooth.le.BluetoothLeAdvertiser;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.os.Build;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.os.ParcelUuid;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;
import androidx.lifecycle.Lifecycle;

import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.CountDownLatch;

// BLE の周辺機器役（Issue #1123、親 #164 の b-android）の Android 実装ヘルパー。固定の GATT サービス 1 つと
// 読み取り専用の特性 1 つを公開し、アドバタイズする。書き込み・通知・ペアリング・背景でのアドバタイズは範囲外。
// MainActivity から生成・委譲される。Rust の `device/android_ble_peripheral.rs` が
// `MainActivity.blePeripheralStart`（開始）と `blePeripheralStop`（停止）を JNI で呼び、事象は native 2 本で返す。
// UUID と値は Rust の定数を引数で受け取る（Java に複製しない）。端末名は scan response に OS が載せる。
//
// 並行性: 公開の状態（current・GATT server・advertiser・receiver）は HandlerThread 1 本（bg）専用でロックを
// 持たない。AdvertiseCallback・BluetoothGattServerCallback（binder スレッド）・state の broadcast は
// すべて bg へ post し、bg 上で OS の完了を待たない。許可ダイアログだけ main で扱い、結果を bg へ post する。
// コールバックは start ごとに token を捕まえ、current と違う旧世代の通知は捨てる。
//
// ライフサイクル: onStop で資源を解放して中断（コード 6）で確定し、自動では再開しない。onPause は見ない
// （許可ダイアログは onPause しか起こさないため）。onDestroy も同様に確定してから bg スレッドを終える。
// 再生成後の新インスタンスは空から始まり、旧 token の通知は Rust 側 reset_pending_tracking と照合で捨てられる。
//
// レシーバーは RECEIVER_EXPORTED で登録する（システムの broadcast は NOT_EXPORTED では届かない）。
// ACTION_STATE_CHANGED は protected broadcast だが、受信側でも action と extra を検証する。
//
// 結果コードの SSOT は Rust の `fandhe_device::ble_peripheral::CODE_*`。ここは値を合わせるだけで写像の
// 分岐をしない（開始失敗の errorCode は生のまま Rust へ渡す）。中心役のアドレス・名前・読み取りの中身・
// OS のエラーコードはログへ出さない。
// 守らない範囲: 他アプリの同時アドバタイズ、端末名が長く scan response が DATA_TOO_LARGE になる場合
// （Failed になる）、背景でのアドバタイズ、Activity 再生成をまたぐ公開の継続。
final class BlePeripheralHelper {
    // 値は Rust の CODE_*（SSOT）に合わせる。
    private static final int CODE_ADVERTISING = 0;
    private static final int CODE_READ = 1;
    private static final int CODE_BLUETOOTH_OFF = 2;
    private static final int CODE_PERMISSION_DENIED = 3;
    private static final int CODE_UNSUPPORTED = 4;
    private static final int CODE_FAILED = 5;
    private static final int CODE_INTERRUPTED = 6;

    // stop の戻り値（Rust の stop_outcome_from_apple と合わせる）。
    private static final int STOP_STOPPED = 0;
    private static final int STOP_NOT_RUNNING = 1;
    private static final int STOP_FAILED = 2;

    private static native void nativeOnBlePeripheralEvent(long token, int code);

    private static native void nativeOnBlePeripheralStartFailure(long token, int errorCode);

    private final ComponentActivity activity;
    private final Handler mainHandler = new Handler(Looper.getMainLooper());
    private final HandlerThread thread = new HandlerThread("ble-peripheral");
    private final Handler bg;
    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity のフィールド初期化子で生成する。
    private final ActivityResultLauncher<String[]> permissionLauncher;

    // main 専用。ダイアログ表示中に再要求が来たら、launch せずこの token だけ差し替える。
    private boolean permissionPending;
    private long permissionToken;

    // 以下は bg 専用。current == 0 は動いていない（token は 1 から払い出される）。
    // main の permissionStep も停止済み世代の判定に読むので volatile（書くのは bg のみ）。
    private volatile long current;
    private UUID serviceUuid;
    private UUID characteristicUuid;
    private byte[] value;
    private BluetoothGattServer gattServer;
    private BluetoothLeAdvertiser advertiser;
    private AdvertiseCallback advertiseCallback;
    private BroadcastReceiver receiver;
    // 解放に失敗した資源が残っている間 true。start() が読むので volatile。回収できるまで新しい公開を受け付けない。
    private volatile boolean unreleased;

    BlePeripheralHelper(ComponentActivity activity) {
        this.activity = activity;
        thread.start();
        bg = new Handler(thread.getLooper());
        permissionLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.RequestMultiplePermissions(), this::onPermissionResult);
    }

    // egui スレッドから同期で呼ばれる。0 = 受理、1 = 受理できなかった。
    int start(long token, String service, String characteristic, String text) {
        final UUID svc;
        final UUID chr;
        try {
            svc = UUID.fromString(service);
            chr = UUID.fromString(characteristic);
        } catch (IllegalArgumentException | NullPointerException e) {
            return 1;
        }
        final byte[] bytes = (text == null ? "" : text).getBytes(StandardCharsets.UTF_8);
        boolean posted = bg.post(() -> {
            // 進行中の公開は通知なしで置き換える。前回の解放失敗が残っていれば、ここで回収し直す。
            if (!teardown()) {
                // 旧公開の資源を回収できない。新しい公開は始めず、失敗として通知する。
                current = 0;
                nativeOnBlePeripheralEvent(token, CODE_FAILED);
                return;
            }
            current = token;
            serviceUuid = svc;
            characteristicUuid = chr;
            value = bytes;
            mainHandler.post(() -> permissionStep(token));
        });
        return posted ? 0 : 1;
    }

    // egui スレッドから同期で呼ばれる。bg は main を待たないのでデッドロックしない。
    int stop() {
        final int[] result = {STOP_FAILED};
        final CountDownLatch latch = new CountDownLatch(1);
        boolean posted = bg.post(() -> {
            try {
                if (current == 0) {
                    // 停止済みでも、前回の解放失敗が残っていれば回収し直す。
                    result[0] = !unreleased ? STOP_NOT_RUNNING
                            : teardown() ? STOP_STOPPED : STOP_FAILED;
                } else {
                    current = 0;
                    result[0] = teardown() ? STOP_STOPPED : STOP_FAILED;
                }
            } finally {
                latch.countDown();
            }
        });
        if (!posted) {
            // looper 終了済み。onDestroy で解放済み。
            return STOP_NOT_RUNNING;
        }
        try {
            latch.await();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            return STOP_FAILED;
        }
        return result[0];
    }

    void onStop() {
        interrupt();
    }

    void onDestroy() {
        interrupt();
        thread.quitSafely();
    }

    private void interrupt() {
        bg.post(() -> {
            if (current != 0) {
                end(current, CODE_INTERRUPTED);
            } else if (unreleased) {
                // 解放失敗の資源を、画面が終わる前にもう一度回収する。
                teardown();
            }
        });
    }

    // ---- main: 実行時許可 ----

    private void permissionStep(long token) {
        if (token != current) {
            // 積まれた後に stop・置き換えで停止済みの世代。許可を求めない。
            return;
        }
        if (!activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.STARTED)) {
            bg.post(() -> end(token, CODE_INTERRUPTED));
            return;
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            // API 30 以下は BLUETOOTH / BLUETOOTH_ADMIN がインストール時許可。
            bg.post(() -> begin(token));
            return;
        }
        permissionToken = token;
        if (permissionPending) {
            return;
        }
        permissionPending = true;
        // 許可済みなら OS がダイアログなしですぐ結果を返す（事前に問い合わせない）。
        permissionLauncher.launch(new String[] {
                Manifest.permission.BLUETOOTH_ADVERTISE, Manifest.permission.BLUETOOTH_CONNECT});
    }

    private void onPermissionResult(Map<String, Boolean> result) {
        permissionPending = false;
        final long token = permissionToken;
        // 取消（空 Map）と一部拒否は拒否扱い。
        boolean granted = !result.isEmpty();
        for (Boolean b : result.values()) {
            if (!Boolean.TRUE.equals(b)) {
                granted = false;
            }
        }
        final boolean ok = granted;
        bg.post(() -> {
            if (token != current) {
                return;
            }
            if (ok) {
                begin(token);
            } else {
                end(token, CODE_PERMISSION_DENIED);
            }
        });
    }

    // ---- bg: 公開 ----

    @SuppressLint("MissingPermission")
    private void begin(long token) {
        if (token != current) {
            return;
        }
        try {
            // 先に受信を登録して、判定から登録までの間の state 変化を落とさない。
            if (!registerStateReceiver(token)) {
                end(token, CODE_FAILED);
                return;
            }
            Context app = activity.getApplicationContext();
            BluetoothManager manager = (BluetoothManager) app.getSystemService(Context.BLUETOOTH_SERVICE);
            BluetoothAdapter adapter = manager == null ? null : manager.getAdapter();
            if (adapter == null) {
                end(token, CODE_UNSUPPORTED);
                return;
            }
            if (adapter.getState() != BluetoothAdapter.STATE_ON) {
                end(token, CODE_BLUETOOTH_OFF);
                return;
            }
            advertiser = adapter.getBluetoothLeAdvertiser();
            if (advertiser == null || !adapter.isMultipleAdvertisementSupported()) {
                // 状態を読み直し、オフならオフ、そうでなければ非対応。
                end(token, adapter.getState() == BluetoothAdapter.STATE_ON
                        ? CODE_UNSUPPORTED : CODE_BLUETOOTH_OFF);
                return;
            }
            gattServer = manager.openGattServer(app, new GattCb(token));
            if (gattServer == null) {
                end(token, CODE_FAILED);
                return;
            }
            BluetoothGattService service =
                    new BluetoothGattService(serviceUuid, BluetoothGattService.SERVICE_TYPE_PRIMARY);
            service.addCharacteristic(new BluetoothGattCharacteristic(characteristicUuid,
                    BluetoothGattCharacteristic.PROPERTY_READ,
                    BluetoothGattCharacteristic.PERMISSION_READ));
            if (!gattServer.addService(service)) {
                end(token, CODE_FAILED);
            }
            // 続きは onServiceAdded（bg へ post される）。
        } catch (SecurityException e) {
            end(token, CODE_PERMISSION_DENIED);
        } catch (RuntimeException e) {
            end(token, CODE_FAILED);
        }
    }

    @SuppressLint("MissingPermission")
    private void startAdvertising(long token) {
        if (token != current || advertiser == null) {
            return;
        }
        try {
            AdvertiseSettings settings = new AdvertiseSettings.Builder()
                    .setConnectable(true).setTimeout(0).build();
            // 128 bit UUID と flags と独自名では legacy の 31B を超えるため、名前は scan response へ分ける。
            AdvertiseData data = new AdvertiseData.Builder()
                    .setIncludeDeviceName(false).addServiceUuid(new ParcelUuid(serviceUuid)).build();
            AdvertiseData scanResponse = new AdvertiseData.Builder().setIncludeDeviceName(true).build();
            advertiseCallback = new AdvCb(token);
            advertiser.startAdvertising(settings, data, scanResponse, advertiseCallback);
        } catch (SecurityException e) {
            end(token, CODE_PERMISSION_DENIED);
        } catch (RuntimeException e) {
            end(token, CODE_FAILED);
        }
    }

    private boolean registerStateReceiver(long token) {
        BroadcastReceiver r = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                if (intent == null || !BluetoothAdapter.ACTION_STATE_CHANGED.equals(intent.getAction())) {
                    return;
                }
                int state = intent.getIntExtra(BluetoothAdapter.EXTRA_STATE, BluetoothAdapter.ERROR);
                if (state == BluetoothAdapter.STATE_TURNING_OFF || state == BluetoothAdapter.STATE_OFF) {
                    // 受信は bg の Handler 上で届く。自動では再開しない。
                    end(token, CODE_BLUETOOTH_OFF);
                }
            }
        };
        // 参照を保存してから登録し、登録から保存までの間に届いた通知を落とさない。
        receiver = r;
        try {
            ContextCompat.registerReceiver(activity, r,
                    new IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED), null, bg,
                    ContextCompat.RECEIVER_EXPORTED);
            return true;
        } catch (RuntimeException e) {
            receiver = null;
            return false;
        }
    }

    @SuppressLint("MissingPermission")
    private void onRead(long token, BluetoothDevice device, int requestId, int offset, UUID uuid) {
        if (token != current || gattServer == null) {
            return;
        }
        try {
            if (!characteristicUuid.equals(uuid)) {
                gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_READ_NOT_PERMITTED, offset, null);
                return;
            }
            if (offset < 0 || offset > value.length) {
                gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_INVALID_OFFSET, offset, null);
                return;
            }
            boolean sent = gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset,
                    Arrays.copyOfRange(value, offset, value.length));
            // offset 0 の成功応答だけを 1 件と数える。
            if (sent && offset == 0) {
                nativeOnBlePeripheralEvent(token, CODE_READ);
            }
        } catch (RuntimeException e) {
            // 応答に失敗した読み取りは数えない。公開自体は続ける。
        }
    }

    // 唯一の確定点。1 公開につき高々 1 回（token == current のときだけ）。先に current を空にし、
    // 解放を済ませてから通知する。解放の失敗で理由を上書きしない。
    private void end(long token, int code) {
        if (!release(token)) {
            return;
        }
        nativeOnBlePeripheralEvent(token, code);
    }

    private void endWithStartFailure(long token, int errorCode) {
        if (!release(token)) {
            return;
        }
        nativeOnBlePeripheralStartFailure(token, errorCode);
    }

    private boolean release(long token) {
        if (token != current) {
            return false;
        }
        current = 0;
        teardown();
        return true;
    }

    // 何度呼んでも安全。各解放を個別に捕まえ、きれいに解放できたかを返す。失敗した資源は参照を残し（unreleased = true）、
    // 次の stop() の teardown で回収し直す。失敗した段より後の資源も解放を試みる。
    @SuppressLint("MissingPermission")
    private boolean teardown() {
        boolean clean = true;
        if (advertiser != null && advertiseCallback != null) {
            try {
                advertiser.stopAdvertising(advertiseCallback);
                advertiser = null;
                advertiseCallback = null;
            } catch (IllegalStateException e) {
                // adapter がオフ。OS が片付け済み。
                advertiser = null;
                advertiseCallback = null;
            } catch (RuntimeException e) {
                clean = false;
            }
        } else {
            advertiser = null;
            advertiseCallback = null;
        }
        if (gattServer != null) {
            try {
                gattServer.close();
                gattServer = null;
            } catch (RuntimeException e) {
                clean = false;
            }
        }
        if (receiver != null) {
            try {
                activity.unregisterReceiver(receiver);
                receiver = null;
            } catch (IllegalArgumentException e) {
                // 解除済みは無視する。
                receiver = null;
            } catch (RuntimeException e) {
                clean = false;
            }
        }
        unreleased = !clean;
        return clean;
    }

    private final class GattCb extends BluetoothGattServerCallback {
        private final long token;

        GattCb(long token) {
            this.token = token;
        }

        @Override
        public void onServiceAdded(int status, BluetoothGattService service) {
            bg.post(() -> {
                if (token != current) {
                    return;
                }
                if (status == BluetoothGatt.GATT_SUCCESS) {
                    startAdvertising(token);
                } else {
                    end(token, CODE_FAILED);
                }
            });
        }

        @Override
        public void onCharacteristicReadRequest(BluetoothDevice device, int requestId, int offset,
                BluetoothGattCharacteristic characteristic) {
            final UUID uuid = characteristic.getUuid();
            bg.post(() -> onRead(token, device, requestId, offset, uuid));
        }
    }

    private final class AdvCb extends AdvertiseCallback {
        private final long token;

        AdvCb(long token) {
            this.token = token;
        }

        @Override
        public void onStartSuccess(AdvertiseSettings settingsInEffect) {
            // 「公開中」はここだけ。
            bg.post(() -> {
                if (token == current) {
                    nativeOnBlePeripheralEvent(token, CODE_ADVERTISING);
                }
            });
        }

        @Override
        public void onStartFailure(int errorCode) {
            bg.post(() -> endWithStartFailure(token, errorCode));
        }
    }
}
