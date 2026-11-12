package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// passkey-signin が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1257）。
// 宣言は `fandhe-cap-passkey-signin` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `PasskeyHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
public final class PasskeyLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        PasskeyHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        PasskeyHelper.detach(activity);
    }
}
