// Phase 11 の拡充ループ（親 #911）型サンプル「文字サイズ追従」: iOS 実装（Issue #914）。
// macOS は一次情報調査の結果、第三者アプリ向け公開 API が無いため恒久的に
// `UnsupportedTextScale`（`fandhe_device::text_scale` モジュール doc・PR #920 参照）で、
// 本ファイルは `TARGET_OS_IPHONE` に閉じる（`apple/` 配下だが iOS 専用。ファイル配置は
// `fandhe_apple_biometric.m`/`fandhe_apple_location.m` と揃え、`build.rs` の
// `compile_apple_*` 群に相乗りできる形にしておく想定。実際の呼び出しは iOS 分岐からのみ）。
//
// `crates/fandhe-egui-proxy/src/device/ios.rs`（`IosTextScale`）から C ABI 経由で
// 呼ばれる。`build.rs` が iOS ターゲット・feature `device` 有効時のみ `cc` コンパイル
// する（macOS 分岐からは呼ばない。UIKit は macOS ではリンクされていないため）。
//
// 契約（`ios.rs` のコメントと対、`fandhe_ios_motion.m` と同型の pull・同期 out 引数）:
//   fandhe_apple_text_scale_read(float *out_scale) -> int32_t
//     0 = 成功。*out_scale に倍率（既定 Large の本文サイズ基準、doc 下記参照）を書く。
//     1 = 異常（`pointSize` が 0 以下等）で out 引数は書き換えない。
//
// 倍率は `UIFont.preferredFontForTextStyle:UIFontTextStyleBody` の `pointSize` を
// 既定（Large、`pointSize == 17.0`）の本文サイズで割った比で求める
// （Issue #914 本文の一次情報: Apple Human Interface Guidelines の Dynamic Type
// 既定値）。`UIContentSizeCategory` の生カテゴリ文字列を Rust 側へ渡さず、この 1 つの
// `float` のみを返す（`fandhe_device::text_scale::TextScale` の契約に合わせる、
// 情報最小化 security.md A09）。
//
// 呼び出しは main スレッド専用: `preferredFontForTextStyle:` は `UIContentSizeCategory`
// を `UITraitCollection.current`（main スレッドの trait environment）経由で解決する
// UIKit API のため、`ios.rs::IosTextScale::scale_factor` が `is_main_thread()`
// （`IosMotion`/`IosClipboard` と同じガード）で FFI 呼び出し前に検査する契約とする。
//
// 現在稼働中の文字サイズ変更（`UIContentSizeCategoryDidChangeNotification`）への追従は
// 行わない: 本 Issue の範囲では画面状態は一回読みのみで、読み直し API を core に
// 追加していない（実装計画 §7、天井は `docs/device/capabilities/dynamic-type.json`
// の `note` に記録）。
//
// ログ出力・保存は一切行わない（security.md A09）。

#import <TargetConditionals.h>

#if TARGET_OS_IPHONE

#import <UIKit/UIKit.h>

// 既定（Large）の本文サイズ。倍率の基準値（ファイル冒頭コメント参照）。
static const float kFandheDefaultBodyPointSize = 17.0f;

int32_t fandhe_apple_text_scale_read(float *out_scale) {
    UIFont *font = [UIFont preferredFontForTextStyle:UIFontTextStyleBody];
    float pointSize = (float)font.pointSize;
    if (pointSize <= 0.0f) {
        return 1;
    }

    *out_scale = pointSize / kFandheDefaultBodyPointSize;
    return 0;
}

#endif // TARGET_OS_IPHONE
