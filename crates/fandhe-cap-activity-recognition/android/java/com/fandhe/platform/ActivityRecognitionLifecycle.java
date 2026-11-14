package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// activity-recognition が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1329）。
// 宣言は `fandhe-cap-activity-recognition` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `ActivityRecognitionHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 許可ダイアログのランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。
//
// Started / Stopped は `Activity.onStart()` / `onStop()` の中、つまり `super.onXxx()` の中で届く。従来の `MainActivity` は
// `onStart` では `super` の後で、`onStop` では `super` の前に helper を呼んでいたため、`super` に対する前後だけが違う。
// 購読の張り直しと解除（`reconcile`）は main thread で冪等に決まり、前後の違いで結果は変わらない。
public final class ActivityRecognitionLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        ActivityRecognitionHelper.attach(activity);
    }

    @Override
    protected void onHostStarted(ComponentActivity activity) {
        ActivityRecognitionHelper.started(activity);
    }

    @Override
    protected void onHostStopped(ComponentActivity activity) {
        ActivityRecognitionHelper.stopped(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        ActivityRecognitionHelper.detach(activity);
    }
}
