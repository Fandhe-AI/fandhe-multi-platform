package com.fandhe.androidapp;

import android.Manifest;
import android.content.Context;
import android.content.pm.PackageManager;
import android.net.wifi.ScanResult;
import android.net.wifi.WifiInfo;
import android.net.wifi.WifiManager;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.location.LocationManagerCompat;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;

// Wi-Fi 情報画面（Issue #516）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_wifi_info.rs` が `MainActivity.requestWifiInfo` / `cancelWifiInfo`
// を JNI で呼び、結果を `nativeOnWifiInfoResult` で受ける）。
//
// 1 要求 = 機能有無 → 精密位置情報の許可 → 位置情報サービス → Wi-Fi の電源 → 読み取り。
// 空の結果が「0 件」か「許可・設定が足りず OS が空を返した」か区別できないため、読む前に
// すべて判定して失敗にする。自分からスキャンは起こさず（startScan を呼ばない）、
// `getScanResults()` で OS のキャッシュを読むだけ。SSID の整形・dBm の値域・上位 5 件への
// 絞り込みは Rust 側（契約の SSOT）に一本化し、ここは素の値を送る。
// 読み取りは binder 数回の短い呼び出しなので UI スレッドで同期実行する。
// ponytail: UI スレッドで binder 数回。重くなったら名前付きスレッドへ移す。
// `cancel` は UI キューの停滞に依存しないよう呼び出しスレッドで即時に処理する。
//
// ライフサイクル: `onPause` では破棄しない（許可ダイアログ自体が pause を起こすため）。
// `onDestroy`（Activity 再生成を含む）では保留中の要求を FAILED で確定して破棄する。
// `request` はインスタンスフィールドなので、再生成後の新インスタンスは NONE から始まり、
// 旧インスタンスの結果は `complete(id)` に失敗して捨てられる。
final class WifiInfoHelper {
    // 結果コード。SSOT は Rust 側 `android_wifi_info.rs::decode_code`。
    static final int RESULT_OK = 0;
    static final int RESULT_UNSUPPORTED = 1;
    static final int RESULT_PERMISSION_DENIED = 2;
    static final int RESULT_RADIO_OFF = 3;
    static final int RESULT_LOCATION_OFF = 4;
    static final int RESULT_FAILED = 5;

    // Rust 側 `NEARBY_CAP` と同じ。
    private static final int NEARBY_CAP = 64;

    private static native void nativeOnWifiInfoResult(
            long requestId, int code, boolean connected, String currentSsid, int currentRssi,
            String[] ssids, int[] rssis);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    private final Object lock = new Object();
    // 取り消し済みの最大 id（lock の内側）。cancel が start より先に走っても古い要求を開始しない。
    private long cancelledId = RequestTracker.NONE;
    // 許可ダイアログを起動した要求 id（UI スレッド専用）。
    private long permissionId = RequestTracker.NONE;
    // UI スレッドで最後に受理した id。これ以下の開始要求は追い越された古い要求として捨てる。
    private long latestId = RequestTracker.NONE;

    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity の
    // フィールド初期化子から本クラスを生成してここで登録する。
    private final ActivityResultLauncher<String[]> permissionLauncher;

    WifiInfoHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.RequestMultiplePermissions(), this::onPermissionResult);
    }

    // JNI スレッドから呼ばれる。
    void read(long id) {
        activity.runOnUiThread(() -> start(id));
    }

    // Rust 側の期限切れから呼ばれる。
    void cancel(long id) {
        synchronized (lock) {
            cancelledId = Math.max(cancelledId, id);
            request.complete(id);
        }
    }

    // MainActivity.onDestroy から UI スレッドで呼ぶ。
    void onDestroy() {
        long id;
        synchronized (lock) {
            id = request.take();
        }
        if (id != RequestTracker.NONE) {
            nativeOnWifiInfoResult(id, RESULT_FAILED, false, null, 0, null, null);
        }
    }

    private boolean destroyed() {
        return activity.isFinishing() || activity.isDestroyed();
    }

    private boolean fineGranted() {
        return activity.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION)
                == PackageManager.PERMISSION_GRANTED;
    }

    private void start(long id) {
        if (id <= latestId) {
            return;
        }
        latestId = id;
        if (destroyed()) {
            nativeOnWifiInfoResult(id, RESULT_FAILED, false, null, 0, null, null);
            return;
        }
        synchronized (lock) {
            if (id <= cancelledId) {
                return;
            }
            request.track(id);
        }
        if (!activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_WIFI)
                || wifi() == null) {
            finishIf(id, RESULT_UNSUPPORTED);
            return;
        }
        checkPermissionAndRead(id);
    }

    private WifiManager wifi() {
        return (WifiManager) activity.getApplicationContext().getSystemService(Context.WIFI_SERVICE);
    }

    private void checkPermissionAndRead(long id) {
        if (!fineGranted()) {
            // ダイアログは同時に 1 つだけ。出ている間に差し替わった要求は結果到着時に再判定する。
            if (permissionId == RequestTracker.NONE) {
                permissionId = id;
                // Android 12 以降は FINE 単独の要求が無視されるため COARSE と同時に要求する。
                // 「おおよそ」のみの許可は FINE 未許可として PERMISSION_DENIED にする。
                permissionLauncher.launch(new String[] {
                        Manifest.permission.ACCESS_FINE_LOCATION,
                        Manifest.permission.ACCESS_COARSE_LOCATION});
            }
            return;
        }
        readNow(id);
    }

    // 許可ダイアログ自体が Activity を pause させるため、resumed では分岐しない。
    private void onPermissionResult(Map<String, Boolean> result) {
        long launched = permissionId;
        permissionId = RequestTracker.NONE;
        long current = request.current();
        if (current == RequestTracker.NONE) {
            return; // 期限切れ・破棄済み・再生成後のインスタンス。
        }
        if (destroyed()) {
            finishIf(current, RESULT_FAILED);
        } else if (launched != current) {
            checkPermissionAndRead(current);
        } else if (!fineGranted()) {
            finishIf(current, RESULT_PERMISSION_DENIED);
        } else {
            readNow(current);
        }
    }

    @SuppressWarnings({"deprecation", "MissingPermission"})
    private void readNow(long id) {
        if (!LocationManagerCompat.isLocationEnabled(
                (android.location.LocationManager)
                        activity.getSystemService(Context.LOCATION_SERVICE))) {
            finishIf(id, RESULT_LOCATION_OFF);
            return;
        }
        WifiManager wm = wifi();
        try {
            if (!wm.isWifiEnabled()) {
                finishIf(id, RESULT_RADIO_OFF);
                return;
            }
            // API 31 以降は非推奨だが、位置情報の許可があれば SSID が得られる（#510 で実機確認）。
            WifiInfo info = wm.getConnectionInfo();
            boolean connected = info != null && info.getNetworkId() != -1
                    && !WifiManager.UNKNOWN_SSID.equals(info.getSSID());
            String currentSsid = connected ? unquote(info.getSSID()) : null;
            int currentRssi = connected ? info.getRssi() : 0;

            List<ScanResult> scans = wm.getScanResults();
            List<ScanResult> sorted = scans == null
                    ? new ArrayList<>() : new ArrayList<>(scans);
            Collections.sort(sorted, (a, b) -> Integer.compare(b.level, a.level));
            int n = Math.min(sorted.size(), NEARBY_CAP);
            String[] ssids = new String[n];
            int[] rssis = new int[n];
            for (int i = 0; i < n; i++) {
                String s = sorted.get(i).SSID;
                ssids[i] = (s == null || s.isEmpty()) ? null : s;
                rssis[i] = sorted.get(i).level;
            }
            finishIf(id, RESULT_OK, connected, currentSsid, currentRssi, ssids, rssis);
        } catch (SecurityException e) {
            finishIf(id, RESULT_PERMISSION_DENIED);
        } catch (RuntimeException e) {
            finishIf(id, RESULT_FAILED); // SSID を含みうるため内容はログに出さない。
        }
    }

    // `getSSID()` はダブルクォートで囲まれた UTF-8 文字列、または囲みなしの 16 進表記を返す。
    // 囲みなしは文字列として扱えないため null（「非公開」と同じ表示）にする。
    private static String unquote(String s) {
        if (s != null && s.length() >= 2 && s.startsWith("\"") && s.endsWith("\"")) {
            return s.substring(1, s.length() - 1);
        }
        return null;
    }

    private void finishIf(long id, int code) {
        finishIf(id, code, false, null, 0, null, null);
    }

    // 現在の要求のときだけ 1 回確定する。古い結果は照合で捨てる。
    private void finishIf(long id, int code, boolean connected, String ssid, int rssi,
            String[] ssids, int[] rssis) {
        synchronized (lock) {
            if (!request.complete(id)) {
                return;
            }
        }
        nativeOnWifiInfoResult(id, code, connected, ssid, rssi, ssids, rssis);
    }
}
