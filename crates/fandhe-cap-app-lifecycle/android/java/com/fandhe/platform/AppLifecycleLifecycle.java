package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// app-lifecycle が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1192）。
// 宣言は `fandhe-cap-app-lifecycle` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `AppLifecycleHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
//
// 前面・背面の 4 つの通知（Started / Stopped / Resumed / Paused）はいずれも `Activity.onStart()` などの中、つまり
// `super.onXxx()` の中で届く。従来の `MainActivity` は `onStart` / `onResume` / `onPause` では `super` の後で、`onStop` では
// `super` の前に helper を呼んでいたため、`super` に対する前後だけが違う。`isChangingConfigurations()` の判定は同じ
// コールバックの内側で行うので変わらない。購読は Rust が開始ごとに張り直すため、状態の保存はしない。
public final class AppLifecycleLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        AppLifecycleHelper.attach(activity);
    }

    @Override
    protected void onHostStarted(ComponentActivity activity) {
        AppLifecycleHelper.started(activity);
    }

    @Override
    protected void onHostStopped(ComponentActivity activity) {
        AppLifecycleHelper.stopped(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        AppLifecycleHelper.resumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        AppLifecycleHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        AppLifecycleHelper.detach(activity);
    }
}
