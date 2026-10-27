package com.fandhe.androidapp;

import android.content.Context;
import android.content.RestrictionsManager;
import android.os.Bundle;

// MDM 管理アプリ設定の読み取り（Issue #946、親 #185）の Android 実装ヘルパー。MainActivity.managedAppConfigRead()
// から委譲される。Rust の `device/android_managed_app_config.rs` がワーカースレッドから
// 押下ごとに JNI（`()Ljava/lang/String;`）で 1 回呼ぶ同期 getter で、Java -> Rust の native
// コールバックは無い（no-op スタブ不要）。getApplicationRestrictions はディスク I/O を伴うので UI
// スレッドから呼ばない。
//
// 戻り値のワイヤ形式（SSOT は `fandhe_device::managed_app_config` のモジュール doc、ここはその写し）:
//   <bool>U+001F<integer>U+001F<text>U+001F<other>
//   各キー欄  "-" 未設定 / "?" 型違い（値が null を含む）/ bool は "1"・"0" / integer は 10 進 /
//             text は文字数（コードポイント数）
//   other 欄  固定 3 キー以外のキー数（キー名・値は運ばない）
// 制限なし（null・空の Bundle）は "-U+001F-U+001F-U+001F0"（Rust で NoConfig）。全体の失敗
// （サービス null・RuntimeException）は null（Rust で Internal）。失敗を「設定なし」に畳まない。
//
// 文字列の中身・例外の文言は読まず、運ばず、ログにも出さない。権限は不要（MDM / EMM 側の配布が前提）。
// getSystemService(Class) と RestrictionsManager は API 23 以降（minSdk 24）。Activity は握らず
// ApplicationContext だけを使い、要求 id・保留状態も持たないので、Activity 再生成で破棄すべきものは無い。
// 守る範囲: 失敗・型違い・未設定の区別。守らない範囲: 読み取り後の変化の反映（再押下まで）、
// API 34+ の Supervision 管理者の値、OS 呼び出しが戻らない場合の期限。
final class ManagedAppConfigHelper {
    private static final char SEP = '\u001F';
    private static final String KEY_BOOL = "fandheSampleBool";
    private static final String KEY_INTEGER = "fandheSampleInteger";
    private static final String KEY_TEXT = "fandheSampleText";

    private ManagedAppConfigHelper() {}

    // 型ごとの getter では型違いを判別できないので Bundle.get を使う（API 33 で deprecated）。
    @SuppressWarnings("deprecation")
    static String read(Context context) {
        try {
            RestrictionsManager rm = context.getSystemService(RestrictionsManager.class);
            if (rm == null) {
                return null;
            }
            Bundle b = rm.getApplicationRestrictions();
            if (b == null || b.isEmpty()) {
                return "-" + SEP + "-" + SEP + "-" + SEP + "0";
            }
            String bool = "-";
            int known = 0;
            if (b.containsKey(KEY_BOOL)) {
                known++;
                Object v = b.get(KEY_BOOL);
                bool = v instanceof Boolean ? ((Boolean) v ? "1" : "0") : "?";
            }
            String integer = "-";
            if (b.containsKey(KEY_INTEGER)) {
                known++;
                Object v = b.get(KEY_INTEGER);
                integer = (v instanceof Integer || v instanceof Long)
                        ? String.valueOf(((Number) v).longValue()) : "?";
            }
            String text = "-";
            if (b.containsKey(KEY_TEXT)) {
                known++;
                Object v = b.get(KEY_TEXT);
                text = v instanceof String
                        ? String.valueOf(((String) v).codePointCount(0, ((String) v).length())) : "?";
            }
            int other = b.keySet().size() - known;
            return bool + SEP + integer + SEP + text + SEP + other;
        } catch (RuntimeException e) {
            return null;
        }
    }
}
