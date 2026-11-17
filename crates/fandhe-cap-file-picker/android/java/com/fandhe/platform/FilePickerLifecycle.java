package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// file-picker が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1354）。
// 宣言は `fandhe-cap-file-picker` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `FilePickerHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// `onHostCreated` は STARTED より前に呼ばれるため、結果ランチャーの登録（`register`）はここで行える。
// 従来は `MainActivity` のフィールド初期化子で登録していた（同じ `onCreate` より前）。要求 id は static のため、再生成後の新 helper が
// 同じキーで再登録されたランチャーの callback で保留中の結果を拾う。
public final class FilePickerLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        FilePickerHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        FilePickerHelper.detach(activity);
    }
}
