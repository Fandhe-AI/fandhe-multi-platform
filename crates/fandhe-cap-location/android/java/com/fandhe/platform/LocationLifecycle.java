package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// location が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1332）。
// 宣言は `fandhe-cap-location` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `LocationHelper` の `attach` / `resumed` / `paused` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 許可ダイアログのランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。
// `onHostResumed` / `onHostPaused` は `Activity.onResume()` / `onPause()` の中、つまり `super.onResume()` /
// `super.onPause()` の内側で届く。従来の `MainActivity` は `super` の後で測位の再登録・解除を行っていたため、
// `super` に対する前後だけが違う（測位の可否は UI スレッド上の `activityResumed` と static の意思フラグで決まり、
// 順序に依存しない）。
public final class LocationLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        LocationHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        LocationHelper.resumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        LocationHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        LocationHelper.detach(activity);
    }
}
