package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// geocoding-search が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1190）。
// 宣言は `fandhe-cap-geocoding-search` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `GeocodingHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 状態の保存はしない（再生成後は Rust 側の画面も未要求から作り直されるため、要求は持ち越さない）。
public final class GeocodingLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        GeocodingHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        GeocodingHelper.detach(activity);
    }
}
