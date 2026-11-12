package com.fandhe.platform;

import java.text.BreakIterator;

// 単語分割画面（Issue #1095、#1187）の Android 実装ヘルパー。利用者の `MainActivity` には何も書かせない。
// Rust の `src/android.rs` が egui スレッドから static の `segment(String)` を JNI で同期に呼ぶ。Java -> Rust の
// native コールバックを持たないため no-op スタブは不要で、状態も持たないので Activity のライフサイクルへ
// 参加する必要もない。`BreakIterator`（API 24 = minSdk）の語境界で切った部分文字列を出現順に U+0000 で連結して
// 返す。入力は Rust 側 `validate_input` が NUL を拒否済みで、トークンは入力の部分文字列のため区切りは曖昧に
// ならない。空白・記号だけのトークンの除去は Rust 側 `normalize_words` が行う。
// トークン 0 件は空文字列、失敗（RuntimeException）は null（Java 例外を JNI 側へ残さず Rust が `Internal` へ写す）。
// 入力・トークンはログへ出さない。
final class NlpHelper {
    private NlpHelper() {}

    static String segment(String text) {
        try {
            BreakIterator it = BreakIterator.getWordInstance();
            it.setText(text);
            StringBuilder sb = new StringBuilder();
            boolean first = true;
            int start = it.first();
            for (int end = it.next(); end != BreakIterator.DONE; start = end, end = it.next()) {
                if (!first) {
                    sb.append('\0');
                }
                sb.append(text, start, end);
                first = false;
            }
            return sb.toString();
        } catch (RuntimeException e) {
            return null;
        }
    }
}
