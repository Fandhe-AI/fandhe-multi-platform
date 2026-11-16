package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// now-playing-controls が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1331）。
// 宣言は `fandhe-cap-now-playing-controls` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `NowPlayingHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
//
// Resumed / Paused は `Activity.onResume()` / `onPause()` の中、つまり `super.onXxx()` の中で届く。従来の `MainActivity` は
// `super` の後で helper を呼んでいたため、`super` に対する前後だけが違う。helper の状態はすべて `lock` の内側で決まり、
// 前後の違いで結果は変わらない。
public final class NowPlayingLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        NowPlayingHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        NowPlayingHelper.resumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        NowPlayingHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        NowPlayingHelper.detach(activity);
    }
}
