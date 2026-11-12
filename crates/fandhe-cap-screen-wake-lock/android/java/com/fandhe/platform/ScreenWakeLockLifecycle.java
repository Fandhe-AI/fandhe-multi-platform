package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// screen-wake-lock が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1257、#1187）。
// 宣言は `fandhe-cap-screen-wake-lock` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `ScreenWakeLockHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 状態の保存はしない（再生成後は Rust 側の画面も未要求から作り直されるため、要求は持ち越さない）。
public final class ScreenWakeLockLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        ScreenWakeLockHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        ScreenWakeLockHelper.detach(activity);
    }
}
