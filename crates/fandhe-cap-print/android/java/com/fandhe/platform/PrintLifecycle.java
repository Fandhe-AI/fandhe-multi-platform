package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// print が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1334）。
// 宣言は `fandhe-cap-print` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `PrintHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
public final class PrintLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        PrintHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        PrintHelper.detach(activity);
    }
}
