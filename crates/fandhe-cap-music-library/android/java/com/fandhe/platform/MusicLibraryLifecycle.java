package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// music-library が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1332）。
// 宣言は `fandhe-cap-music-library` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `MusicLibraryHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 許可ダイアログのランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。破棄時は
// 保留中の要求を RESULT_FAILED で確定して破棄する（従来の `MainActivity.onDestroy` の `super` の後ではなく、
// `Activity.onDestroy()` の `super` の内側で届く点だけが違う。結果の確定は `RequestTracker.take` で 1 回きり）。
public final class MusicLibraryLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        MusicLibraryHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        MusicLibraryHelper.detach(activity);
    }
}
