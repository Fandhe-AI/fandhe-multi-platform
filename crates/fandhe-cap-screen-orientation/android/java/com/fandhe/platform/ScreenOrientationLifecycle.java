package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// screen-orientation が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1334）。
// 宣言は `fandhe-cap-screen-orientation` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `ScreenOrientationHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
public final class ScreenOrientationLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        ScreenOrientationHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        ScreenOrientationHelper.detach(activity);
    }
}
