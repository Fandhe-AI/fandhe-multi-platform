package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// ble-peripheral が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1331）。
// 宣言は `fandhe-cap-ble-peripheral` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `BlePeripheralHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。許可ダイアログのランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。
//
// Stopped は `Activity.onStop()` の中、つまり `super.onStop()` の中で届く。従来の `MainActivity` は `super.onStop()` の前に helper を呼んでいたが、同じ `onStop` の内側で呼ばれるため `isChangingConfigurations()` などの判定は変わらない。破棄（再生成を含む）では helper が公開と GATT サーバーを解放し、未解放が残った場合の回収は `stop` が担う。
public final class BlePeripheralLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        BlePeripheralHelper.attach(activity);
    }

    @Override
    protected void onHostStopped(ComponentActivity activity) {
        BlePeripheralHelper.stopped(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        BlePeripheralHelper.detach(activity);
    }
}
