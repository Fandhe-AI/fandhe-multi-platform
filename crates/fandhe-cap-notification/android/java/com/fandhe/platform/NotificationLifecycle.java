package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// notification が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1331）。
// 宣言は `fandhe-cap-notification` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `NotificationHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 許可ダイアログのランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。
//
// Resumed / Paused は `Activity.onResume()` / `onPause()` の中、つまり `super.onXxx()` の中で届く。従来の `MainActivity` は
// `super` の後で `activityResumed` を更新していたため、`super` に対する前後だけが違う。前面かどうかの判定は UI スレッドで
// 要求を処理する時点の値を読むだけで、前後の違いで結果は変わらない。
public final class NotificationLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        NotificationHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        NotificationHelper.resumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        NotificationHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        NotificationHelper.detach(activity);
    }
}
