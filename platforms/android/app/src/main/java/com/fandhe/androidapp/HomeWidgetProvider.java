package com.fandhe.androidapp;

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
 * Rust 側（`device/android_home_widget.rs`）から JNI 経由で呼ばれる
 * `MainActivity.homeWidgetSetText` が行い、本クラスは読み出しと再描画要求を担う。
 * `android:process` を指定していないため、アプリ本体と同じプロセスで動く。
 * 反映は OS 側で非同期（契約は `fandhe_device::home_widget::HomeWidget` 参照）。
 *
 * <p>切り離し時に削除する（README.md「Phase 8（端末機能）の切り離し手順」参照）。
 */
public class HomeWidgetProvider extends AppWidgetProvider {
    static final String PREFS_NAME = "fandhe_home_widget";
    static final String KEY_TEXT = "text";
    // macOS 側 `FandheTextWidget` の未保存表示と揃える。
    private static final String UNSET_TEXT = "未設定";

    /** 保存済みテキスト。未保存は null（空文字と区別する）。 */
    static String readText(Context context) {
        return context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .getString(KEY_TEXT, null);
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
