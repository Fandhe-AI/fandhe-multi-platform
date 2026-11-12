package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// app-badge が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1257）。
// 宣言は `fandhe-cap-app-badge` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `AppBadgeHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
public final class AppBadgeLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        AppBadgeHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        AppBadgeHelper.detach(activity);
    }
}
