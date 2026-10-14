package com.fandhe.androidapp;

import android.content.Context;
import android.hardware.display.DisplayManager;
import android.util.DisplayMetrics;
import android.view.Display;

// 複数ディスプレイ画面（Issue #1343、親 #1134）の Android 実装ヘルパー。MainActivity.multiDisplayRead()
// から委譲される。Rust の `device/android_multi_display.rs` が egui スレッドから 1 秒ごとに
// JNI（`()Ljava/lang/String;`）で呼ぶ同期 getter で、Java -> Rust の native コールバックは
// 無い（no-op スタブ不要）。
//
// 戻り値は画面ごとのレコードを U+001E でつないだ文字列。レコード内の 5 項目は U+001F でつなぐ
// （Rust 側 `from_android` と対で決める）:
//   1 幅（px、10 進）  Display.getRealMetrics().widthPixels。現在の回転を反映した実ピクセル
//   2 高さ（px、10 進）heightPixels
//   3 倍率             DisplayMetrics.density
//   4 主画面           getDisplayId() == DEFAULT_DISPLAY なら 1、それ以外は 0
//   5 名前             Display.getName()。null は空文字。trim せず、可逆にエスケープする:
//                      バックスラッシュ -> `\\`、U+001E -> `\r`、U+001F -> `\s`（Rust 側 `unescape_name` で復号）
// 画面 0 枚は空文字。DisplayManager が無い・RuntimeException は null。
//
// 権限は不要。個体識別子（getUniqueId・DisplayAddress）は読まない。DisplayManager・Display は
// スレッド制約が無い。Activity は握らず ApplicationContext だけを使い、要求 id・保留状態も持たない
// ので、Activity 再生成で破棄すべきものは無い。使う API はすべて API 17 以降（minSdk 24）。
// 守る範囲: 失敗・空の区別。守らない範囲: 読み取り後の変化の反映（Rust 側の 1 秒 pull まで）。
final class MultiDisplayHelper {
    private static final char RS = '\u001E';
    private static final char SEP = '\u001F';

    private MultiDisplayHelper() {}

    // getRealMetrics は API 31 で deprecated。代替の Display.Mode.getPhysicalWidth は回転を反映せず、
    // WindowMetrics は画面ごとに window context が要るため、全 API 帯で動くこちらを使う。
    @SuppressWarnings("deprecation")
    static String read(Context context) {
        try {
            DisplayManager dm = (DisplayManager) context.getSystemService(Context.DISPLAY_SERVICE);
            if (dm == null) {
                return null;
            }
            StringBuilder sb = new StringBuilder();
            for (Display d : dm.getDisplays()) {
                DisplayMetrics m = new DisplayMetrics();
                d.getRealMetrics(m);
                String raw = d.getName();
                String name = raw == null ? "" : raw.replace("\\", "\\\\").replace("\u001E", "\\r").replace("\u001F", "\\s");
                if (sb.length() > 0) {
                    sb.append(RS);
                }
                sb.append(m.widthPixels).append(SEP).append(m.heightPixels).append(SEP)
                    .append(Float.toString(m.density)).append(SEP)
                    .append(d.getDisplayId() == Display.DEFAULT_DISPLAY ? '1' : '0').append(SEP)
                    .append(name);
            }
            return sb.toString();
        } catch (RuntimeException e) {
            return null;
        }
    }
}
