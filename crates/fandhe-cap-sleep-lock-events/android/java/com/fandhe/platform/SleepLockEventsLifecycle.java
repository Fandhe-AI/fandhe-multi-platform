package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// sleep-lock-events が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1191）。
// 宣言は `fandhe-cap-sleep-lock-events` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `SleepLockEventsHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 購読は Activity ごとの helper が持ち、再生成では持ち越さない（新しい helper は購読を持たない状態から始まる）。
public final class SleepLockEventsLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        SleepLockEventsHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        SleepLockEventsHelper.detach(activity);
    }
}
