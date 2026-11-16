package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// pen-stylus が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1334）。
// 宣言は `fandhe-cap-pen-stylus` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `PenStylusHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
//
// タッチの観測は共通層 `HostInput`（`Window.Callback` を 1 段包み、通知してから元の戻り値をそのまま返す。イベントは消費しない）へ
// `onHostCreated` で登録する。背景遷移と破棄では接触なしへ戻し、最後の値を出し続けない。
public final class PenStylusLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        PenStylusHelper.attach(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        PenStylusHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        PenStylusHelper.detach(activity);
    }
}
