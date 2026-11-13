package com.fandhe.platform;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.net.Uri;

import androidx.activity.ComponentActivity;
import androidx.lifecycle.Lifecycle;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// 電話発信・地図など他アプリの起動機能（Issue #863、親 #176）の Android 実装ヘルパー。利用者の Activity には何も書かせない
// （Rust の `fandhe-cap-external-app-launch/src/android.rs` が static の `launch(Activity, id, target, uri)` を JNI で呼び、
// 結果を `nativeOnExternalAppLaunchResult` で受けて `driver::outcome_from_android_code` で契約の結果へ写像する）。
// Activity の生成・破棄は `ExternalAppLaunchLifecycle`（非公開 ContentProvider、`FeatureLifecycleProvider` 継承）が
// `attach` / `detach` で知らせる。Activity ごとの helper は `HELPERS` に持つ（Rust のワーカーと UI スレッドの両方から触るので
// 同期化し、Activity が GC されれば項目も消える）。表に無い Activity への要求は false を返し、Rust が失敗として確定する。
//
// 開く先は固定の 2 種のみ。電話は `ACTION_DIAL`（ダイヤラーに番号を出すだけで発信しない）、
// 地図は `ACTION_VIEW`。action は target コードで Java が決め、URI は Rust の固定定数を受け取る。
// 外部から Intent・action・extra は受けない。`ACTION_CALL`・`CALL_PHONE` は使わない。
// `resolveActivity` は呼ばず、`<queries>` と権限は宣言しない（startActivity は package visibility を要さず、
// ActivityNotFoundException を扱う）。開けなかった操作を別の URI で代用しない。
//
// 結果は「OS が Intent を受け付けた」(HANDED_OFF) か「受け手がいない」(NOT_FOUND) まで。
// 非前景・破棄中は OS に黙って抑止されうるため起動せず BACKGROUND を返す。
// ライフサイクル: `request` はインスタンスフィールド。再生成後の新インスタンスは NONE から始まり、
// 旧インスタンスの結果は Rust 側 `reset_pending_tracking()` と id 照合で捨てられる。
// 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止。守らない範囲: 他アプリへ渡した後の利用者の操作。
// id・結果コード・例外メッセージはログに出さない。
final class ExternalAppLaunchHelper {
    // target コード。Rust 側 `android_external_app_launch.rs` の TARGET_* と対。
    static final int TARGET_PHONE = 0;
    static final int TARGET_MAP = 1;

    // 結果コードの SSOT。Rust 側 `outcome_from_android_code` が同じ値で写像する。
    static final int RESULT_HANDED_OFF = 0;
    static final int RESULT_NOT_FOUND = 1;
    static final int RESULT_BACKGROUND = 2;
    static final int RESULT_PRESENT_FAILED = 3;

    private static native void nativeOnExternalAppLaunchResult(long requestId, int code);

    // Activity ごとの helper（上記クラス冒頭コメント参照）。
    private static final Map<Activity, ExternalAppLaunchHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();

    private ExternalAppLaunchHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // Activity の生成時（`ExternalAppLaunchLifecycle` から）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new ExternalAppLaunchHelper(activity));
    }

    // Activity の破棄時（同上）。保留中の要求は Rust 側 `reset_pending_tracking()` と id 照合で捨てられる。
    static void detach(Activity activity) {
        HELPERS.remove(activity);
    }

    // JNI スレッドから呼ばれる入口。helper が無い（生成前・破棄後）なら false を返し、Rust が失敗として確定する
    // （結果の native は呼ばれない）。
    static boolean launch(Activity activity, long id, int target, String uri) {
        ExternalAppLaunchHelper helper = HELPERS.get(activity);
        if (helper == null) {
            return false;
        }
        helper.begin(id, target, uri);
        return true;
    }

    private void begin(long id, int target, String uri) {
        request.track(id);
        activity.runOnUiThread(() -> start(id, target, uri));
    }

    private void start(long id, int target, String uri) {
        // 差し替え済みの旧要求は結果を返さず捨てる。
        if (!request.complete(id)) {
            return;
        }
        if (activity.isFinishing() || activity.isDestroyed()
                || !activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            nativeOnExternalAppLaunchResult(id, RESULT_BACKGROUND);
            return;
        }
        String action;
        if (target == TARGET_PHONE) {
            action = Intent.ACTION_DIAL;
        } else if (target == TARGET_MAP) {
            action = Intent.ACTION_VIEW;
        } else {
            nativeOnExternalAppLaunchResult(id, RESULT_PRESENT_FAILED);
            return;
        }
        int code;
        try {
            activity.startActivity(new Intent(action, Uri.parse(uri)));
            code = RESULT_HANDED_OFF;
        } catch (ActivityNotFoundException e) {
            code = RESULT_NOT_FOUND;
        } catch (RuntimeException e) {
            code = RESULT_PRESENT_FAILED;
        }
        nativeOnExternalAppLaunchResult(id, code);
    }
}
