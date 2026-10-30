package com.fandhe.androidapp;

import android.app.Activity;
import android.app.AlarmManager;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.media.AudioAttributes;
import android.media.RingtoneManager;
import android.net.Uri;
import android.os.Build;
import android.provider.Settings;

import androidx.core.app.NotificationManagerCompat;

// アラーム・正確な時刻の予約（Issue #1078、親 #1056）。Rust 側 `device/android_alarm.rs` が
// `MainActivity.alarmSchedule/alarmCancel/alarmState` 経由でワーカースレッドから同期で呼ぶ。
// 保留中の要求・要求 id・native コールバックを持たないので、Activity 再生成時に破棄するものは無い。
//
// 結果コード（Rust 側 `schedule_result` / `cancel_result` の match が SSOT。値を揃えること）:
//   0 OK / 1 EXACT_ALARM_DENIED / 2 NOTIFICATIONS_DISABLED / 3 SECURITY / 4 FAILED
// state の戻り値: 0 = 未予約、正の値 = 鳴る時刻（UNIX ms）、負の値 = 失敗。Rust 側の検査で
// 鳴る時刻は常に現在より後なので、0 と負の値は時刻と衝突しない。
// 予約の有無は OS が持つ PendingIntent（FLAG_NO_CREATE で引く）を正にし、時刻だけ記録値を返す。
// 時刻・例外の文言はログに出さない。メソッドは static synchronized で直列化する
// （state の記録削除と schedule の予約作成が交差して「記録の無い予約」を作らないため）。
public final class AlarmHelper {
    static final int OK = 0;
    static final int EXACT_ALARM_DENIED = 1;
    static final int NOTIFICATIONS_DISABLED = 2;
    static final int SECURITY = 3;
    static final int FAILED = 4;

    static final String CHANNEL_ID = "fandhe_alarm";
    private static final String PREFS = "fandhe_alarm";
    private static final String KEY_FIRE_AT = "fire_at_ms";
    private static final int REQUEST_CODE = 1078;
    private static final int BASE_FLAGS = PendingIntent.FLAG_ONE_SHOT | PendingIntent.FLAG_IMMUTABLE;

    private AlarmHelper() {}

    // flags を一箇所に集める。存在確認は同じ flags に FLAG_NO_CREATE を足して引く。
    private static PendingIntent receiverIntent(Context c, boolean create) {
        Intent i = new Intent(c, AlarmReceiver.class);
        return PendingIntent.getBroadcast(
                c, REQUEST_CODE, i, create ? BASE_FLAGS : BASE_FLAGS | PendingIntent.FLAG_NO_CREATE);
    }

    static void ensureChannel(Context c) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return;
        NotificationManager nm = c.getSystemService(NotificationManager.class);
        if (nm == null) return;
        Uri sound = RingtoneManager.getDefaultUri(RingtoneManager.TYPE_ALARM);
        if (sound == null) sound = RingtoneManager.getDefaultUri(RingtoneManager.TYPE_NOTIFICATION);
        NotificationChannel ch = new NotificationChannel(
                CHANNEL_ID, c.getString(R.string.alarm_channel_name), NotificationManager.IMPORTANCE_HIGH);
        ch.setSound(sound, new AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ALARM)
                .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                .build());
        nm.createNotificationChannel(ch);
    }

    private static boolean notificationsEnabled(Context c) {
        if (!NotificationManagerCompat.from(c).areNotificationsEnabled()) return false;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            ensureChannel(c);
            NotificationManager nm = c.getSystemService(NotificationManager.class);
            NotificationChannel ch = nm == null ? null : nm.getNotificationChannel(CHANNEL_ID);
            if (ch != null && ch.getImportance() == NotificationManager.IMPORTANCE_NONE) return false;
        }
        return true;
    }

    private static void openExactAlarmSettings(final Activity a) {
        a.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (a.isFinishing() || a.isDestroyed()) return;
                try {
                    Intent i = new Intent(Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM,
                            Uri.parse("package:" + a.getPackageName()));
                    a.startActivity(i);
                } catch (ActivityNotFoundException e) {
                    // 開けなくても失敗コードは返っている。
                } catch (RuntimeException e) {
                    // 同上。
                }
            }
        });
    }

    static synchronized int schedule(Activity a, long fireAtMs) {
        try {
            Context c = a.getApplicationContext();
            if (!notificationsEnabled(c)) return NOTIFICATIONS_DISABLED;
            AlarmManager am = c.getSystemService(AlarmManager.class);
            if (am == null) return FAILED;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && !am.canScheduleExactAlarms()) {
                openExactAlarmSettings(a);
                return EXACT_ALARM_DENIED;
            }
            // 置き換え: 古い予約を先に取り消す（失敗時に古い予約は戻さない）。
            PendingIntent old = receiverIntent(c, false);
            if (old != null) {
                am.cancel(old);
                old.cancel();
            }
            SharedPreferences sp = c.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
            if (!sp.edit().putLong(KEY_FIRE_AT, fireAtMs).commit()) return FAILED;
            PendingIntent pi = receiverIntent(c, true);
            try {
                Intent show = new Intent(c, MainActivity.class);
                PendingIntent showPi = PendingIntent.getActivity(
                        c, REQUEST_CODE, show, PendingIntent.FLAG_IMMUTABLE);
                am.setAlarmClock(new AlarmManager.AlarmClockInfo(fireAtMs, showPi), pi);
                return OK;
            } catch (SecurityException e) {
                rollback(am, pi, sp);
                return SECURITY;
            } catch (RuntimeException e) {
                rollback(am, pi, sp);
                return FAILED;
            }
        } catch (RuntimeException e) {
            return FAILED;
        }
    }

    private static void rollback(AlarmManager am, PendingIntent pi, SharedPreferences sp) {
        try {
            am.cancel(pi);
            pi.cancel();
        } catch (RuntimeException e) {
            // 戻しの失敗は握る（呼び出し元が失敗コードを返す）。
        }
        sp.edit().remove(KEY_FIRE_AT).commit();
    }

    static synchronized int cancel(Context c) {
        try {
            AlarmManager am = c.getSystemService(AlarmManager.class);
            PendingIntent pi = receiverIntent(c, false);
            if (pi != null) {
                if (am != null) am.cancel(pi);
                pi.cancel();
            }
            boolean ok = c.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
                    .edit().remove(KEY_FIRE_AT).commit();
            return ok ? OK : FAILED;
        } catch (RuntimeException e) {
            return FAILED;
        }
    }

    static synchronized long state(Context c) {
        try {
            SharedPreferences sp = c.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
            if (receiverIntent(c, false) == null) {
                // 鳴った・電源断・許可の取り消しのいずれでも OS 側に無い。
                sp.edit().remove(KEY_FIRE_AT).commit();
                return 0L;
            }
            long at = sp.getLong(KEY_FIRE_AT, 0L);
            return at > 0 ? at : -1L;
        } catch (RuntimeException e) {
            return -1L;
        }
    }
}
