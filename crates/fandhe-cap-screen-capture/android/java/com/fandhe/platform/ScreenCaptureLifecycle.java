package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// screen-capture が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1257）。
// 宣言は `fandhe-cap-screen-capture` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `ScreenCaptureHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
public final class ScreenCaptureLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        ScreenCaptureHelper.attach(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        ScreenCaptureHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        ScreenCaptureHelper.detach(activity);
    }
}
