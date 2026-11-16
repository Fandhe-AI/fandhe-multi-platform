package com.fandhe.platform;

import android.Manifest;
import android.app.Activity;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothManager;
import android.bluetooth.le.BluetoothLeScanner;
import android.bluetooth.le.ScanCallback;
import android.bluetooth.le.ScanRecord;
import android.bluetooth.le.ScanResult;
import android.bluetooth.le.ScanSettings;
import android.content.Context;
import android.content.pm.PackageManager;
import android.location.LocationManager;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;
import androidx.core.location.LocationManagerCompat;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// BLE スキャン（Issue #1062）の Android 実装ヘルパー。Activity ごとに 1 つ、`BluetoothLeLifecycle`
// （`FeatureLifecycleProvider` 継承）が `attach` で生成して `HELPERS` に持つ。Rust の `fandhe-cap-bluetooth-le`
// （`src/android.rs`）の `request_ble_scan` が static の `requestBleScan(Activity, long, int)` を呼び、広告ごとに
// `nativeOnBleScanResult`、終了時に 1 回だけ `nativeOnBleScanFinished` で通知する
// （macOS / Windows と共通の `OBSERVED` / `bridge` へ pull 型で橋渡し）。
// 同一 id の統合・RSSI 降順・上限は Rust 側 `normalize_scan` に任せ、ここでは並べ替えも
// 重複除去もしない。完了コード → 契約のエラーの写像は Rust 側 `error_from_android_code` が SSOT（ここでは分岐しない）。
// helper が無い（生成前・破棄後）要求は成功にも待ちにもせず、内部エラーで確定する。
//
// 並行性（device-bridge-contracts.md 2・11 節）: 状態を触る経路は UI スレッド 1 本に
// 集約する。`requestBleScan` は `runOnUiThread`、`ScanCallback` は main looper へ届く
// （Handler 未指定の既定）、期限 `Handler` も main looper、ライフサイクルも UI
// スレッドのため、ロックは持たない。通知は `request.complete(id)` が true の
// ときだけ行い（first-wins）、二重完了と古い世代の通知を防ぐ。
//
// 期限: Rust の `SCAN_DURATION_MS` を `durationMs` で受けてそのまま使う（複製しない）。
// `startScan` 成功時点から数え、権限ダイアログの回答待ちには掛けない（macOS と同じ
// 既知の天井）。周辺機器の名前・アドレスはログへ出さない（security.md A09）。
//
// 許可ランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。登録キーは従来の文字列のまま。
//
// 結果コード（Rust 側 `error_from_android_code` と一致させる）: 0=成功／1=Bluetooth オフ／
// 2=権限拒否／3=非搭載・LE 非対応／4=内部エラー／5=中断（`onPause`/`onDestroy`/非前景での
// 開始）／6=位置情報サービスがオフ（API 30 以下。オフのままだと 0 件になり「0 台の成功」に
// 化けるため失敗にする）。
final class BluetoothLeHelper {
    private static final int RESULT_OK = 0;
    private static final int RESULT_DISABLED = 1;
    private static final int RESULT_PERMISSION_DENIED = 2;
    private static final int RESULT_UNSUPPORTED = 3;
    private static final int RESULT_INTERNAL_ERROR = 4;
    private static final int RESULT_INTERRUPTED = 5;
    private static final int RESULT_LOCATION_OFF = 6;

    // `address` は識別子、`name` は広告名（無ければ null）、`rssi` は `ScanResult.getRssi()`
    // の生値（取得不能の番兵 127 も Rust 側が `None` へ写す）。
    private static native void nativeOnBleScanResult(
            long requestId, String address, String name, int rssi);

    private static native void nativeOnBleScanFinished(long requestId, int code);

    // Activity ごとの helper（上記クラス冒頭コメント参照）。
    private static final Map<Activity, BluetoothLeHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private final ComponentActivity activity;
    private final ActivityResultLauncher<String[]> permissionLauncher;

    // 以下は UI スレッドだけで読み書きする。
    private final RequestTracker request = new RequestTracker();
    private final Handler handler = new Handler(Looper.getMainLooper());
    private BluetoothLeScanner scanner;
    private ScanCallback scanCallback;
    private Runnable scanTimeout;
    private int scanDurationMs;
    // 権限ダイアログの回答待ち。表示中に来た再要求は新しい id を追跡し直すだけで、
    // 結果は「その時点の現在 id」へ適用する（要求する権限は同一のため）。
    private boolean permissionPending;
    // 許可結果が非前景中に届いたため `startScan` を次の `onResume` へ保留している
    // （正常な許可を中断扱いにせず、背景での開始も避ける、契約表 1 節）。
    private boolean startPendingResume;
    private boolean foreground;

    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、`BluetoothLeLifecycle.onHostCreated`
    // （`Activity.onCreate` の中）から `attach` で生成する。
    private BluetoothLeHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.getActivityResultRegistry().register(
                "fandhe:MainActivity:9", activity, new ActivityResultContracts.RequestMultiplePermissions(),
                this::onPermissionResult);
    }

    // Activity の生成時（`BluetoothLeLifecycle` から、UI スレッド）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new BluetoothLeHelper(activity));
    }

    // Activity の破棄時（同上）。再生成を含む。スキャン中・権限ダイアログ待ちを問わず止めて、追跡中の
    // 要求があれば中断で確定する（破棄後に結果を届ける先は無いが Rust 側を畳む）。
    static void detach(Activity activity) {
        BluetoothLeHelper helper = HELPERS.remove(activity);
        if (helper != null) {
            helper.stopScanQuietly();
            helper.permissionPending = false;
            helper.startPendingResume = false;
            helper.finish(helper.request.current(), RESULT_INTERRUPTED);
        }
    }

    // `BluetoothLeLifecycle` から（UI スレッド）。許可結果が非前景中に届き開始を保留していた要求を再開する。
    // `Activity.onResume()` の中、つまり `super.onResume()` の中で届く。従来の `MainActivity` は `super` の後で
    // 呼んでいたため、`super` に対する前後だけが違う。
    static void resumed(Activity activity) {
        BluetoothLeHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.foreground = true;
            if (helper.startPendingResume) {
                helper.startPendingResume = false;
                long id = helper.request.current();
                if (id != RequestTracker.NONE && helper.scanCallback == null) {
                    helper.startScan(id);
                }
            }
        }
    }

    // `BluetoothLeLifecycle` から（UI スレッド）。バックグラウンド遷移でスキャン中なら止めて中断で確定する。
    // 権限ダイアログ待ち（`scanCallback == null`）は破棄しない（ダイアログ自体が `onPause` を起こすため）。
    static void paused(Activity activity) {
        BluetoothLeHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.foreground = false;
            if (helper.scanCallback != null) {
                long id = helper.request.current();
                helper.stopScanQuietly();
                helper.finish(id, RESULT_INTERRUPTED);
            }
        }
    }

    // Rust から呼ばれる開始依頼（Rust スレッド → UI スレッドへ集約）。helper が無い要求は内部エラーで確定する。
    static void requestBleScan(Activity activity, final long requestId, final int durationMs) {
        BluetoothLeHelper helper = HELPERS.get(activity);
        if (helper == null) {
            nativeOnBleScanFinished(requestId, RESULT_INTERNAL_ERROR);
            return;
        }
        activity.runOnUiThread(() -> helper.begin(requestId, durationMs));
    }

    // API 31 以上は `BLUETOOTH_SCAN` + `BLUETOOTH_CONNECT`、30 以下は `ACCESS_FINE_LOCATION`
    // （宣言は Cargo.toml の `android.permissions`。API 31 以上のみ CONNECT を要求するため minSdk 未満の分岐は不要）。
    private static String[] permissions() {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
                ? new String[] {
                    Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT
                }
                : new String[] {Manifest.permission.ACCESS_FINE_LOCATION};
    }

    private boolean granted() {
        for (String p : permissions()) {
            if (ContextCompat.checkSelfPermission(activity, p) != PackageManager.PERMISSION_GRANTED) {
                return false;
            }
        }
        return true;
    }

    private void onPermissionResult(Map<String, Boolean> result) {
        boolean granted = true;
        for (String p : permissions()) {
            granted &= Boolean.TRUE.equals(result.get(p));
        }
        permissionPending = false;
        long id = request.current();
        if (id == RequestTracker.NONE) {
            return;
        }
        if (!granted) {
            finish(id, RESULT_PERMISSION_DENIED);
            return;
        }
        if (activity.isFinishing() || activity.isDestroyed()) {
            finish(id, RESULT_INTERRUPTED);
            return;
        }
        if (!foreground) {
            startPendingResume = true;
            return;
        }
        startScan(id);
    }

    // 要求を 1 回だけ確定して通知する（追跡中の id と一致したときのみ）。
    private void finish(long id, int code) {
        if (id != RequestTracker.NONE && request.complete(id)) {
            nativeOnBleScanFinished(id, code);
        }
    }

    // タイマー解除 → `stopScan` → 参照解放を全終了経路（期限・失敗・置き換え・
    // `onPause`・`onDestroy`）で必ず通す。Bluetooth オフ時の `stopScan` が投げる
    // 例外は握る（契約表 9 節: 失敗経路でも資源を残さない）。
    private void stopScanQuietly() {
        if (scanTimeout != null) {
            handler.removeCallbacks(scanTimeout);
            scanTimeout = null;
        }
        if (scanner != null && scanCallback != null) {
            try {
                scanner.stopScan(scanCallback);
            } catch (RuntimeException ignored) {
                // SecurityException / IllegalStateException（Bluetooth オフ）。
            }
        }
        scanner = null;
        scanCallback = null;
    }

    // UI スレッド。置き換え: 進行中のスキャンは止め、旧 id は通知せず破棄する（Rust 側は既に新世代）。
    private void begin(final long requestId, final int durationMs) {
        stopScanQuietly();
        request.track(requestId);
        scanDurationMs = durationMs;
        if (permissionPending || startPendingResume) {
            return;
        }
        if (activity.isFinishing() || activity.isDestroyed() || !foreground) {
            finish(requestId, RESULT_INTERRUPTED);
            return;
        }
        // 非搭載の判定を権限より先に行い、非対応端末で権限ダイアログを出さない。
        if (!activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)
                || adapter() == null) {
            finish(requestId, RESULT_UNSUPPORTED);
            return;
        }
        if (!granted()) {
            permissionPending = true;
            try {
                permissionLauncher.launch(permissions());
            } catch (RuntimeException e) {
                permissionPending = false;
                finish(requestId, RESULT_INTERNAL_ERROR);
            }
            return;
        }
        startScan(requestId);
    }

    private BluetoothAdapter adapter() {
        BluetoothManager manager = (BluetoothManager) activity.getSystemService(Context.BLUETOOTH_SERVICE);
        return manager == null ? null : manager.getAdapter();
    }

    private static int failureCode(int scanErrorCode) {
        return scanErrorCode == ScanCallback.SCAN_FAILED_FEATURE_UNSUPPORTED
                ? RESULT_UNSUPPORTED
                : RESULT_INTERNAL_ERROR;
    }

    private void startScan(final long id) {
        final BluetoothAdapter adapter = adapter();
        if (adapter == null) {
            finish(id, RESULT_UNSUPPORTED);
            return;
        }
        // API 31+ の `isEnabled()` / `BluetoothDevice.getAddress()` は `BLUETOOTH_CONNECT` を
        // 要しうるため、実行時許可に含めている。それでも SecurityException が出たら落とさず権限拒否で確定する。
        final BluetoothLeScanner leScanner;
        try {
            leScanner = adapter.isEnabled() ? adapter.getBluetoothLeScanner() : null;
        } catch (SecurityException e) {
            finish(id, RESULT_PERMISSION_DENIED);
            return;
        }
        if (leScanner == null) {
            finish(id, RESULT_DISABLED);
            return;
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            // サービスが取れない端末は位置 OFF と同じ扱い（null を渡すと NPE になる）。
            LocationManager lm = (LocationManager) activity.getSystemService(Context.LOCATION_SERVICE);
            if (lm == null || !LocationManagerCompat.isLocationEnabled(lm)) {
                finish(id, RESULT_LOCATION_OFF);
                return;
            }
        }
        ScanCallback callback = new ScanCallback() {
            @Override
            public void onScanResult(int callbackType, ScanResult result) {
                if (request.current() != id || result.getDevice() == null) {
                    return;
                }
                // `BluetoothDevice.getName()` は API 31+ で BLUETOOTH_CONNECT を要するため
                // 使わず、広告の ScanRecord から名前を取る。
                ScanRecord record = result.getScanRecord();
                final String address;
                try {
                    address = result.getDevice().getAddress();
                } catch (SecurityException e) {
                    // スキャン中に権限が取り消された（API 31+）。止めて権限拒否で確定する。
                    stopScanQuietly();
                    finish(id, RESULT_PERMISSION_DENIED);
                    return;
                }
                nativeOnBleScanResult(
                        id, address, record == null ? null : record.getDeviceName(), result.getRssi());
            }

            @Override
            public void onScanFailed(int errorCode) {
                if (request.current() != id) {
                    return;
                }
                stopScanQuietly();
                finish(id, failureCode(errorCode));
            }
        };
        ScanSettings settings =
                new ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build();
        try {
            leScanner.startScan(null, settings, callback);
        } catch (SecurityException e) {
            finish(id, RESULT_PERMISSION_DENIED);
            return;
        } catch (RuntimeException e) {
            finish(id, RESULT_INTERNAL_ERROR);
            return;
        }
        scanner = leScanner;
        scanCallback = callback;
        scanTimeout = () -> {
            scanTimeout = null;
            if (request.current() != id) {
                return;
            }
            // 途中で Bluetooth がオフになった場合は失敗にする（部分結果は Rust 側で捨てる）。
            boolean stillOn;
            try {
                stillOn = adapter.isEnabled();
            } catch (SecurityException e) {
                stillOn = false;
            }
            // API 30 以下は途中で位置情報サービスが OFF になると広告が届かず 0 台の偽の成功になる。
            boolean locationOff = false;
            if (stillOn && Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
                LocationManager lm = (LocationManager) activity.getSystemService(Context.LOCATION_SERVICE);
                locationOff = lm == null || !LocationManagerCompat.isLocationEnabled(lm);
            }
            stopScanQuietly();
            finish(
                    id,
                    !stillOn
                            ? RESULT_DISABLED
                            : locationOff ? RESULT_LOCATION_OFF : RESULT_OK);
        };
        handler.postDelayed(scanTimeout, scanDurationMs);
    }
}
