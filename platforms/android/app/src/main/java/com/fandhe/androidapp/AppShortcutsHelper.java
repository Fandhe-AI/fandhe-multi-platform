package com.fandhe.androidapp;

import android.content.Context;
import android.content.Intent;
import android.content.pm.ShortcutInfo;
import android.content.pm.ShortcutManager;
import android.net.Uri;
import android.os.Build;
import java.util.Collections;

// アプリショートカット（Issue #426、親 #101）の Android 実装ヘルパー。
// Rust の `device/android_app_shortcuts.rs` が `MainActivity.appShortcutsInstall` 経由で
// ワーカースレッドから同期に呼ぶ（JNI 署名 `(Ljava/lang/String;Ljava/lang/String;)I`）。
// 動的ショートカットを使う。静的ショートカットは CLEAR_TASK が固定で MainActivity が
// 作り直される（#259）ため使わない。intent は MainActivity への明示 intent（ACTION_VIEW）で、
// SINGLE_TOP + CLEAR_TOP により起動中は onNewIntent、未起動なら onCreate の既存 url-scheme
// 経路に届く。
// 要求 id・保留状態を持たない同期呼び出しで、Context は Application なので Activity 再生成の
// 影響を受けない。URL・例外の文言はログへ出さない。
// 結果コードの SSOT はここ。Rust の `registration_from_code` と一致させること。
final class AppShortcutsHelper {
    static final int RESULT_INSTALLED = 0;
    static final int RESULT_UNSUPPORTED = 1; // API 25 未満
    static final int RESULT_FAILED = 2;

    private static final String SHORTCUT_ID = "open-demo";

    private AppShortcutsHelper() {}

    static int install(Context context, String label, String url) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N_MR1) {
            return RESULT_UNSUPPORTED;
        }
        try {
            ShortcutManager manager = context.getSystemService(ShortcutManager.class);
            if (manager == null) {
                return RESULT_FAILED;
            }
            Intent intent =
                    new Intent(Intent.ACTION_VIEW, Uri.parse(url), context, MainActivity.class);
            intent.addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
            ShortcutInfo info =
                    new ShortcutInfo.Builder(context, SHORTCUT_ID)
                            .setShortLabel(label)
                            .setIntent(intent)
                            .build();
            // 置き換えなので、プロセスを起動し直しても重複しない。
            return manager.setDynamicShortcuts(Collections.singletonList(info))
                    ? RESULT_INSTALLED
                    : RESULT_FAILED;
        } catch (RuntimeException e) {
            return RESULT_FAILED;
        }
    }
}
