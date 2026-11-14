package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// biometric-auth が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1330）。
// 宣言は `fandhe-cap-biometric-auth` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `BiometricAuthHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
//
// 前面の判定（`resumed`）は `onHostResumed` / `onHostPaused` で更新する。どちらも `Activity.onResume()` / `onPause()` の
// 中（`super.onResume()` / `super.onPause()` の内側）で届く。従来の `MainActivity` は `super` の後で `activityResumed` を
// 更新していたため、`super` に対する前後だけが違う。`BiometricPrompt` の提示は UI キュー上で行い、その時点の `resumed` を
// 見るため、順序の違いで state-save 後の提示を通すことはない。
public final class BiometricAuthLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        BiometricAuthHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        BiometricAuthHelper.setResumed(activity, true);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        BiometricAuthHelper.setResumed(activity, false);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        BiometricAuthHelper.detach(activity);
    }
}
