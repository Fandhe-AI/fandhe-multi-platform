package com.fandhe.androidapp;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

// 行動認識（Issue #1127）の配送先。`ActivityRecognitionHelper` が張った PendingIntent（明示コンポーネント宛て）
// から Play services に起こされ、Intent を `ActivityRecognitionHelper.onResult` へ渡すだけ。
// exported=false・intent-filter なし。Activity の生死とは無関係に動く（プロセス死後に残った孤児購読の回収も担う）。
public final class ActivityRecognitionReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        ActivityRecognitionHelper.onResult(context, intent);
    }
}
