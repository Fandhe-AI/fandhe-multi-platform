package com.fandhe.platform;

import android.content.Intent;
import android.os.Bundle;

import androidx.activity.ComponentActivity;

// share-receive が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1257）。
// 宣言は `fandhe-cap-share-receive` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 利用者の `MainActivity` の `onCreate` / `onNewIntent` に `ShareReceiveHelper.deliver` を書かせない。
//
// コールドスタート: 再生成（savedInstanceState あり）と履歴（Recents）からの再起動（元のインテントが
// 再送される）では届けず、二重配送を避ける。同一プロセス内の再生成は Rust 側 static の受信箱が保持するので
// 表示は保たれる。守らない範囲: プロセス死亡後の復元では前回の共有を再表示しない。
// 起動中: `ComponentActivity` の `addOnNewIntentListener` で `onNewIntent` を受ける（`launchMode="singleTask"` で
// 起動中の共有は新インスタンスではなくここへ来る）。`deliver` は ACTION_SEND / SEND_MULTIPLE 以外を
// 無視するので、利用者の `onNewIntent` が他の用途（URL・認証コールバック）に使われていても衝突しない。
public final class ShareReceiveLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        activity.addOnNewIntentListener(ShareReceiveHelper::deliver);
        Intent intent = activity.getIntent();
        if (savedInstanceState == null
                && (intent.getFlags() & Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) == 0) {
            ShareReceiveHelper.deliver(intent);
        }
    }
}
