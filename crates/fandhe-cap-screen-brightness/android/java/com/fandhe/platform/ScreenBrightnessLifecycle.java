package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// screen-brightness が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1190）。
// 宣言は `fandhe-cap-screen-brightness` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `ScreenBrightnessHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 要求値は Helper の static が再生成をまたいで持ち越すため、状態の保存はしない。
public final class ScreenBrightnessLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        ScreenBrightnessHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        ScreenBrightnessHelper.detach(activity);
    }
}
