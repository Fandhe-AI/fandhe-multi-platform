package com.fandhe.platform;

import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProvider;
import android.content.ComponentName;
import android.content.Context;
import android.widget.RemoteViews;

/**
 * ホーム画面ウィジェット（Issue #1067、親 #1064）の `AppWidgetProvider`。アプリが
 * 保存した 1 行テキストを表示するだけの最小実装（Glance は依存が増えるため使わない）。
 *
 * <p>保存先は SharedPreferences（{@link #PREFS_NAME} / {@link #KEY_TEXT}）。書き込みは
 * Rust 側（`crates/fandhe-cap-home-widget/src/android.rs`）から JNI 経由で呼ばれる
 * static の {@link #setText}（利用者の `MainActivity` に受け口は要らない、#1257）が行い、
 * 読み出し（{@link #readText}）と再描画要求も本クラスが担う。
 * `android:process` を指定していないため、アプリ本体と同じプロセスで動く。
 * 反映は OS 側で非同期（契約は `fandhe_cap_home_widget::HomeWidget` 参照）。
 *
 * <p>切り離し時は crate ごと削除する（docs/architecture/device-feature-removal.md 参照）。
 */
public class HomeWidgetProvider extends AppWidgetProvider {
    static final String PREFS_NAME = "fandhe_home_widget";
    static final String KEY_TEXT = "text";
    // macOS 側 `FandheTextWidget` の未保存表示と揃える。
    private static final String UNSET_TEXT = "未設定";

    /**
     * 保存済みテキスト。未保存は null（空文字と区別する）。
     * Rust から JNI で直接呼ばれる。署名: (Landroid/content/Context;)Ljava/lang/String;
     */
    static String readText(Context context) {
        return context.getApplicationContext()
                .getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .getString(KEY_TEXT, null);
    }

    /**
     * Rust のワーカースレッドから JNI で呼ばれる同期メソッド。SharedPreferences へ `commit()` で保存し、
     * 成否を返す（`apply()` は非同期で直後の読み出しが古くなりうるため使わない）。`true` は「保存できた」
     * ことだけを約束し、再描画要求はベストエフォート（ウィジェット 0 個の空振りも更新要求の例外も
     * `commit()` 成功なら `true`、macOS 実装と同じ）。保存値はログへ出さない。
     * 署名: (Landroid/content/Context;Ljava/lang/String;)Z
     */
    static boolean setText(Context context, String text) {
        Context app = context.getApplicationContext();
        boolean committed = app.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .edit()
                .putString(KEY_TEXT, text)
                .commit();
        if (!committed) {
            return false;
        }
        try {
            requestUpdate(app);
        } catch (RuntimeException e) {
            // 再描画要求の失敗は保存成功を覆さない（例外の型名だけ記録し、保存値は出さない）。
            android.util.Log.w("FandheHomeWidget", "requestUpdate failed: " + e.getClass().getSimpleName());
        }
        return true;
    }

    private static RemoteViews buildViews(Context context) {
        String text = readText(context);
        RemoteViews views = new RemoteViews(context.getPackageName(), R.layout.home_widget);
        views.setTextViewText(R.id.home_widget_text, text == null ? UNSET_TEXT : text);
        return views;
    }

    /** 全ウィジェットへ再描画を要求する。ウィジェットが 0 個なら空振りする。 */
    static void requestUpdate(Context context) {
        AppWidgetManager.getInstance(context)
                .updateAppWidget(
                        new ComponentName(context, HomeWidgetProvider.class),
                        buildViews(context));
    }

    @Override
    public void onUpdate(Context context, AppWidgetManager manager, int[] appWidgetIds) {
        manager.updateAppWidget(appWidgetIds, buildViews(context));
    }
}
