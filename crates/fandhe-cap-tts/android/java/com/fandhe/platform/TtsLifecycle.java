package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// 読み上げ（TTS）が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1354）。
// 宣言は `fandhe-cap-tts` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `TtsHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
//
// `onHostDestroyed` は `Activity.onDestroy()` の中（`super.onDestroy()` の内側）で届く。従来の `MainActivity.onDestroy` は
// `super.onDestroy()` の前に破棄フラグを立て、後で保留要求の確定と `TextToSpeech` の解放をしていた。
// 前後だけが違い、フラグ・確定・解放は `TtsHelper.detach` がこの順で冪等に行う（UI スレッドの直列キュー上で、
// 破棄後に実行される要求の Runnable と `onTtsInit` はフラグを見て拒否する）。
public final class TtsLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        TtsHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        TtsHelper.detach(activity);
    }
}
