package com.fandhe.platform;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.provider.CalendarContract;

import androidx.activity.ComponentActivity;
import androidx.lifecycle.Lifecycle;

import java.util.Calendar;
import java.util.GregorianCalendar;

// 予定の追加（Issue #1226、#1187）の Android 実装ヘルパー。利用者の `MainActivity` には何も書かせない。
// Rust の `src/android.rs` が static の `request(Activity, id, title, ...)` を JNI で呼び、結果を
// `nativeOnCalendarResult` で受けて `driver::bridge` と pull 型で橋渡しする。状態は新しい要求に差し替えられた
// 旧要求を UI スレッド側で捨てるための追跡（`request`）だけで、Activity のライフサイクルへ参加する必要はない。
//
// カレンダーアプリの編集画面（`ACTION_INSERT`）を開くだけで権限は使わない。結果コードは `startActivity` が
// 例外なく戻った時点で返し、編集画面からの戻りは待たない（結果コードは保存の有無を表さないため）。
// 件名・日時はログへ出さない。
//
// 結果コード（本ファイルが SSOT、Rust 側 `outcome_from_android_code` の doc と対）:
//   0=編集画面へ渡した（保存の有無は分からない）, 1=カレンダーアプリ無し,
//   2=非前景・破棄中で起動しなかった, 3=その他の起動失敗。
final class CalendarHelper {
    static final int RESULT_HANDED = 0;
    static final int RESULT_NO_APP = 1;
    static final int RESULT_BACKGROUND = 2;
    static final int RESULT_LAUNCH_FAILED = 3;

    private static native void nativeOnCalendarResult(long requestId, int code);

    // 新しい要求に差し替えられた旧要求を UI スレッド側で捨てるための追跡。
    private static final RequestTracker REQUEST = new RequestTracker();

    private CalendarHelper() {}

    static void request(Activity activity, long requestId, String title,
            int sy, int smo, int sd, int sh, int smi,
            int ey, int emo, int ed, int eh, int emi) {
        REQUEST.track(requestId);
        activity.runOnUiThread(() -> {
            if (!REQUEST.complete(requestId)) {
                return; // 新しい要求に差し替えられた旧要求
            }
            boolean foreground = activity instanceof ComponentActivity
                    && ((ComponentActivity) activity).getLifecycle().getCurrentState()
                            .isAtLeast(Lifecycle.State.RESUMED);
            if (activity.isFinishing() || activity.isDestroyed() || !foreground) {
                // 背景からの起動は OS に黙って抑止されうる（嘘の成功の防止）。
                nativeOnCalendarResult(requestId, RESULT_BACKGROUND);
                return;
            }
            // th_TH 等で仏暦になる Calendar.getInstance() を避け、西暦固定にする。
            // 1582 年より前もユリウス暦へ倒さず、Rust 側の検証（先発グレゴリオ暦）と暦を揃える。
            Calendar begin = new GregorianCalendar();
            ((GregorianCalendar) begin).setGregorianChange(new java.util.Date(Long.MIN_VALUE));
            begin.clear();
            begin.set(sy, smo - 1, sd, sh, smi);
            Calendar end = new GregorianCalendar();
            ((GregorianCalendar) end).setGregorianChange(new java.util.Date(Long.MIN_VALUE));
            end.clear();
            end.set(ey, emo - 1, ed, eh, emi);
            Intent intent = new Intent(Intent.ACTION_INSERT)
                    .setData(CalendarContract.Events.CONTENT_URI)
                    .putExtra(CalendarContract.Events.TITLE, title)
                    .putExtra(CalendarContract.EXTRA_EVENT_BEGIN_TIME, begin.getTimeInMillis())
                    .putExtra(CalendarContract.EXTRA_EVENT_END_TIME, end.getTimeInMillis());
            try {
                activity.startActivity(intent);
                nativeOnCalendarResult(requestId, RESULT_HANDED);
            } catch (ActivityNotFoundException e) {
                nativeOnCalendarResult(requestId, RESULT_NO_APP);
            } catch (RuntimeException e) {
                nativeOnCalendarResult(requestId, RESULT_LAUNCH_FAILED);
            }
        });
    }
}
