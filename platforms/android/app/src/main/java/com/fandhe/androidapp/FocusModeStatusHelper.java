package com.fandhe.androidapp;

import android.app.NotificationManager;
import android.content.Context;

// 集中モード・おやすみモードの状態（Issue #439）の Android 実装ヘルパー。
// Rust の `device/android_focus_mode_status.rs` が `MainActivity.focusModeStatusRead` 経由で
// 1 秒間隔に同期で呼ぶ（JNI 署名 `()I`）。
// コールバック登録・要求 id・保留状態を持たないため、Activity 再生成で破棄すべき要求は無い。
// 読み取りのみなので ACCESS_NOTIFICATION_POLICY は宣言しない。状態の値はログへ出さない。
// 定数の値は Rust の `fandhe_device::focus_mode_status::focus_from_code` と一致させること。
//   0: オフ、1: オン、-1: 失敗（UNKNOWN・未定義値・取得不能を含む）。
final class FocusModeStatusHelper {
    static final int OFF = 0;
    static final int ON = 1;
    static final int FAILED = -1;

    private FocusModeStatusHelper() {}

    static int read(Context context) {
        try {
            NotificationManager nm = context.getSystemService(NotificationManager.class);
            if (nm == null) {
                return FAILED;
            }
            switch (nm.getCurrentInterruptionFilter()) {
                case NotificationManager.INTERRUPTION_FILTER_ALL:
                    return OFF;
                // フィルタの種類は区別せず、通知を絞っている状態をオンとみなす。
                case NotificationManager.INTERRUPTION_FILTER_PRIORITY:
                case NotificationManager.INTERRUPTION_FILTER_NONE:
                case NotificationManager.INTERRUPTION_FILTER_ALARMS:
                    return ON;
                default:
                    return FAILED;
            }
        } catch (RuntimeException e) {
            return FAILED;
        }
    }
}
