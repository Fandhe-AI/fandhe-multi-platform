package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// companion-device が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1191）。
// 宣言は `fandhe-cap-companion-device` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `CompanionDeviceHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// `onHostCreated` は STARTED より前に呼ばれるため、結果ランチャーの登録（`registerForActivityResult`）はここで行える。
// 状態の保存はしない（再生成後の旧要求は Rust 側 `reset_pending_tracking` が捨てる）。
public final class CompanionDeviceLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        CompanionDeviceHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        CompanionDeviceHelper.detach(activity);
    }
}
