package com.fandhe.androidapp;

import android.app.UiModeManager;
import android.content.ContentResolver;
import android.content.Context;
import android.os.Build;
import android.provider.Settings;

// アクセシビリティ設定画面（Issue #1310）の Android 実装ヘルパー。
// Rust の `device/android_accessibility_prefs.rs` が `MainActivity.accessibilityPrefsRead` 経由で
// 1 秒間隔に同期で呼ぶ（JNI 署名 `()I`）。OS の事実を 2 ビット x 2 に詰めて返すだけで、
// 値の検証は Rust 側 `prefs_from_code`（ホストの `cargo test` で検査）が行う。
// コールバック登録・要求 id・保留状態を持たないため、Activity 再生成で破棄すべき要求は無い。
// 権限は不要。設定値はログへ出さない。
// 定数の値は Rust 側の同名定数（Apple 実装と同じビット表）と一致させること。
//   bit0-1: 視差軽減、bit2-3: コントラスト増加。各 UNKNOWN=0 / OFF=1 / ON=2。
final class AccessibilityPrefsHelper {
    static final int FAILED = -1;
    static final int UNKNOWN = 0;
    static final int OFF = 1;
    static final int ON = 2;
    static final int CONTRAST_SHIFT = 2;

    private AccessibilityPrefsHelper() {}

    static int read(Context context) {
        try {
            ContentResolver cr = context.getContentResolver();
            if (cr == null) {
                return FAILED;
            }
            // 視差軽減はアニメ倍率 0 で代用する。未設定の既定 1.0 は OS の既定と同じ（= オフ）。
            float scale = Settings.Global.getFloat(cr, Settings.Global.ANIMATOR_DURATION_SCALE, 1.0f);
            int motion;
            if (Float.isNaN(scale) || Float.isInfinite(scale) || scale < 0f) {
                motion = UNKNOWN;
            } else {
                motion = scale == 0f ? ON : OFF;
            }
            // UiModeManager.getContrast は API 34+。それ未満は公開 API が無いので不明。
            int contrast = UNKNOWN;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                UiModeManager um = context.getSystemService(UiModeManager.class);
                if (um == null) {
                    return FAILED;
                }
                float c = um.getContrast();
                if (Float.isNaN(c) || c < -1f || c > 1f) {
                    contrast = UNKNOWN;
                } else {
                    contrast = c > 0f ? ON : OFF;
                }
            }
            return motion | (contrast << CONTRAST_SHIFT);
        } catch (RuntimeException e) {
            return FAILED;
        }
    }
}
