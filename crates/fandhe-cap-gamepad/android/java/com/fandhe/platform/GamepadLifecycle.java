package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// gamepad が Activity のライフサイクルと入力へ参加するための非公開 ContentProvider（Issue #1303）。
// 宣言は `fandhe-cap-gamepad` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 利用者の `MainActivity` に `dispatchKeyEvent` / `dispatchGenericMotionEvent` の override を書かせない。
//
// 入力: `onHostCreated` で共通層 `HostInput`（#1327）へキー・ジョイスティックの観測者を登録する
// （`GamepadHelper.wrap`）。イベントは消費しない。再生成後は新しい Window へ登録し直す。
// 接続状態: 前面へ戻るたびに `InputManager` の監視を登録して走査し、前面を離れる時に解除して「無し」を通知する
// （バックグラウンド中の key up・切断の取りこぼしで「接続中」が張り付かないため）。
public final class GamepadLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        GamepadHelper.wrap(activity);
    }

    @Override
    protected void onHostResumed(ComponentActivity activity) {
        GamepadHelper.onResumed(activity);
    }

    @Override
    protected void onHostPaused(ComponentActivity activity) {
        GamepadHelper.onPaused();
    }
}
