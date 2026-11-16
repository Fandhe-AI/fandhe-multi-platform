package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// lan-discovery が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1332）。
// 宣言は `fandhe-cap-lan-discovery` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `LanDiscoveryHelper` の `attach` / `pause` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// `onHostPaused` は `Activity.onPause()` の中、つまり `super.onPause()` の内側で届く。従来の `MainActivity` は
// `super.onPause()` の後で `lanDiscoveryHelper.onPause()` を呼んでいたため、`super` に対する前後だけが違う
// （helper の中断は main looper 上で冪等に決まり、順序に依存しない）。
public final class LanDiscoveryLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        LanDiscoveryHelper.attach(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        LanDiscoveryHelper.pause(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        LanDiscoveryHelper.detach(activity);
    }
}
