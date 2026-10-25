package com.fandhe.androidapp;

import android.annotation.SuppressLint;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.net.Uri;
import android.provider.Settings;

import androidx.activity.ComponentActivity;
import androidx.lifecycle.Lifecycle;

// OS の設定画面を開く機能（Issue #800、親 #171）の Android 実装ヘルパー。MainActivity から
// 生成・委譲される（Rust の `device/android_open_system_settings.rs` が
// `MainActivity.requestOpenSystemSettings` を JNI で呼び、結果を `nativeOnOpenSettingsResult` で受けて
// `open_system_settings::outcome_from_android_code` で契約の結果へ写像する）。
//
// 開く頁は固定の 2 種のみ（自アプリの詳細設定・通知設定）。外部から Intent・URI・action は受けない。
// `resolveActivity` は呼ばず、`<queries>` と権限は宣言しない（startActivity は package visibility を要さず、
// ActivityNotFoundException を扱う）。開けなかった頁を別の頁で代用しない。
//
// 結果は「OS が Intent を受け付けた」(HANDED_OFF) か「受け手がいない」(NOT_FOUND) まで。
// 利用者が設定を変えたかは分からない。非前景・破棄中は OS に黙って抑止されうるため起動せず BACKGROUND を返す。
// ライフサイクル: `request` はインスタンスフィールド。再生成後の新インスタンスは NONE から始まり、
// 旧インスタンスの結果は Rust 側 `reset_pending_tracking()` と id 照合で捨てられる。
// 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止。守らない範囲: 設定画面へ渡した後の利用者の操作。
// id・結果コード・例外メッセージはログに出さない。
final class SystemSettingsHelper {
    // 頁コード。Rust 側 `android_open_system_settings.rs` の PAGE_* と対。
    static final int PAGE_APP = 0;
    static final int PAGE_NOTIFICATIONS = 1;

    // 結果コードの SSOT。Rust 側 `outcome_from_android_code` が同じ値で写像する。
    static final int RESULT_HANDED_OFF = 0;
    static final int RESULT_NOT_FOUND = 1;
    static final int RESULT_BACKGROUND = 2;
    static final int RESULT_PRESENT_FAILED = 3;

    private static native void nativeOnOpenSettingsResult(long requestId, int code);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();

    SystemSettingsHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // JNI スレッドから呼ばれる。
    void open(long id, int page) {
        request.track(id);
        activity.runOnUiThread(() -> start(id, page));
    }

    // `ACTION_APP_NOTIFICATION_SETTINGS` / `EXTRA_APP_PACKAGE` は API 26 の定数だが `static final String` で
    // コンパイル時に文字列へ展開されるため、minSdk 24 でも NoSuchFieldError にはならない。
    // API 24・25 では受け手の Activity が無く ActivityNotFoundException になり NOT_FOUND を返す（意図した挙動）。
    @SuppressLint("InlinedApi")
    private void start(long id, int page) {
        // 差し替え済みの旧要求は結果を返さず捨てる。
        if (!request.complete(id)) {
            return;
        }
        if (activity.isFinishing() || activity.isDestroyed()
                || !activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            nativeOnOpenSettingsResult(id, RESULT_BACKGROUND);
            return;
        }
        Intent intent;
        if (page == PAGE_APP) {
            intent = new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                    Uri.fromParts("package", activity.getPackageName(), null));
        } else if (page == PAGE_NOTIFICATIONS) {
            intent = new Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                    .putExtra(Settings.EXTRA_APP_PACKAGE, activity.getPackageName());
        } else {
            nativeOnOpenSettingsResult(id, RESULT_PRESENT_FAILED);
            return;
        }
        int code;
        try {
            activity.startActivity(intent);
            code = RESULT_HANDED_OFF;
        } catch (ActivityNotFoundException e) {
            code = RESULT_NOT_FOUND;
        } catch (RuntimeException e) {
            code = RESULT_PRESENT_FAILED;
        }
        nativeOnOpenSettingsResult(id, code);
    }
}
