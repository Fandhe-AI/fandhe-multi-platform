package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// camera-preview-stream が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1329）。
// 宣言は `fandhe-cap-camera-preview-stream` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `CameraPreviewHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 許可ダイアログのランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。
//
// Resumed / Paused は `Activity.onResume()` / `onPause()` の中、つまり `super.onXxx()` の中で届く。従来の `MainActivity` は
// どちらも `super` の後で helper を呼んでいたため、`super` に対する前後だけが違う。権限ダイアログの応答と `onPause` の到着順
// （契約表 1）は helper が `permissionId` / `startPendingResume` で実際の状態を見て判断するため、この違いで変わらない。
public final class CameraPreviewStreamLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        CameraPreviewHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        CameraPreviewHelper.resumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        CameraPreviewHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        CameraPreviewHelper.detach(activity);
    }
}
