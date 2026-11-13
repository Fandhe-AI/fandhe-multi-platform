package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// scanner-devices が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1193）。
// 宣言は `fandhe-cap-scanner-devices` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `ScannerDevicesHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 保留中の要求は Activity ごとの helper が持ち、再生成では持ち越さない（破棄時に INTERRUPTED で確定する）。
public final class ScannerDevicesLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        ScannerDevicesHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        ScannerDevicesHelper.detach(activity);
    }
}
