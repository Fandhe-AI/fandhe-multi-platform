package com.fandhe.platform;

import android.annotation.SuppressLint;
import android.content.Context;
import android.provider.Settings;

/**
 * アプリ・端末の識別子（Issue #904、親 #181）の Android ヘルパー。
 *
 * <p>呼び出し元: Rust 側 {@code crates/fandhe-cap-device-identifier/src/android.rs} が、ボタン押下ごとに
 * static の {@code read(Context)} を JNI で直接 1 回呼ぶ（利用者の {@code MainActivity} に受け口は要らない、
 * Issue #1183）。JNI シグネチャ {@code (Landroid/content/Context;)Ljava/lang/String;} は Rust 側と一致させる。
 * Java から Rust への native コールバックは無いので no-op スタブは要らない。
 *
 * <p>{@code Settings.Secure.ANDROID_ID} をそのまま返す（trim しない）。権限は不要で minSdk 24 の
 * 全域で使える。Rust が渡す Activity は {@code getApplicationContext()} に取り替えて使い、保留状態を持たないため Activity 再生成で破棄する
 * ものは無い。値はログに出さない。失敗（null・例外）は null を返し、Rust 側が失敗にする。
 */
final class DeviceIdentifierHelper {
    private DeviceIdentifierHelper() {}

    // ANDROID_ID を読むこと自体がこの機能の目的で、範囲（読んで伏せて表示するだけ）は対応表の note で限定している。
    @SuppressLint("HardwareIds")
    static String read(Context context) {
        try {
            return Settings.Secure.getString(
                    context.getApplicationContext().getContentResolver(), Settings.Secure.ANDROID_ID);
        } catch (RuntimeException e) {
            return null;
        }
    }
}
