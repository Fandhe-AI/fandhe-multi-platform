package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// bluetooth-classic が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1331）。
// 宣言は `fandhe-cap-bluetooth-classic` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `BluetoothClassicHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。許可ダイアログのランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。
//
// Started / Stopped / Resumed / Paused は `Activity.onXxx()` の中、つまり `super.onXxx()` の中で届く。従来の `MainActivity` は `super` に対する前後の別の位置で helper を呼んでいたため、`super` に対する前後だけが違う。helper の状態は main thread の同じ経路で冪等に決まり、前後の違いで結果は変わらない。
public final class BluetoothClassicLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        BluetoothClassicHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        BluetoothClassicHelper.resumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        BluetoothClassicHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        BluetoothClassicHelper.detach(activity);
    }
}
