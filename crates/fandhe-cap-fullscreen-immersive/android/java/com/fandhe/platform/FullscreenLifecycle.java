package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// fullscreen-immersive が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1334）。
// 宣言は `fandhe-cap-fullscreen-immersive` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `FullscreenHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
//
// insets の配り先は decorView を触る `HostInsets.addListener` で登録するため、`onHostCreated` ではなく `onHostStarted` で呼ぶ
// （`getDecorView()` を早く呼ぶと後続の `requestWindowFeature` が使えなくなるおそれがある）。
public final class FullscreenLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        FullscreenHelper.attach(activity);
    }

    @Override
    protected void onHostStarted(ComponentActivity activity) {
        FullscreenHelper.started(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        FullscreenHelper.detach(activity);
    }
}
