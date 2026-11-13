package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// system-sound が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1192）。
// 宣言は `fandhe-cap-system-sound` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `SystemSoundHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 保留中の要求を持たない同期呼び出しのため、状態の保存はしない。
public final class SystemSoundLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        SystemSoundHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        SystemSoundHelper.detach(activity);
    }
}
