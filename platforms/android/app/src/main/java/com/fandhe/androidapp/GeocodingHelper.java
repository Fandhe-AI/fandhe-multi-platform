package com.fandhe.androidapp;

import android.content.Context;
import android.location.Address;
import android.location.Geocoder;
import android.os.Build;

import androidx.annotation.RequiresApi;

import java.lang.ref.WeakReference;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.RejectedExecutionException;

// 住所 → 座標の変換（Issue #693、親 #133）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_geocoding_search.rs` が `MainActivity.requestGeocode` /
// `cancelGeocode` を JNI で呼び、結果を `nativeOnGeocodeResult` で受ける）。
//
// 1 回の要求 = `android.location.Geocoder` で住所 1 件を変換し、先頭候補の座標と候補数を返す。
// API 33 以上は `GeocodeListener` 版、24〜32 は同期版を単一スレッドの executor で呼ぶ。
// `Geocoder.isPresent()` が false の端末（geocode provider なし）は UNSUPPORTED で、ダミー成功へ
// 倒さない。位置情報の権限は不要（住所 → 座標は現在地を使わない）。
//
// 失敗の写像: IOException・onError・その他の例外は区別せず FAILED（通信不可と回数上限は区別できない）。
// 0 件・座標なしの判定と範囲検証は Rust 側 `check_result` に任せ、ここでは丸めない。API 33 で
// 失敗しても onError が届かない場合は、Rust 側の期限（15 秒）で TIMEOUT になり成功にはならない。
//
// 取り消し: Geocoder に取り消し API は無い。`cancel` は追跡を外すだけで、OS 側の処理は走り切る
// （結果は捨てる）。同期版が詰まると単一 executor の後続が待たされるが、Rust の期限で失敗が確定する。
//
// Activity 非保持: helper は Activity を持たない。application context は呼び出しごとに MainActivity から
// 引数で受け、フィールドにも持たない。OS 側の未完了処理（同期版の長い停止・GeocodeListener）から
// Activity へ到達できないため、再生成後も旧 Activity とその画面資源が回収を妨げられない。
// リスナーは static 入れ子クラスで、helper も弱参照でしか持たない。
//
// ライフサイクル: `onDestroy`（Activity 再生成を含む）で保留中の要求を INTERRUPTED で確定する。
// `request` はインスタンスフィールドなので、再生成後の新インスタンスは未追跡から始まり旧結果を拾わない。
// 住所・座標・OS の例外メッセージはログに出さない。
final class GeocodingHelper {
    // 結果コード。SSOT はここで、Rust 側 `android_geocoding_search.rs::decode` が同じ値で写像する。
    static final int RESULT_OK = 0;
    static final int RESULT_UNSUPPORTED = 1;
    static final int RESULT_INTERRUPTED = 2;
    static final int RESULT_FAILED = 3;

    private static native void nativeOnGeocodeResult(
            long requestId, int code, double latitude, double longitude, boolean hasCoordinate, int count);

    private final RequestTracker request = new RequestTracker();
    private final ExecutorService executor = Executors.newSingleThreadExecutor();
    private final Object lock = new Object();
    // 取り消し済みの最大 id（lock の内側）。cancel が start より先に走っても古い要求を開始しない。
    private long cancelledId = RequestTracker.NONE;
    private volatile boolean destroyed;

    // JNI スレッドから呼ばれる。appContext は MainActivity が `getApplicationContext()` で渡す。
    void request(Context appContext, long id, String address, int maxResults) {
        try {
            executor.execute(() -> start(appContext, id, address, maxResults));
        } catch (RejectedExecutionException e) {
            nativeOnGeocodeResult(id, RESULT_INTERRUPTED, 0, 0, false, 0);
        }
    }

    // Rust 側の期限切れから呼ばれる。追跡を外すだけで、OS 側の処理は止められない。
    void cancel(long id) {
        synchronized (lock) {
            cancelledId = Math.max(cancelledId, id);
            request.complete(id);
        }
    }

    // MainActivity.onDestroy から UI スレッドで呼ぶ。
    void onDestroy() {
        destroyed = true;
        long id;
        synchronized (lock) {
            id = request.take();
        }
        if (id != RequestTracker.NONE) {
            nativeOnGeocodeResult(id, RESULT_INTERRUPTED, 0, 0, false, 0);
        }
        executor.shutdownNow();
    }

    // executor 上で実行する。
    private void start(Context appContext, long id, String address, int maxResults) {
        synchronized (lock) {
            if (id <= cancelledId) {
                return;
            }
            if (!destroyed) {
                request.track(id);
            }
        }
        if (destroyed) {
            nativeOnGeocodeResult(id, RESULT_INTERRUPTED, 0, 0, false, 0);
            return;
        }
        try {
            if (!Geocoder.isPresent()) {
                finish(id, RESULT_UNSUPPORTED, null);
                return;
            }
            Geocoder geocoder = new Geocoder(appContext);
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                startOnT(geocoder, id, address, maxResults);
            } else {
                startLegacy(geocoder, id, address, maxResults);
            }
        } catch (RuntimeException e) {
            finish(id, RESULT_FAILED, null);
        }
    }

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private void startOnT(Geocoder geocoder, long id, String address, int maxResults) {
        geocoder.getFromLocationName(address, maxResults, new Listener(this, id));
    }

    @SuppressWarnings("deprecation")
    private void startLegacy(Geocoder geocoder, long id, String address, int maxResults) {
        List<Address> list;
        try {
            list = geocoder.getFromLocationName(address, maxResults);
        } catch (java.io.IOException | RuntimeException e) {
            finish(id, RESULT_FAILED, null);
            return;
        }
        finish(id, RESULT_OK, list);
    }

    // API 33 以上の結果受け口。helper を弱参照でしか持たず、Activity への参照は無い。
    // 失敗理由は区別しない。onError が届かない場合は Rust 側の期限が TIMEOUT にする。
    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private static final class Listener implements Geocoder.GeocodeListener {
        private final WeakReference<GeocodingHelper> helper;
        private final long id;

        Listener(GeocodingHelper helper, long id) {
            this.helper = new WeakReference<>(helper);
            this.id = id;
        }

        @Override
        public void onGeocode(List<Address> addresses) {
            GeocodingHelper h = helper.get();
            if (h != null) {
                h.finish(id, RESULT_OK, addresses);
            }
        }

        @Override
        public void onError(String errorMessage) {
            GeocodingHelper h = helper.get();
            if (h != null) {
                h.finish(id, RESULT_FAILED, null);
            }
        }
    }

    // この id が保留中の要求なら 1 回だけ確定する。判定と追跡の解除は lock の内側、native 通知は外で行う
    // （first-wins）。code が OK 以外のとき list は使わない。
    private void finish(long id, int code, List<Address> list) {
        synchronized (lock) {
            if (!request.complete(id)) {
                return; // 取り消し・差し替え・破棄済み。
            }
        }
        if (code != RESULT_OK) {
            nativeOnGeocodeResult(id, code, 0, 0, false, 0);
            return;
        }
        if (list == null || list.isEmpty()) {
            nativeOnGeocodeResult(id, RESULT_OK, 0, 0, false, 0); // 件数 0 は Rust 側で NOT_FOUND。
            return;
        }
        Address first = list.get(0);
        boolean has = first != null && first.hasLatitude() && first.hasLongitude();
        nativeOnGeocodeResult(id, RESULT_OK,
                has ? first.getLatitude() : 0, has ? first.getLongitude() : 0, has, list.size());
    }
}
