package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// share-receive が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1257）。
// 宣言は `fandhe-cap-share-receive` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 利用者の `MainActivity` の `onCreate` / `onNewIntent` に `ShareReceiveHelper.deliver` を書かせない。
//
// Intent の配送は共通層の `deliverIntents`（起動時は再生成・履歴起動で配らない、起動中は `onNewIntent`）に任せる。
// 同一プロセス内の再生成は Rust 側 static の受信箱が保持するので表示は保たれる。
// 守らない範囲: プロセス死亡後の復元では前回の共有を再表示しない。
// `launchMode="singleTask"` で起動中の共有は新インスタンスではなく `onNewIntent` へ来る。`deliver` は ACTION_SEND / SEND_MULTIPLE 以外を
// 無視するので、利用者の `onNewIntent` が他の用途（URL・認証コールバック）に使われていても衝突しない。
public final class ShareReceiveLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        deliverIntents(activity, savedInstanceState, ShareReceiveHelper::deliver);
    }
}
