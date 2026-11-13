package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// compose-mail-sms が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1193）。
// 宣言は `fandhe-cap-compose-mail-sms` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `ComposeHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
public final class ComposeLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        ComposeHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        ComposeHelper.detach(activity);
    }
}
