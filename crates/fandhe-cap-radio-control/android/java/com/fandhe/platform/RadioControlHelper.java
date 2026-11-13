package com.fandhe.platform;

import android.Manifest;
import android.annotation.SuppressLint;
import android.app.Activity;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothManager;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.provider.Settings;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.lifecycle.Lifecycle;

// 無線（Wi-Fi・Bluetooth）の状態取得と切り替え（Issue #974、親 #189）の Android 実装ヘルパー。
// Rust の `src/android.rs` が static の `read(Activity, int)`（同期）と `set(Activity, long, int, boolean)`（非同期）を
// JNI で呼び、結果を `nativeOnRadioControlResult` で受けて契約（`contract`）の結果へ写像する。
// Activity ごとの helper は `HELPERS` に持ち、`RadioControlLifecycle`（`FeatureLifecycleProvider` 継承）が
// `attach` / `detach` で知らせる。helper が無い（生成前・破棄後）`set` は SET_FAILED で確定し、結果が来ないまま待たせない。
//
// `WifiManager.setWifiEnabled` と `BluetoothAdapter.enable/disable` は targetSdk 35 では常に失敗する
// ため使わない。Wi-Fi の切り替えと Bluetooth のオフは設定画面を開いて OPENED、Bluetooth のオンは
// ACTION_REQUEST_ENABLE（OK は APPLIED、CANCELED は NOT_CHANGED で失敗にしない）。
// Intent は固定の action のみで、外部から action・URI・extra は受けない。
//
// ライフサイクル: `onPause` では破棄しない（OS のダイアログや設定画面が pause を起こすため）。
// `onDestroy`（Activity 再生成を含む）では保留中の要求を INTERRUPTED で確定する。`request` は
// インスタンスフィールドで、再生成後の新インスタンスは NONE から始まり、旧結果は捨てられる。
// 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止。守らない範囲: 設定画面での利用者の操作、
// 読み直し時点の切り替わり途中、ダイアログや設定画面の待ち時間。
// id・結果コード・例外メッセージ・アダプター情報はログに出さない。
final class RadioControlHelper {
    // 種類。Rust 側 `android.rs::kind_code` と対。
    static final int KIND_WIFI = 0;
    static final int KIND_BLUETOOTH = 1;

    // 読み取りの戻り値。Rust 側 `decode_read` が同じ値で写像する（SSOT は本クラス）。
    static final int READ_ON = 0;
    static final int READ_OFF = 1;
    static final int READ_TRANSITIONING = 2;
    static final int READ_UNKNOWN = 3;
    static final int READ_NOT_PRESENT = 4;
    static final int READ_PERMISSION_DENIED = 5;
    static final int READ_FAILED = 6;

    // 切り替えの結果コード。Rust 側 `decode_switch` が同じ値で写像する。
    static final int SET_APPLIED = 0;
    static final int SET_OPENED = 1;
    static final int SET_NOT_CHANGED = 2;
    static final int SET_UNSUPPORTED = 3;
    static final int SET_PERMISSION_DENIED = 4;
    static final int SET_INTERRUPTED = 5;
    static final int SET_FAILED = 6;

    private static native void nativeOnRadioControlResult(long requestId, int code);

    // Activity ごとの helper。再生成後の新しい Activity は新しい helper を持ち、旧要求は持ち越さない。
    private static final Map<Activity, RadioControlHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();

    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、`attach`（`onHostCreated`）で生成してここで登録する。
    private final ActivityResultLauncher<String> permissionLauncher;
    private final ActivityResultLauncher<Intent> enableLauncher;

    private RadioControlHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.getActivityResultRegistry().register("fandhe:RadioControlHelper:1", activity,
                new ActivityResultContracts.RequestPermission(), this::onPermissionResult);
        this.enableLauncher = activity.getActivityResultRegistry().register("fandhe:RadioControlHelper:2", activity,
                new ActivityResultContracts.StartActivityForResult(), result -> {
                    long id = request.current();
                    if (id == RequestTracker.NONE) {
                        return;
                    }
                    finish(id, result.getResultCode() == android.app.Activity.RESULT_OK
                            ? SET_APPLIED : SET_NOT_CHANGED);
                });
    }

    // Activity の生成時（`RadioControlLifecycle` から、UI スレッド）。ランチャーを登録する。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new RadioControlHelper(activity));
    }

    // Activity の破棄時（同上）。保留中の要求を INTERRUPTED で確定する。
    static void detach(Activity activity) {
        RadioControlHelper helper = HELPERS.remove(activity);
        if (helper != null) {
            helper.onDestroy();
        }
    }

    // 任意スレッドから同期で呼ばれる。helper にも Activity の状態にも依存しない。getState の許可不足は
    // SecurityException を PERMISSION_DENIED へ写して扱う。
    @SuppressLint("MissingPermission")
    static int read(Activity activity, int kind) {
        Context ctx = activity.getApplicationContext();
        try {
            if (kind == KIND_WIFI) {
                if (!ctx.getPackageManager().hasSystemFeature(PackageManager.FEATURE_WIFI)) {
                    return READ_NOT_PRESENT;
                }
                WifiManager wm = (WifiManager) ctx.getSystemService(Context.WIFI_SERVICE);
                if (wm == null) {
                    return READ_NOT_PRESENT;
                }
                switch (wm.getWifiState()) {
                    case WifiManager.WIFI_STATE_ENABLED:
                        return READ_ON;
                    case WifiManager.WIFI_STATE_DISABLED:
                        return READ_OFF;
                    case WifiManager.WIFI_STATE_ENABLING:
                    case WifiManager.WIFI_STATE_DISABLING:
                        return READ_TRANSITIONING;
                    default:
                        return READ_UNKNOWN;
                }
            }
            if (kind == KIND_BLUETOOTH) {
                BluetoothAdapter adapter = adapter(ctx);
                if (adapter == null) {
                    return READ_NOT_PRESENT;
                }
                switch (adapter.getState()) {
                    case BluetoothAdapter.STATE_ON:
                        return READ_ON;
                    case BluetoothAdapter.STATE_OFF:
                        return READ_OFF;
                    case BluetoothAdapter.STATE_TURNING_ON:
                    case BluetoothAdapter.STATE_TURNING_OFF:
                        return READ_TRANSITIONING;
                    default:
                        return READ_UNKNOWN;
                }
            }
            return READ_FAILED;
        } catch (SecurityException e) {
            return READ_PERMISSION_DENIED;
        } catch (RuntimeException e) {
            return READ_FAILED;
        }
    }

    private static BluetoothAdapter adapter(Context ctx) {
        BluetoothManager bm = (BluetoothManager) ctx.getSystemService(Context.BLUETOOTH_SERVICE);
        return bm == null ? null : bm.getAdapter();
    }

    // JNI スレッドから呼ばれる入口。helper が無ければ結果を SET_FAILED で確定する。
    static void set(Activity activity, long id, int kind, boolean on) {
        RadioControlHelper helper = HELPERS.get(activity);
        if (helper == null) {
            nativeOnRadioControlResult(id, SET_FAILED);
            return;
        }
        helper.request.track(id);
        helper.activity.runOnUiThread(() -> helper.start(id, kind, on));
    }

    private boolean destroyed() {
        return activity.isFinishing() || activity.isDestroyed();
    }

    // 完了は必ずここを通す（first-wins。差し替え済み・破棄済みの旧要求は黙って捨てる）。
    private void finish(long id, int code) {
        if (request.complete(id)) {
            nativeOnRadioControlResult(id, code);
        }
    }

    private void start(long id, int kind, boolean on) {
        // 差し替え済みの旧要求は結果を返さず捨てる。
        if (request.current() != id) {
            return;
        }
        if (destroyed()
                || !activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            finish(id, SET_FAILED);
            return;
        }
        if (kind == KIND_WIFI) {
            if (!activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_WIFI)) {
                finish(id, SET_UNSUPPORTED);
                return;
            }
            String action = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q
                    ? Settings.Panel.ACTION_WIFI : Settings.ACTION_WIFI_SETTINGS;
            finish(id, openSettings(action));
        } else if (kind == KIND_BLUETOOTH) {
            if (adapter(activity.getApplicationContext()) == null) {
                finish(id, SET_UNSUPPORTED);
            } else if (!on) {
                finish(id, openSettings(Settings.ACTION_BLUETOOTH_SETTINGS));
            } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
                    && activity.checkSelfPermission(Manifest.permission.BLUETOOTH_CONNECT)
                            != PackageManager.PERMISSION_GRANTED) {
                permissionLauncher.launch(Manifest.permission.BLUETOOTH_CONNECT);
            } else {
                launchEnable(id);
            }
        } else {
            finish(id, SET_FAILED);
        }
    }

    private int openSettings(String action) {
        try {
            activity.startActivity(new Intent(action));
            return SET_OPENED;
        } catch (ActivityNotFoundException e) {
            return SET_UNSUPPORTED;
        } catch (SecurityException e) {
            return SET_PERMISSION_DENIED;
        } catch (RuntimeException e) {
            return SET_FAILED;
        }
    }

    // 許可ダイアログの応答と onPause の順序が入れ替わっても中断扱いにしないため、RESUMED は見ない。
    private void onPermissionResult(boolean granted) {
        long id = request.current();
        if (id == RequestTracker.NONE) {
            return;
        }
        if (!granted) {
            finish(id, SET_PERMISSION_DENIED);
        } else if (destroyed()) {
            finish(id, SET_INTERRUPTED);
        } else {
            launchEnable(id);
        }
    }

    // ACTION_REQUEST_ENABLE は API 31+ では BLUETOOTH_CONNECT が必要。許可は呼び出し前に確認済み。
    @SuppressLint("MissingPermission")
    private void launchEnable(long id) {
        try {
            enableLauncher.launch(new Intent(BluetoothAdapter.ACTION_REQUEST_ENABLE));
        } catch (ActivityNotFoundException e) {
            finish(id, SET_UNSUPPORTED);
        } catch (SecurityException e) {
            finish(id, SET_PERMISSION_DENIED);
        } catch (RuntimeException e) {
            finish(id, SET_FAILED);
        }
    }

    // `detach`（UI スレッド）から呼ぶ。
    private void onDestroy() {
        long id = request.take();
        if (id != RequestTracker.NONE) {
            nativeOnRadioControlResult(id, SET_INTERRUPTED);
        }
    }
}
