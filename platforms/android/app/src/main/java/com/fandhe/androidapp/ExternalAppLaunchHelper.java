package com.fandhe.androidapp;

import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.net.Uri;

import androidx.activity.ComponentActivity;
import androidx.lifecycle.Lifecycle;

// 電話発信・地図など他アプリの起動機能（Issue #863、親 #176）の Android 実装ヘルパー。MainActivity から
// 生成・委譲される（Rust の `device/android_external_app_launch.rs` が
// `MainActivity.requestExternalAppLaunch` を JNI で呼び、結果を `nativeOnExternalAppLaunchResult` で受けて
// `external_app_launch::outcome_from_android_code` で契約の結果へ写像する）。
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

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();

    ExternalAppLaunchHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // JNI スレッドから呼ばれる。
    void launch(long id, int target, String uri) {
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
