package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// flashlight が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1329）。
// 宣言は `fandhe-cap-flashlight` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `FlashlightHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
//
// Started / Stopped は `Activity.onStart()` / `onStop()` の中、つまり `super.onXxx()` の中で届く。従来の `MainActivity` は
// `onStart` では `super` の後で、`onStop` では `super` の前に helper を呼んでいたため、`super` に対する前後だけが違う。
// いずれも自分が点けた分の消灯と背景中の点灯拒否が目的で、`super` との前後で結果は変わらない。
public final class FlashlightLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        FlashlightHelper.attach(activity);
    }

    @Override
    protected void onHostStarted(ComponentActivity activity) {
        FlashlightHelper.started(activity);
    }

    @Override
    protected void onHostStopped(ComponentActivity activity) {
        FlashlightHelper.stopped(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        FlashlightHelper.detach(activity);
    }
}
