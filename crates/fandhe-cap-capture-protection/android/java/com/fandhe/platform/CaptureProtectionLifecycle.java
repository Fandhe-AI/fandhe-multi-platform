package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// capture-protection が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1329）。
// 宣言は `fandhe-cap-capture-protection` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `CaptureProtectionHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
//
// 生成は `Activity.onCreate()` の中（`super.onCreate()` の内側）で届く。従来の `MainActivity` は `super.onCreate` の直後に
// 要求値を掛け直していたため、`super` に対する前後だけが違い、Window への `addFlags` は最初のフレーム前に済む。
// Started / Stopped も `super.onStart()` / `onStop()` の中で届く（従来は `onStart` は `super` の後、`onStop` は `super` の前）。
// 検知コールバックの登録・解除の対応は変わらない。
public final class CaptureProtectionLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        CaptureProtectionHelper.attach(activity);
    }

    @Override
    protected void onHostStarted(ComponentActivity activity) {
        CaptureProtectionHelper.started(activity);
    }

    @Override
    protected void onHostStopped(ComponentActivity activity) {
        CaptureProtectionHelper.stopped(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        CaptureProtectionHelper.detach(activity);
    }
}
