package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// tilt-motion が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1331）。
// 宣言は `fandhe-cap-tilt-motion` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `TiltMotionHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
//
// Resumed / Paused は `Activity.onResume()` / `onPause()` の中、つまり `super.onXxx()` の中で届く。従来の `MainActivity` は
// `super` の後でリスナーを登録・解除していたため、`super` に対する前後だけが違う。登録と解除は UI スレッドで冪等に決まり、
// 前後の違いで結果は変わらない。
public final class TiltMotionLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        TiltMotionHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        TiltMotionHelper.resumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        TiltMotionHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        TiltMotionHelper.detach(activity);
    }
}
