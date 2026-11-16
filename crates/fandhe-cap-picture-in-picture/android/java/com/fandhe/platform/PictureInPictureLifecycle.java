package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// picture-in-picture が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1335）。
// 宣言は `fandhe-cap-picture-in-picture` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `PictureInPictureHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
//
// 小窓の出入りは `attach`（`Activity.onCreate` の中）で `ComponentActivity.addOnPictureInPictureModeChangedListener`
// へ登録する（`MainActivity` の `onPictureInPictureModeChanged` 上書きは不要。口の設計は capability-crate-migration.md §18）。
// Destroyed は `Activity.onDestroy()` の中、つまり `super.onDestroy()` の中で届く。従来の `MainActivity` は
// `super` の後で helper を呼んでいたため、`super` に対する前後だけが違う。状態を捨てるだけの処理で、結果は変わらない。
public final class PictureInPictureLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        PictureInPictureHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        PictureInPictureHelper.detach(activity);
    }
}
