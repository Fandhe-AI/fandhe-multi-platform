// 互換用: パッケージ移動前（com.fandhe.androidapp）に WorkManager へ予約済みの Worker を、
// 新版でも生成できるようにする薄いサブクラス。WorkManager は Worker の完全修飾クラス名を
// DB へ保存するため、旧名を残して新クラスへ委譲する。新規の予約は com.fandhe.platform 側を使う。
package com.fandhe.androidapp;

import android.content.Context;
import androidx.annotation.NonNull;
import androidx.work.WorkerParameters;

public final class BackgroundTaskWorker extends com.fandhe.platform.BackgroundTaskWorker {
    public BackgroundTaskWorker(@NonNull Context context, @NonNull WorkerParameters params) {
        super(context, params);
    }
}
