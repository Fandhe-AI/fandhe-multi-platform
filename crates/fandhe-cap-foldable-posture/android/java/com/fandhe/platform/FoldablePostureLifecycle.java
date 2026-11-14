package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// foldable-posture が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1329）。
// 宣言は `fandhe-cap-foldable-posture` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `FoldablePostureHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
//
// Started / Stopped は `Activity.onStart()` / `onStop()` の中、つまり `super.onXxx()` の中で届く。従来の `MainActivity` は
// `onStart` では `super` の後で、`onStop` では `super` の前に helper を呼んでいたため、`super` に対する前後だけが違う。
// 購読の登録と解除は 1 対 1 のままで、`onStop` 後に前回値を返さない点も変わらない。
public final class FoldablePostureLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        FoldablePostureHelper.attach(activity);
    }

    @Override
    protected void onHostStarted(ComponentActivity activity) {
        FoldablePostureHelper.started(activity);
    }

    @Override
    protected void onHostStopped(ComponentActivity activity) {
        FoldablePostureHelper.stopped(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        FoldablePostureHelper.detach(activity);
    }
}
