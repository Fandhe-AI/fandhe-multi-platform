package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// removable-storage が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1192）。
// 宣言は `fandhe-cap-removable-storage` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `RemovableStorageHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 購読は Rust が開始ごとに張り直すため、状態の保存はしない。
public final class RemovableStorageLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        RemovableStorageHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        RemovableStorageHelper.detach(activity);
    }
}
