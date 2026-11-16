package com.fandhe.platform;

import android.Manifest;
import android.app.Activity;
import android.content.pm.PackageManager;
import android.location.Location;
import android.location.LocationListener;
import android.location.LocationManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Looper;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;
import androidx.core.location.LocationManagerCompat;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// 位置情報（Issue #890。ジオフェンス #1058 も共用）の Android 実装ヘルパー。
// Rust の `fandhe-cap-location/src/android.rs` が static の `requestLocationUpdates(Activity, boolean)` を JNI で
// 呼び、実測位は `nativeOnLocationChanged` / `nativeOnLocationFailure` / `nativeOnLocationRetrying` で継続的に
// 届く（継続 push 型。要求 id を持たない）。Activity のライフサイクルへは `LocationLifecycle`（非公開
// ContentProvider、`FeatureLifecycleProvider` 継承）が `attach` / `resumed` / `paused` / `detach` で知らせる。
// Activity ごとの helper は `HELPERS` に持つ（JNI スレッドと UI スレッドの両方から触るので同期化し、Activity が
// GC されれば項目も消える）。
//
// 「測位を続けたい」意思・権限ダイアログの結果待ち・失敗通知済みの印は Activity 再生成を跨いで保持するため
// static に置く（従来の `MainActivity` のフィールドと同じ。UI スレッドでのみ読み書きする）。
// 権限は ACCESS_COARSE_LOCATION が基本で、ジオフェンスが精密位置を求めたとき（`locationPreciseWanted`）だけ
// ACCESS_FINE_LOCATION を同時に要求する（最小権限、security.md A05）。BACKGROUND は要求しない。
final class LocationHelper {
    // 位置情報の更新値を Rust へ送る（latitude/longitude: 度（WGS84）、accuracy: 水平精度（メートル））。
    private static native void nativeOnLocationChanged(double latitude, double longitude, float accuracy);

    // code の対応表（Rust 側 `decode_location_failure` と対で決める）:
    // 1=権限拒否／2=位置情報サービス無効（設定 OFF・provider 無効）／3=その他の失敗。
    private static native void nativeOnLocationFailure(int code);

    // 失敗からの回復（権限再取得・provider 再有効化）に、直前の失敗状態を「未取得」へ戻す。値は運ばない。
    // 呼び出し口は `notifyLocationRecoveredIfNeeded` のみ（`locationFailurePosted` 参照）。
    private static native void nativeOnLocationRetrying();

    private static final int LOCATION_FAILURE_PERMISSION_DENIED = 1;
    private static final int LOCATION_FAILURE_SERVICE_DISABLED = 2;
    private static final int LOCATION_FAILURE_FAILED = 3;

    // 位置情報の更新間隔（ponytail: 型サンプルの表示更新用途で強い実測根拠はない。頻度が問題になったら見直す）。
    private static final long LOCATION_UPDATE_INTERVAL_MS = 1000L;

    private static final Map<Activity, LocationHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    // 「測位を続けたい」という意思を Activity 再生成を跨いで保持する。`onPause` では `removeUpdates` するだけで
    // false へは戻さない（バックグラウンドから復帰したら測位を再開する契約）。
    private static boolean locationWanted;

    // 権限ダイアログの結果待ち。ダイアログは Activity が再生成されても表示され続けるため、この間に新インスタンスの
    // resumed が走っても、利用者が応答する前に PermissionDenied を誤って通知しない。
    private static boolean awaitingLocationPermission;

    // COARSE のみ許可済みの利用者へ FINE への昇格ダイアログを出し済みか（プロセス内）。
    private static boolean fineUpgradeAsked;

    // ジオフェンスが精密位置（FINE）を要求済みか。プロセス内は下ろさない（再開時の再要求のダイアログは
    // `fineUpgradeAsked` で 1 回に抑える）。JNI スレッドが書くため volatile。
    private static volatile boolean locationPreciseWanted;

    // `requestLocationUpdates` が積んだ開始要求のうち、まだ「権限確認 → 必要ならダイアログ起動」まで実行できて
    // いないものがあるか。paused で早期 return した要求を握り潰さず、次の resumed が代わりに実行する。
    private static boolean locationStartRequestPending;

    // 直近で Rust 側へ失敗を通知済みか。`nativeOnLocationRetrying` は「失敗からの回復」専用で、平常な resumed の
    // 再登録でまで呼ぶと取得済みの Fix を破棄してしまう。失敗通知の唯一の出口 `notifyLocationFailure` が立て、
    // 回復通知の出口 `notifyLocationRecoveredIfNeeded` が「直前が失敗のときだけ」下ろす。
    private static boolean locationFailurePosted;

    private final ComponentActivity activity;
    private final LocationManager locationManager;

    // 測位の開始・停止を UI スレッド上の resumed 状態と一致させる可視性フラグ。開始 Runnable・権限結果の両方で
    // 確認し、resumed でなければ登録・通知を行わず次の resumed まで保留する（UI スレッドでのみ読み書きする）。
    private boolean activityResumed;

    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、`LocationLifecycle.onHostCreated`
    // （`Activity.onCreate` の中）から `attach` で本クラスを生成してここで登録する。キーは従来の文字列のまま。
    private final ActivityResultLauncher<String[]> requestLocationPermission;

    // OS からの継続コールバック。`hasAccuracy()` が偽のサンプルは `Fix.accuracy_m`「有限・0 以上」の契約を
    // 偽装しないため捨てる。
    private final LocationListener locationListener = new LocationListener() {
        @Override
        public void onLocationChanged(Location location) {
            if (!location.hasAccuracy()) {
                return;
            }
            // 実測位が届いた時点で「失敗通知済み」ではなくなる。
            locationFailurePosted = false;
            nativeOnLocationChanged(
                    location.getLatitude(), location.getLongitude(), location.getAccuracy());
        }

        @Override
        public void onProviderDisabled(String provider) {
            notifyLocationFailure(LOCATION_FAILURE_SERVICE_DISABLED);
        }

        @Override
        public void onProviderEnabled(String provider) {
            // listener は登録済みのままのため `startLocationUpdates` は再度呼ばれない。ここで明示的に
            // 「未取得」へ戻さないと、次の fix まで画面が Disabled のまま固定される。
            notifyLocationRecoveredIfNeeded();
        }

        @Override
        public void onStatusChanged(String provider, int status, Bundle extras) {
            // 非推奨 API（API 29 以降呼ばれない）。使わない。
        }
    };

    private LocationHelper(ComponentActivity activity) {
        this.activity = activity;
        this.locationManager = activity.getSystemService(LocationManager.class);
        // 権限ダイアログの結果。`awaitingLocationPermission` を解除した後、resumed 状態を確認してから配送する。
        // paused 中に届いたときは何もしない（granted なら次の resumed が `startLocationUpdates` を呼び、
        // denied なら次の resumed が通常どおり失敗を送る）。
        this.requestLocationPermission = activity.getActivityResultRegistry().register(
                "fandhe:MainActivity:8", activity,
                new ActivityResultContracts.RequestMultiplePermissions(), result -> {
                    awaitingLocationPermission = false;
                    if (!activityResumed) {
                        return;
                    }
                    // 取消（空 Map）は拒否扱い。FINE か COARSE のどちらかが許可されれば進む。
                    boolean granted = Boolean.TRUE.equals(result.get(Manifest.permission.ACCESS_FINE_LOCATION))
                            || Boolean.TRUE.equals(result.get(Manifest.permission.ACCESS_COARSE_LOCATION))
                            || hasLocationPermission();
                    if (granted) {
                        startLocationUpdates();
                    } else {
                        notifyLocationFailure(LOCATION_FAILURE_PERMISSION_DENIED);
                    }
                });
    }

    // Activity の生成時（`LocationLifecycle` から、UI スレッド）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new LocationHelper(activity));
    }

    // Activity の破棄時（同上）。登録済みのリスナーを外して Activity を持ち越さない。
    static void detach(ComponentActivity activity) {
        LocationHelper helper = HELPERS.remove(activity);
        if (helper != null && helper.locationManager != null) {
            helper.locationManager.removeUpdates(helper.locationListener);
        }
    }

    // 前面復帰時（`Activity.onResume()` の中、つまり `super.onResume()` の内側で届く）。
    // 測位継続の意思があり、かつ権限が既にあるなら再登録する（`paused` で解除済みのため復帰のたびに要る）。
    // 権限が無ければここでは要求しない（ダイアログは Rust 起点の開始要求経由のみで出す）。
    static void resumed(ComponentActivity activity) {
        LocationHelper helper = HELPERS.get(activity);
        if (helper == null) {
            return;
        }
        helper.activityResumed = true;
        // `awaitingLocationPermission` の間は権限確認をスキップする。ダイアログはまだ表示されているため、
        // 未許可を見て即 PermissionDenied を通知すると、利用者の応答前に拒否扱いになる。
        if (locationWanted && !awaitingLocationPermission) {
            if (locationStartRequestPending) {
                // 開始 Runnable が paused 中で早期 return し、まだ権限確認・ダイアログ起動を一度も行えていない要求。
                // 利用者は実際には拒否していないので、ここで初めてダイアログを出してよい。
                locationStartRequestPending = false;
                helper.ensureLocationPermissionAndStart();
            } else if (helper.hasLocationPermission()) {
                helper.startLocationUpdates();
            } else {
                // 設定画面等でバックグラウンド中に権限が取り消された場合。`removeUpdates` 済みで以後何も届かず、
                // Rust 側が古い Fix を保持し続けるため、PermissionDenied を明示的に通知する。
                notifyLocationFailure(LOCATION_FAILURE_PERMISSION_DENIED);
            }
        }
    }

    // 前面離脱時（`Activity.onPause()` の中、つまり `super.onPause()` の内側で届く）。
    // 「使用中のみ」の権限のため、バックグラウンドでは測位しない。
    static void paused(ComponentActivity activity) {
        LocationHelper helper = HELPERS.get(activity);
        if (helper == null) {
            return;
        }
        // `removeUpdates` より先に false へ落とし、停止済み Activity へリスナーが再登録されないようにする。
        helper.activityResumed = false;
        if (helper.locationManager != null) {
            helper.locationManager.removeUpdates(helper.locationListener);
        }
    }

    // JNI スレッドから呼ばれる。結果を待たず即座に戻る。`precise` はジオフェンスからの要求で真（FINE が要る）、
    // 位置情報画面からは偽（COARSE のみ）。helper が無い（生成前・破棄後）ときは結果が来ないまま待たせず失敗で確定する。
    static void requestLocationUpdates(Activity activity, boolean precise) {
        final LocationHelper helper = HELPERS.get(activity);
        if (helper == null) {
            notifyLocationFailureFromAnyThread();
            return;
        }
        // `locationWanted`・`locationStartRequestPending` より先に、同じ JNI スレッドで設定する
        // （UI スレッドへ遅れて立てると、先に走った resumed が COARSE だけで要求してしまう）。
        // 現在の開始要求が求める精度で毎回上書きする（最小権限）。
        locationPreciseWanted = precise;
        locationWanted = true;
        locationStartRequestPending = true;
        helper.activity.runOnUiThread(() -> {
            // この Runnable の実行前に paused が走っていれば、停止済み Activity へ再登録しない（次の resumed が拾う）。
            // 実行前に resumed が先に `locationStartRequestPending` を消費済みなら何もしない
            // （利用者が応答していないのに権限要求を再 launch して取消結果を拒否と誤通知しない）。
            if (!helper.activityResumed || !locationStartRequestPending) {
                return;
            }
            locationStartRequestPending = false;
            helper.ensureLocationPermissionAndStart();
        });
    }

    // helper が無いときの失敗通知（UI スレッド外から呼ばれるため static 状態は触らず、通知だけ行う）。
    private static void notifyLocationFailureFromAnyThread() {
        nativeOnLocationFailure(LOCATION_FAILURE_FAILED);
    }

    // `nativeOnLocationFailure` の唯一の呼び出し口（`locationFailurePosted` 参照）。
    private static void notifyLocationFailure(int code) {
        locationFailurePosted = true;
        nativeOnLocationFailure(code);
    }

    // 「失敗からの回復」を確定できるときだけ `nativeOnLocationRetrying` を呼ぶ。直前に失敗を通知していなければ
    // 何もしない。`startLocationUpdates` と `onProviderEnabled` から呼ばれる。
    private static void notifyLocationRecoveredIfNeeded() {
        if (locationFailurePosted) {
            locationFailurePosted = false;
            nativeOnLocationRetrying();
        }
    }

    // 権限確認 → 未許可ならダイアログ起動、を行う。UI スレッド・resumed 状態を呼び出し元が保証すること。
    // 「おおよそ」だけ許可済みの利用者にも、プロセスにつき 1 回 FINE への昇格ダイアログを出す
    // （ジオフェンスの半径 100 m に FINE が要る）。拒否されても COARSE が許可済みなら結果コールバックが測位を始める。
    private void ensureLocationPermissionAndStart() {
        if (hasFineLocationPermission()
                || (hasLocationPermission() && (!locationPreciseWanted || fineUpgradeAsked))) {
            startLocationUpdates();
        } else {
            boolean askFine = locationPreciseWanted;
            awaitingLocationPermission = true;
            try {
                requestLocationPermission.launch(askFine
                        ? new String[] {
                            Manifest.permission.ACCESS_FINE_LOCATION,
                            Manifest.permission.ACCESS_COARSE_LOCATION
                        }
                        : new String[] {Manifest.permission.ACCESS_COARSE_LOCATION});
                // FINE を含むダイアログを実際に出せたら「精密位置を聞いた」とする（launch 失敗では立てない）。
                if (askFine) {
                    fineUpgradeAsked = true;
                }
            } catch (RuntimeException e) {
                // `launch()` 自体が例外を送出した場合（`onSaveInstanceState` 後の呼び出し等）。
                awaitingLocationPermission = false;
                notifyLocationFailure(LOCATION_FAILURE_FAILED);
            }
        }
    }

    private boolean hasFineLocationPermission() {
        return ContextCompat.checkSelfPermission(activity, Manifest.permission.ACCESS_FINE_LOCATION)
                == PackageManager.PERMISSION_GRANTED;
    }

    // FINE か COARSE のどちらかが許可済みか（「おおよそ」のみ許可でも true）。
    private boolean hasLocationPermission() {
        return ContextCompat.checkSelfPermission(activity, Manifest.permission.ACCESS_FINE_LOCATION)
                        == PackageManager.PERMISSION_GRANTED
                || ContextCompat.checkSelfPermission(activity, Manifest.permission.ACCESS_COARSE_LOCATION)
                        == PackageManager.PERMISSION_GRANTED;
    }

    // 権限確認後（開始要求・許可コールバック・resumed の再登録）に呼ばれる。UI スレッド前提。
    private void startLocationUpdates() {
        if (locationManager == null) {
            notifyLocationFailure(LOCATION_FAILURE_FAILED);
            return;
        }
        String provider = currentLocationProvider();
        boolean serviceEnabled = LocationManagerCompat.isLocationEnabled(locationManager)
                && locationManager.isProviderEnabled(provider);
        if (serviceEnabled) {
            // 直前に失敗を通知済みのときだけ「未取得」へ戻す（平常な再登録では取得済みの Fix を破棄しない）。
            notifyLocationRecoveredIfNeeded();
        } else {
            // サービスが無効なままの再登録では回復通知を出さない（Disabled → 取得中 → Disabled の明滅を防ぐ）。
            // 無効を通知したうえで登録自体は行う。有効化後に OS が自然に更新を届けるため回復できる。
            notifyLocationFailure(LOCATION_FAILURE_SERVICE_DISABLED);
        }
        try {
            locationManager.requestLocationUpdates(
                    provider,
                    LOCATION_UPDATE_INTERVAL_MS,
                    0f,
                    locationListener,
                    Looper.getMainLooper());
        } catch (SecurityException | IllegalArgumentException e) {
            notifyLocationFailure(LOCATION_FAILURE_FAILED);
        }
    }

    // provider 選択: API 31（S）以降は `FUSED_PROVIDER`、それ未満（minSdk 24）は `NETWORK_PROVIDER`。GPS は使わない。
    private static String currentLocationProvider() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            return LocationManager.FUSED_PROVIDER;
        }
        return LocationManager.NETWORK_PROVIDER;
    }
}
