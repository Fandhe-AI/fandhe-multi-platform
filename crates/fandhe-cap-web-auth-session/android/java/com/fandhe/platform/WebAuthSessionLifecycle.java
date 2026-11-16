package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// web-auth-session が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1333、共通層の口は #1325）。
// 宣言は `fandhe-cap-web-auth-session` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 利用者の `MainActivity` の `onNewIntent` / `onPause` / `onResume` に何も書かせない。
//
// コールバック URL は `onHostCreated` で足す `addOnNewIntentListener`（`WebAuthSessionHelper.attach`）が受ける。
// Paused / Resumed は `Activity.onPause()` / `onResume()` の中、つまり `super.onXxx()` の中で届く。従来の `MainActivity` は
// `super` の後で helper を呼んでいたため、`super` に対する前後だけが違う。helper の状態は同じ経路（UI スレッド・`this` のロック）で
// 冪等に決まる（離脱の記録とコールバックの確定は、どちらが先でも `onNewIntent` → `onResume` の順で矛盾しない）。
public final class WebAuthSessionLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        WebAuthSessionHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        WebAuthSessionHelper.resumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        WebAuthSessionHelper.paused(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        WebAuthSessionHelper.detach(activity);
    }
}
