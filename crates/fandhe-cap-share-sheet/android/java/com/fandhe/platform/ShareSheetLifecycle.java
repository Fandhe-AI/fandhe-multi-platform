package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// share-sheet が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1257、#1302）。
// 宣言は `fandhe-cap-share-sheet` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `ShareSheetHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
public final class ShareSheetLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        ShareSheetHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        ShareSheetHelper.detach(activity);
    }
}
