package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// wifi-info が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1329）。
// 宣言は `fandhe-cap-wifi-info` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `WifiInfoHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 許可ダイアログのランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。保留中の要求は
// Activity ごとの helper が持ち、再生成では持ち越さない（破棄時に RESULT_FAILED で確定する）。
public final class WifiInfoLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        WifiInfoHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        WifiInfoHelper.detach(activity);
    }
}
