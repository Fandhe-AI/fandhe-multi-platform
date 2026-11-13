package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// nfc（タグ読み取り）が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1193）。
// 宣言は `fandhe-cap-nfc` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `NfcReadHelper` の `attach` / `paused` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// `onHostPaused` は `Activity.onPause()` の中（`super.onPause()` の中）で届く。従来の `MainActivity` は
// `super` の後で helper を呼んでいたため前後だけが違い、reader mode の解除と INTERRUPTED の確定は
// RESUMED の見直し（`begin`）と要求 id の照合で順序が入れ替わっても同じ結果になる。
public final class NfcReadLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        NfcReadHelper.attach(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        NfcReadHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        NfcReadHelper.detach(activity);
    }
}
