package com.fandhe.androidapp;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.os.Build;

// 通話 UI 統合（Issue #278）の着信通知の応答・拒否ボタンの受け口。exported=false、明示 Intent
// 宛ての PendingIntent（FLAG_IMMUTABLE）だけが届く。extras の id が `CallUiHelper.currentId` と
// 一致し Connection が生きているときだけ処理する。プロセス死後に残った通知のボタンは
// currentId が NONE なので native を呼ばず、通知を消すだけで戻る。
public final class CallUiActionReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        long id = intent == null
                ? RequestTracker.NONE
                : intent.getLongExtra(CallUiHelper.EXTRA_CALL_ID, RequestTracker.NONE);
        CallUiConnectionService.CallConnection c = CallUiHelper.connection;
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O
                || id == RequestTracker.NONE
                || id != CallUiHelper.currentId
                || c == null) {
            CallUiHelper.cancelNotification();
            return;
        }
        String action = intent.getAction();
        if (CallUiHelper.ACTION_ANSWER.equals(action)) {
            c.doAnswer();
        } else if (CallUiHelper.ACTION_DECLINE.equals(action)) {
            c.doReject();
        }
    }
}
