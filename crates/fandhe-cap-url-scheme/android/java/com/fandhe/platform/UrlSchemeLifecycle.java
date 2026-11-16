package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// url-scheme が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1333、共通層の口は #1325）。
// 宣言は `fandhe-cap-url-scheme` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 利用者の `MainActivity` の `onCreate` / `onNewIntent` に `UrlSchemeHelper.deliver` を書かせない。
//
// Intent の配送は共通層の `deliverIntents`（起動時は再生成・履歴起動で配らない、起動中は `onNewIntent`）に任せる。
// 同一プロセス内の再生成は Rust 側 static の受信箱が保持するので表示は保たれる。
// 従来の `MainActivity.onNewIntent` との違い: 従来は `setIntent(intent)` で最新の Intent を保持していたが、
// 本経路は保持しない（`getIntent()` を読む利用者は `deliverIntents` の起動時配送だけを使う）。
// 守らない範囲: プロセス死亡後の復元では前回の URL を再表示しない（新プロセスの受信箱は空）。
// `launchMode="singleTask"` で起動中の URL は新インスタンスではなく `onNewIntent` へ来る。
public final class UrlSchemeLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        deliverIntents(activity, savedInstanceState, UrlSchemeHelper::deliver);
    }
}
