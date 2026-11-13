package com.fandhe.platform;

import android.content.Context;

// 文字サイズ追従（Issue #914）の Android 実装ヘルパー。Rust の `fandhe-cap-dynamic-type/src/android.rs` が
// static メソッド `textScaleFactor(Context)` を JNI（署名 `(Landroid/content/Context;)F`）で同期に呼ぶ。
// `Configuration.fontScale` は「システム標準の何倍か」を表す倍率で、Rust 側の `TextScale` 契約と一致する。
// 値の丸め（NaN・範囲外）は Rust 側 `read_scale` が行う。
//
// `AndroidManifest.xml` の `configChanges` に `fontScale` を含めているため、文字サイズ変更で Activity は
// 再生成されない（Issue #73）。Rust 側は未読のとき 1 回読むだけで、稼働中の変更には追従しない
// （天井は `docs/device/capabilities/dynamic-type.json` の `note`）。
// コールバック・要求 id・保留状態を持たないため、Activity 再生成で破棄すべき要求は無い。
final class TextScaleHelper {
    private TextScaleHelper() {}

    static float textScaleFactor(Context context) {
        return context.getResources().getConfiguration().fontScale;
    }
}
