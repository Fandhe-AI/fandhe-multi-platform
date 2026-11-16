package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// background-upload が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1333、共通層は #1257）。
// 宣言は `fandhe-cap-background-upload` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 利用者の `MainActivity.onCreate` に `BackgroundUploadHelper.onCreate()` を書かせない。
//
// Created は `Activity.onCreate()` の中（`super.onCreate` の中）で届く。従来の `MainActivity` は `super.onCreate` の後で
// helper を呼んでいたため、`super` に対する前後だけが違う。LiveData の登録は main thread・Activity が CREATED の間に行え、
// 登録し直しは冪等（インスタンスごとに 1 回）で順序に依存しない。
public final class BackgroundUploadLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        BackgroundUploadHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        BackgroundUploadHelper.detach(activity);
    }
}
