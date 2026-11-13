package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// video-capture が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1192）。
// 宣言は `fandhe-cap-video-capture` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `VideoCaptureHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// `registerForActivityResult` は STARTED より前に呼ぶ契約のため、`onHostCreated`（onCreate 内）で helper を生成する。
public final class VideoCaptureLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        VideoCaptureHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        VideoCaptureHelper.detach(activity);
    }
}
