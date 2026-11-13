package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// external-app-launch が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1192）。
// 宣言は `fandhe-cap-external-app-launch` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `ExternalAppLaunchHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 保留中の要求は Rust 側が id で照合するため、状態の保存はしない。
public final class ExternalAppLaunchLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        ExternalAppLaunchHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        ExternalAppLaunchHelper.detach(activity);
    }
}
