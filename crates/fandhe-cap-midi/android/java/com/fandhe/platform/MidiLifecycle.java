package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// midi が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1257）。
// 宣言は `fandhe-cap-midi` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `MidiHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
public final class MidiLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        MidiHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        MidiHelper.detach(activity);
    }
}
