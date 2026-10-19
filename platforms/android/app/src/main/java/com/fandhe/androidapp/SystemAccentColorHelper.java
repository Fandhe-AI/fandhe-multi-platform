package com.fandhe.androidapp;

import android.content.Context;
import android.os.Build;

// システムのアクセント色（Issue #422）の Android 実装ヘルパー。
// Rust の `device/android_system_accent_color.rs` が `MainActivity.systemAccentColorRead` 経由で
// 1 秒間隔に同期で呼ぶ（JNI 署名 `()I`）。
// コールバック登録・要求 id・保留状態を持たないため、Activity 再生成で破棄すべき要求は無い。
// 権限は不要。色の値はログへ出さない。
// 定数の値は Rust の `fandhe_device::system_accent_color::rgb_from_code` と一致させること。
//   0..=0xFFFFFF: 色（0xRRGGBB）、-1: 失敗、-2: 非対応（API 30 以下）。
final class SystemAccentColorHelper {
    static final int FAILED = -1;
    static final int UNSUPPORTED = -2;

    private SystemAccentColorHelper() {}

    static int read(Context context) {
        // minSdk 24 のため、API 31 の定数参照は必ずこの分岐の内側に置く。
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            return UNSUPPORTED;
        }
        try {
            return context.getColor(android.R.color.system_accent1_500) & 0xFFFFFF;
        } catch (RuntimeException e) {
            return FAILED;
        }
    }
}
