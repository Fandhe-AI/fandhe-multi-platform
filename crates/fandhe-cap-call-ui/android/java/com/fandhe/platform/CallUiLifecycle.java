package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// call-ui が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1257、#1187）。
// 宣言は `fandhe-cap-call-ui` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `CallUiHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 状態の保存はしない（再生成後の旧要求は Rust 側 `reset_pending_tracking` が捨てる）。
public final class CallUiLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        CallUiHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        CallUiHelper.detach(activity);
    }
}
