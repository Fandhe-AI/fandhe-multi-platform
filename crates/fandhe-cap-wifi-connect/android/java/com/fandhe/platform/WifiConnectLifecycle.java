package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// wifi-connect が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1191）。
// 宣言は `fandhe-cap-wifi-connect` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `WifiConnectHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 保留中の要求は Activity ごとの helper が持ち、再生成では持ち越さない（破棄時に FAILED で確定する）。
public final class WifiConnectLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        WifiConnectHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        WifiConnectHelper.detach(activity);
    }
}
