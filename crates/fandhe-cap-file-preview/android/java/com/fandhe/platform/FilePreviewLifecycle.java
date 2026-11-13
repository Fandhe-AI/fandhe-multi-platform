package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// file-preview が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1192）。
// 宣言は `fandhe-cap-file-preview` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `FilePreviewHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 保留中の要求は Rust 側の id 照合で捨てられるため、状態の保存はしない。
public final class FilePreviewLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        FilePreviewHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        FilePreviewHelper.detach(activity);
    }
}
