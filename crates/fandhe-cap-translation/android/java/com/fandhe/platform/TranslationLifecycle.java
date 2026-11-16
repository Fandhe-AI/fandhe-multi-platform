package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// translation が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1330）。
// 宣言は `fandhe-cap-translation` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `TranslationHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 従来は `MainActivity.onDestroy` が `super` の前に helper を破棄していたが、`onHostDestroyed` は
// `Activity.onDestroy()` の中で届く。破棄は要求 id の照合で冪等なため順序の違いは結果に影響しない。
public final class TranslationLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        TranslationHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        TranslationHelper.detach(activity);
    }
}
