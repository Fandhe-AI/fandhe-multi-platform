package com.fandhe.androidapp;

import android.graphics.fonts.Font;
import android.graphics.fonts.SystemFonts;
import android.os.Build;

import java.nio.ByteBuffer;
import java.util.Set;

// OS 搭載フォント一覧画面（Issue #1378、親 #1145）の Android 実装ヘルパー。MainActivity の
// `localFontsList()` から委譲される（Rust の `device/android_local_fonts.rs` が egui スレッドで
// ボタン押下ごとに同期で 1 回 JNI 呼び出しする。シグネチャ `()[Ljava/lang/Object;`）。
//
// `Font` に名前を返す公開 API は無いため、各 `Font` の `getBuffer()`（direct・read-only の mmap）と
// `getTtcIndex()` を渡し、Rust が `name` テーブルからファミリー名を読む。
//
// 戻り値（Rust 側 `android_local_fonts.rs` と対で決める契約）:
//   長さ 0 の配列 = API 29 未満（Rust は `Unsupported`）
//   長さ 2 の配列 = { ByteBuffer[] buffers, int[] ttcIndices }（同じ長さ。0 件なら長さ 0 の配列 2 本）
//   null = 失敗（Rust は `Internal`）
// `getBuffer()` が null のフォントは null 要素として渡し、Rust が捨てる（件数には数える）。
// 成功時は必ず長さ 2 なので、長さ 0 の番兵値とは衝突しない。
//
// 取らないもの: ファイルパス（`getFile()`）、書体の情報。バイト列は保持せず、Context / Activity も
// 握らない（`SystemFonts` は static API）。要求 id や保留状態を持たないので、Activity 再生成時に
// 破棄するものは無い。権限は不要。名前・例外メッセージはログに出さない。
final class LocalFontsHelper {
    private LocalFontsHelper() {}

    static Object[] list() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return new Object[0];
        }
        try {
            return listQ();
        } catch (RuntimeException e) {
            return null;
        }
    }

    @androidx.annotation.RequiresApi(Build.VERSION_CODES.Q)
    private static Object[] listQ() {
        Set<Font> fonts = SystemFonts.getAvailableFonts();
        ByteBuffer[] buffers = new ByteBuffer[fonts.size()];
        int[] ttc = new int[fonts.size()];
        int i = 0;
        for (Font f : fonts) {
            buffers[i] = f.getBuffer();
            ttc[i] = f.getTtcIndex();
            i++;
        }
        return new Object[] {buffers, ttc};
    }
}
