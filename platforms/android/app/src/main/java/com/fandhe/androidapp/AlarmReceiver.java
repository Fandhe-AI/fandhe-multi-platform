package com.fandhe.androidapp;

import android.app.Notification;
import android.app.NotificationManager;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.media.AudioManager;
import android.media.RingtoneManager;
import android.net.Uri;
import android.os.Build;

import androidx.core.app.NotificationCompat;

// アラーム（Issue #1078）の時刻に `AlarmHelper` の PendingIntent から起こされ、アラーム用チャンネルの
// 通知を鳴らすだけの受け口。exported=false・intent-filter なし。自アプリの明示 Intent
// （FLAG_IMMUTABLE）だけが届き、extras は読まない。Activity の生死とは無関係に動く。
public final class AlarmReceiver extends BroadcastReceiver {
    private static final int NOTIFICATION_ID = 1078;

    @Override
    public void onReceive(Context context, Intent intent) {
        try {
            AlarmHelper.ensureChannel(context);
            Uri sound = RingtoneManager.getDefaultUri(RingtoneManager.TYPE_ALARM);
            if (sound == null) sound = RingtoneManager.getDefaultUri(RingtoneManager.TYPE_NOTIFICATION);
            NotificationCompat.Builder b = new NotificationCompat.Builder(context, AlarmHelper.CHANNEL_ID)
                    .setSmallIcon(android.R.drawable.ic_lock_idle_alarm)
                    .setContentTitle(context.getString(R.string.alarm_notification_title))
                    .setContentText(context.getString(R.string.alarm_notification_text))
                    .setCategory(NotificationCompat.CATEGORY_ALARM)
                    .setPriority(NotificationCompat.PRIORITY_MAX)
                    .setAutoCancel(true);
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
                b.setSound(sound, AudioManager.STREAM_ALARM);
            }
            Notification n = b.build();
            // 通知を消すまで音を繰り返す。
            n.flags |= Notification.FLAG_INSISTENT;
            NotificationManager nm = context.getSystemService(NotificationManager.class);
            if (nm != null) nm.notify(NOTIFICATION_ID, n);
        } catch (RuntimeException e) {
            // SecurityException を含め握る（受信側で出来ることは無い）。
        }
    }
}
