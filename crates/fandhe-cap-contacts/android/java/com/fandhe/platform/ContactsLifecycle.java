package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// contacts が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1330）。
// 宣言は `fandhe-cap-contacts` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `ContactsHelper` の `attach` / `detach` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// `onHostCreated` は STARTED より前に呼ばれるため、結果ランチャーの登録（`register`）はここで行える。
// 従来は `MainActivity.onCreate` 内で要求 id を捨てていたが、同じ `onCreate` の中（`super.onCreate` の内側）で行う。
public final class ContactsLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        ContactsHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        ContactsHelper.detach(activity);
    }
}
