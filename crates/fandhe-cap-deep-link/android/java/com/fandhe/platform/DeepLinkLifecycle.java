package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// deep-link（App Links）が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1333、共通層の口は #1325）。
// 宣言は `fandhe-cap-deep-link` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 利用者の `MainActivity` の `onCreate` / `onNewIntent` に `DeepLinkHelper.deliver` を書かせない。
//
// Intent の配送は共通層の `deliverIntents`（起動時は再生成・履歴起動で配らない、起動中は `onNewIntent`）に任せる。
// 従来の `MainActivity.onNewIntent` は `setIntent(intent)` で最新の Intent を保持していたが、本経路は保持しない。
// 守らない範囲: プロセス死亡後の復元では前回のリンクを再表示しない（新プロセスの受信箱は空）。
// `autoVerify` 付きの https の intent-filter（ホストは確認用ドメインに依存）は、宣言では表せないため
// flavor の `platforms/android/app/src/deeplink/AndroidManifest.xml` に残る（利用者の責務。crate doc 参照）。
public final class DeepLinkLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        deliverIntents(activity, savedInstanceState, DeepLinkHelper::deliver);
    }
}
