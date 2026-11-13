package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// memory-pressure が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1192）。
// 宣言は機能 crate の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `MemoryPressureHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 購読は Rust が開始ごとに張り直すため、状態の保存はしない。
public final class MemoryPressureLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        MemoryPressureHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        MemoryPressureHelper.detach(activity);
    }
}
