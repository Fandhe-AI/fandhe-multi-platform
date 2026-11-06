package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// camera-capture が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1257）。
// 宣言は `fandhe-cap-camera-capture` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `CameraCaptureHelper` の `attach` / `save` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
public final class CameraCaptureLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        CameraCaptureHelper.attach(activity, savedInstanceState);
    }

    @Override
    protected void onHostSaveInstanceState(ComponentActivity activity, Bundle outState) {
        CameraCaptureHelper.save(activity, outState);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        CameraCaptureHelper.detach(activity);
    }
}
