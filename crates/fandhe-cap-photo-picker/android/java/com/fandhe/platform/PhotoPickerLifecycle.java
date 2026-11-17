package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// 写真選択が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1355）。
// 宣言は `fandhe-cap-photo-picker` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `PhotoPickerHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// `onHostCreated` は STARTED より前に呼ばれるため、結果ランチャーの登録（`register`）はここで行える。
// 従来は `MainActivity` のフィールド初期化子で登録していた（同じ `onCreate` より前）。
public final class PhotoPickerLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        PhotoPickerHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        PhotoPickerHelper.detach(activity);
    }
}
