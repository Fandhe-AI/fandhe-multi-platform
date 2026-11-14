package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// bluetooth-le が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1331）。
// 宣言は `fandhe-cap-bluetooth-le` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `BluetoothLeHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 許可ダイアログのランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。
//
// Resumed / Paused は `Activity.onResume()` / `onPause()` の中、つまり `super.onXxx()` の中で届く。従来の `MainActivity` は
// `super` の後で helper の処理を呼んでいたため、`super` に対する前後だけが違う。helper の状態は UI スレッドの同じ経路で
// 決まり、前後の違いで結果は変わらない。破棄（再生成を含む）ではスキャンを止め、追跡中の要求を中断で確定する。
public final class BluetoothLeLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        BluetoothLeHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        BluetoothLeHelper.resumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        BluetoothLeHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        BluetoothLeHelper.detach(activity);
    }
}
