package com.fandhe.androidapp;

import android.content.Context;
import android.content.SharedPreferences;

import java.util.concurrent.atomic.AtomicLong;

// クイック設定タイル（Issue #720、親 #139）の回数の保存先。`QuickSettingsTileService.onClick` が
// `increment` を呼び、Rust の `device/android_quick_settings_tile.rs` が `MainActivity` 経由で
// `readRaw` / `failedPresses` を JNI で読む。TileService と MainActivity は同じプロセスで動くため、
// 排他の `LOCK` と失敗件数 `FAILED` は static で共有できる。
//
// 保存形式: SharedPreferences `fandhe_quick_settings_tile` の key `count` に ASCII 10 進数の String
// （Java の long は符号付きなので u64 の範囲を文字列で保つ。Rust の `parse_count` と同じ解釈）。
// 守る範囲: 1 プロセス内で 1 押下 = +1（読み・加算・commit を LOCK の内側で行う）、保存に失敗した
// 押下を数えたことにしない（FAILED を +1 し保存値は上書きしない）、壊れた内容・溢れを 0 に戻さない。
// 守らない範囲: プロセス終了で FAILED が消えること・複数プロセス・タイル側への回数表示。
// commit() はメインスレッドで走るが、値は小さく、契約上 commit で成否を確定させる必要がある。
// 回数・保存内容・例外の文言はログに出さない。切り離し時はこのクラスごと削除する。
final class QuickSettingsTileStore {
    static final String PREFS_NAME = "fandhe_quick_settings_tile";
    static final String KEY_COUNT = "count";

    private static final Object LOCK = new Object();
    private static final AtomicLong FAILED = new AtomicLong(0);

    private QuickSettingsTileStore() {}

    /** 1 押下ぶん加算して保存する。失敗した押下は数えず FAILED を +1 する。 */
    static void increment(Context context) {
        synchronized (LOCK) {
            if (!tryIncrement(context)) {
                FAILED.incrementAndGet();
            }
        }
    }

    private static boolean tryIncrement(Context context) {
        try {
            SharedPreferences prefs =
                    context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE);
            String raw = prefs.getString(KEY_COUNT, null);
            long current = 0L;
            if (raw != null) {
                if (!isAsciiDigits(raw)) {
                    return false;
                }
                // u64 を超える値は NumberFormatException（catch で失敗扱い）。
                current = Long.parseUnsignedLong(raw);
            }
            // -1L は u64::MAX。加算すると巻き戻るので失敗にする。
            if (current == -1L) {
                return false;
            }
            return prefs.edit()
                    .putString(KEY_COUNT, Long.toUnsignedString(current + 1L))
                    .commit();
        } catch (RuntimeException e) {
            // NumberFormatException・別の型で保存されていた場合の ClassCastException など。
            return false;
        }
    }

    /** 保存内容をそのまま返す。未保存は null。解釈は Rust 側（`parse_count`）が行う。 */
    static String readRaw(Context context) {
        synchronized (LOCK) {
            return context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                    .getString(KEY_COUNT, null);
        }
    }

    /** このプロセスで保存に失敗した押下の件数。 */
    static long failedPresses() {
        return FAILED.get();
    }

    private static boolean isAsciiDigits(String s) {
        if (s.isEmpty()) {
            return false;
        }
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            if (c < '0' || c > '9') {
                return false;
            }
        }
        return true;
    }
}
