package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// in-app-browser が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1329）。
// 宣言は `fandhe-cap-in-app-browser` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `InAppBrowserHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
//
// Resumed / Paused は `Activity.onResume()` / `onPause()` の中、つまり `super.onXxx()` の中で届く。従来の `MainActivity` は
// どちらも `super` の後で helper を呼んでいたため、`super` に対する前後だけが違う。「起動後の離脱（Paused）と復帰（Resumed）を
// 閉じたとみなす」判定は 2 つの通知の順序だけに依存し、この違いで変わらない。起動直前の RESUMED・`isFinishing` の見直しは
// `InAppBrowserHelper.launch` が引き続き行う。
public final class InAppBrowserLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        InAppBrowserHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        InAppBrowserHelper.resumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        InAppBrowserHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        InAppBrowserHelper.detach(activity);
    }
}
