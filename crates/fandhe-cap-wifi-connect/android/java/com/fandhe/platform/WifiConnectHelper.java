package com.fandhe.platform;

import android.app.Activity;
import android.content.Context;
import android.content.pm.PackageManager;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.NetworkRequest;
import android.net.wifi.WifiManager;
import android.net.wifi.WifiNetworkSpecifier;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;

import androidx.activity.ComponentActivity;
import androidx.annotation.RequiresApi;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// 指定した Wi-Fi への接続画面（Issue #1065、親 #1060）の Android 実装ヘルパー。利用者の `MainActivity` には何も書かせない。
// Rust の `src/android.rs` が static の `connect(Activity, long, String, String, boolean)` を JNI で呼び、
// 結果コードを `nativeOnWifiConnectResult` で受ける。Activity の生成・破棄は `WifiConnectLifecycle`
// （非公開 ContentProvider、`FeatureLifecycleProvider` 継承）が `attach` / `detach` で知らせる。Activity ごとの helper は
// `HELPERS` に持つ（JNI スレッドと UI スレッドの両方から触るので同期化し、Activity が GC されれば項目も消える）。
// helper が無い（生成前・破棄後）要求は FAILED で確定し、結果が来ないまま待たせない。
//
// `WifiNetworkSpecifier`（API 29+）を付けた `requestNetwork` を 1 回出し、OS のダイアログで承認されて
// `onAvailable` が最初に届いたときだけ OK を返す。`onUnavailable` は拒否・見つからない・接続失敗・
// Wi-Fi オフが区別できないため、電源だけ読み直して RADIO_OFF とそれ以外の FAILED に分ける。
// 成功した要求は OS が接続を保つ間は握り続け、次の接続要求か `onDestroy` で解放する
// （解放すると OS が接続を切りうる）。
//
// 状態（`registered`・追跡 id）は main looper だけで触る（要求の受付・callback・onDestroy を直列化）。
// 16 進 64 文字の鍵の判定は Rust 側が行い、`hexKey` で受ける（定数を複製しない）。
// SSID とパスワードはログ・例外文に出さない。
//
// ライフサイクル: `onPause` では破棄しない（OS のダイアログ自体が pause を起こすため）。
// `onDestroy`（Activity 再生成を含む）では保留中の要求を FAILED で確定し、登録を解放する。
// `request` はインスタンスフィールドなので、再生成後の新インスタンスは NONE から始まり、
// 旧インスタンスの結果は `complete(id)` に失敗して捨てられる。
final class WifiConnectHelper {
    // 結果コード。SSOT は Rust 側 `android.rs::decode`。
    static final int RESULT_OK = 0;
    static final int RESULT_UNSUPPORTED = 1;
    static final int RESULT_RADIO_OFF = 2;
    static final int RESULT_FAILED = 3;

    private static native void nativeOnWifiConnectResult(long requestId, int code);

    // Activity ごとの helper（上記クラス冒頭コメント参照）。
    private static final Map<Activity, WifiConnectHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    // 登録中の callback（保留中の要求、または成功して握っている要求）。main looper 専用。
    private ConnectivityManager.NetworkCallback registered;

    private WifiConnectHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // Activity の生成時（`WifiConnectLifecycle` から、UI スレッド）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new WifiConnectHelper(activity));
    }

    // Activity の破棄時（同上）。保留中の要求を FAILED で確定し、登録を解放する。
    static void detach(ComponentActivity activity) {
        WifiConnectHelper helper = HELPERS.remove(activity);
        if (helper != null) {
            helper.onDestroy();
        }
    }

    // JNI スレッドから呼ばれる。helper が無い（生成前・破棄後）ときは FAILED で確定する。
    // 以降の処理は main looper へ移す。
    static void connect(Activity activity, long requestId, String ssid, String password, boolean hexKey) {
        WifiConnectHelper helper = HELPERS.get(activity);
        if (helper == null) {
            nativeOnWifiConnectResult(requestId, RESULT_FAILED);
            return;
        }
        helper.activity.runOnUiThread(() -> helper.start(requestId, ssid, password, hexKey));
    }

    private void onDestroy() {
        long id = request.take();
        if (id != RequestTracker.NONE) {
            nativeOnWifiConnectResult(id, RESULT_FAILED);
        }
        release();
    }

    private void start(long id, String ssid, String password, boolean hexKey) {
        // 前の要求（成功して握っている分を含む）は新しい登録より前に解放する。
        release();
        long stale = request.take();
        if (stale != RequestTracker.NONE) {
            nativeOnWifiConnectResult(stale, RESULT_FAILED);
        }
        if (activity.isFinishing() || activity.isDestroyed()) {
            nativeOnWifiConnectResult(id, RESULT_FAILED);
            return;
        }
        request.track(id);
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            finishIf(id, RESULT_UNSUPPORTED);
            return;
        }
        ConnectivityManager cm =
                (ConnectivityManager) activity.getSystemService(Context.CONNECTIVITY_SERVICE);
        WifiManager wm = (WifiManager) activity.getApplicationContext()
                .getSystemService(Context.WIFI_SERVICE);
        if (cm == null || wm == null
                || !activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_WIFI)) {
            finishIf(id, RESULT_UNSUPPORTED);
            return;
        }
        if (hexKey) {
            // Specifier は鍵を ASCII のパスフレーズとして扱うため OS へ渡さない。
            finishIf(id, RESULT_FAILED);
            return;
        }
        try {
            ConnectivityManager.NetworkCallback cb = new Callback(id, wm);
            requestNetworkQ(cm, ssid, password, cb);
            registered = cb;
        } catch (RuntimeException e) {
            // 例外の文言に SSID が入りうるためログに出さない。
            finishIf(id, RESULT_FAILED);
        }
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private void requestNetworkQ(
            ConnectivityManager cm, String ssid, String password,
            ConnectivityManager.NetworkCallback cb) {
        WifiNetworkSpecifier.Builder spec = new WifiNetworkSpecifier.Builder().setSsid(ssid);
        if (password != null) {
            spec.setWpa2Passphrase(password);
        }
        NetworkRequest req = new NetworkRequest.Builder()
                .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
                .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                .setNetworkSpecifier(spec.build())
                .build();
        cm.requestNetwork(req, cb, new Handler(Looper.getMainLooper()));
    }

    // この id がまだ待っている要求なら結果を返す（first-wins）。
    private void finishIf(long id, int code) {
        if (request.complete(id)) {
            nativeOnWifiConnectResult(id, code);
        }
    }

    // 解放の呼び出しはここ 1 か所。未登録（解放済み）は無視する。
    private void release() {
        ConnectivityManager.NetworkCallback cb = registered;
        registered = null;
        if (cb == null) {
            return;
        }
        try {
            ConnectivityManager cm =
                    (ConnectivityManager) activity.getSystemService(Context.CONNECTIVITY_SERVICE);
            if (cm != null) {
                cm.unregisterNetworkCallback(cb);
            }
        } catch (IllegalArgumentException ignored) {
            // 登録されていない（OS が解放済み）。
        }
    }

    // 要求ごとの callback。main looper で呼ばれる。
    private final class Callback extends ConnectivityManager.NetworkCallback {
        private final long id;
        private final WifiManager wifi;

        Callback(long id, WifiManager wifi) {
            this.id = id;
            this.wifi = wifi;
        }

        @Override
        public void onAvailable(Network network) {
            // 最初の 1 回だけ成功にする。callback は接続を握るため registered に残す。
            finishIf(id, RESULT_OK);
        }

        @Override
        public void onUnavailable() {
            // OS は要求を解放済みなので unregister は呼ばない。
            if (registered == this) {
                registered = null;
            }
            if (!request.complete(id)) {
                return;
            }
            int code = RESULT_FAILED;
            try {
                if (!wifi.isWifiEnabled()) {
                    code = RESULT_RADIO_OFF;
                }
            } catch (RuntimeException ignored) {
                // 読めなければ FAILED のまま。
            }
            nativeOnWifiConnectResult(id, code);
        }
    }
}
