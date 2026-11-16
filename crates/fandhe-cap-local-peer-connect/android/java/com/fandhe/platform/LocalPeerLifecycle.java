package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// local-peer-connect が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1335）。
// 宣言は `fandhe-cap-local-peer-connect` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `LocalPeerHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 許可ダイアログのランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。
//
// Resumed / Paused / Destroyed は `Activity.onResume()` / `onPause()` / `onDestroy()` の中、つまり `super.onXxx()` の中で届く。
// 従来の `MainActivity` は `super` の後で helper を呼んでいたため、`super` に対する前後だけが違う。
// helper の状態は main thread で冪等に決まる。届く時点の `Lifecycle` の状態は既に RESUMED（Paused では STARTED）になっており、
// 前景判定（`foreground()`）と許可待ち後の再開（`startPendingResume`）の経路は前後の違いで結果が変わらない。
public final class LocalPeerLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        LocalPeerHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        LocalPeerHelper.resumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        LocalPeerHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        LocalPeerHelper.detach(activity);
    }
}
