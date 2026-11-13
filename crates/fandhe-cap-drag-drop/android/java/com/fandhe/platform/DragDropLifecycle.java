package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// drag-drop が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1257、#1302）。
// 宣言は `fandhe-cap-drag-drop` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `DragDropHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// DecorView への取り付けは `onHostCreated` では行わない（この時点で `getDecorView()` を呼ぶと、後続の
// `requestWindowFeature` 等が使えなくなるおそれがあるため）。前面へ戻るたびに取り付ける（同じリスナーの再設定は無害）。
public final class DragDropLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        DragDropHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        DragDropHelper.install(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        DragDropHelper.detach(activity);
    }
}
