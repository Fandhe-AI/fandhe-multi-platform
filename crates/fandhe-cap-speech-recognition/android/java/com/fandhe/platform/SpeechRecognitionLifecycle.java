package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// 音声認識が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1354）。
// 宣言は `fandhe-cap-speech-recognition` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `SpeechRecognitionHelper` の `attach` / `detach` / `resumed` / `paused` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// `onHostCreated` は STARTED より前に呼ばれるため、許可ランチャーの登録（`register`）はここで行える。
//
// `onHostResumed` / `onHostPaused` / `onHostStopped` / `onHostDestroyed` は `Activity.onResume()` / `onPause()` / `onStop()` /
// `onDestroy()` の中（`super.onXxx()` の内側）で届く。前面フラグは resumed / paused で写す。構成変更でない背景遷移の聞き取りの中断は
// `onHostStopped` で行い（`isChangingConfigurations()` を確実に読める位置）、構成変更中は聞き取りを新しい helper へ引き継ぐ。
// 許可ダイアログの応答との到着順は、応答側が実際の前面の状態を見て保留にする形で吸収する。
public final class SpeechRecognitionLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        SpeechRecognitionHelper.attach(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        SpeechRecognitionHelper.resumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        SpeechRecognitionHelper.paused(activity);
    }

    @Override
    protected void onHostStopped(ComponentActivity activity) {
        SpeechRecognitionHelper.stopped(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        SpeechRecognitionHelper.detach(activity);
    }
}
