package com.fandhe.androidapp;

import android.app.LocaleManager;
import android.content.Context;
import android.os.Build;
import android.os.LocaleList;
import java.util.Locale;

/**
 * アプリ別の言語設定（Issue #1075、親 #137）の Android ヘルパー。
 *
 * <p>呼び出し元: {@code MainActivity.perAppLanguageRead()}（JNI シグネチャ
 * {@code ()Ljava/lang/String;}）。Rust 側 {@code device/android_per_app_language.rs} が
 * ボタン押下ごとに同期で 1 回呼ぶ。native コールバックは無いので no-op スタブは要らない。
 *
 * <p>戻り値の符号化（値は Rust の {@code android_per_app_language::from_android} と一致させること）。
 * {@code "unsupported"}: API 33 未満 / {@code "app:<tag>"}: アプリ別の設定 /
 * {@code "system:<tag>"}: OS 全体の設定 / null: 失敗。タグの検査は Rust 側の {@code clean_tag} が正。
 *
 * <p>権限は不要。保留状態を持たないので Activity 再生成で破棄するものは無い。値はログに出さない。
 */
final class PerAppLanguageHelper {
    private PerAppLanguageHelper() {}

    static String read(Context context) {
        // minSdk 24 のため、LocaleManager の参照はすべてこの分岐の後ろに置く。
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            return "unsupported";
        }
        try {
            LocaleManager lm = context.getSystemService(LocaleManager.class);
            if (lm == null) {
                return null;
            }
            LocaleList app = lm.getApplicationLocales();
            if (!app.isEmpty()) {
                String t = tag(app.get(0));
                return t == null ? null : "app:" + t;
            }
            // アプリ別の設定が無い（空）は失敗ではなく、OS 全体の設定に従っている状態。
            LocaleList sys = lm.getSystemLocales();
            if (sys.isEmpty()) {
                return null;
            }
            String t = tag(sys.get(0));
            return t == null ? null : "system:" + t;
        } catch (RuntimeException e) {
            return null;
        }
    }

    // 言語なし（ROOT の "und"）は成功にしない。拡張（地域設定の -u-... 等）は 35 文字上限を超えうるので落とす。
    private static String tag(Locale l) {
        if (l == null || l.getLanguage().isEmpty()) {
            return null;
        }
        return l.stripExtensions().toLanguageTag();
    }
}
