package com.fandhe.platform;

import android.os.Bundle;

import androidx.activity.ComponentActivity;

// payment-request が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1335）。
// 宣言は `fandhe-cap-payment-request` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
// 実処理は `PaymentRequestHelper` に任せる（仕組みは `FeatureLifecycleProvider` 参照）。
// 決済結果のランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。
//
// Destroyed は `Activity.onDestroy()` の中、つまり `super.onDestroy()` の中で届く。従来の `MainActivity` は
// `super` の後で helper を呼んでいたため、`super` に対する前後だけが違う。追跡を外すだけの処理で、前後の違いで結果は変わらない。
public final class PaymentRequestLifecycle extends FeatureLifecycleProvider {
    @Override
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {
        PaymentRequestHelper.attach(activity);
    }

    @Override
    protected void onHostDestroyed(ComponentActivity activity) {
        PaymentRequestHelper.detach(activity);
    }
}
