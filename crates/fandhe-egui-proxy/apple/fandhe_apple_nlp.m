// Phase 11 の拡充ループ（親 #1093）型サンプル「単語分割」: macOS 実装（Issue #1094）と
// iOS 実装（Issue #1095）で共用する。`TARGET_OS` では分けない（NaturalLanguage と
// Foundation だけを使い、UIKit / AppKit に触れない）。
//
// `crates/fandhe-egui-proxy/src/device/apple_nlp.rs` から C ABI 経由で呼ばれる。
// `build.rs` が macOS・iOS ターゲットの feature `device` 有効時に `cc` コンパイルする
// （Rust 側 cfg と同じ OS 集合に保つ）。
//
// 契約（一発呼び出し型。結果を待つ要求は持たない）:
//   fandhe_apple_nlp_segment(const char *text, void *ctx,
//                            void (*on_token)(void *ctx, const char *token)) -> int32_t
//     text   : NUL 終端の UTF-8（NUL 混入・長さ上限の検証は Rust 側で済み）
//     on_token: 分割トークンを出現順に 1 件ずつ同期で呼ぶ。`ctx` と `token` は呼び出しの間だけ
//               有効。この関数は `ctx` / `token` を保持しない
//     0 = 成功（トークン 0 件も成功）
//     1 = 失敗（NSString を作れない、またはトークンを UTF-8 へ変換できない。呼び出し元が
//         `Internal` へ畳む）
//
// 空白・記号だけのトークンの除去と前後空白の trim は Rust 側の `normalize_words` が行う
// （契約の SSOT は `fandhe_device::nlp`）。言語は指定せず `NLTokenizer` の自動判定に任せる。
//
// スレッド: UI ではなく、呼び出しごとに `NLTokenizer` を作るため共有状態もロックも持たない。
// 呼ばれたスレッドで同期に実行する（`dispatch_sync` は使わない。main 側が呼び出し元を待つと
// デッドロックするため、`device-bridge-contracts.md`「11」）。Cocoa 呼び出しは
// `@autoreleasepool` の内側で行う。
//
// 入力文字列・トークンはログへ出さない（security.md A09）。

#import <Foundation/Foundation.h>
#import <NaturalLanguage/NaturalLanguage.h>

int32_t fandhe_apple_nlp_segment(const char *text, void *ctx,
                                 void (*on_token)(void *ctx, const char *token)) {
    @autoreleasepool {
        NSString *string = [NSString stringWithUTF8String:text];
        if (string == nil) {
            return 1;
        }
        NLTokenizer *tokenizer = [[NLTokenizer alloc] initWithUnit:NLTokenUnitWord];
        tokenizer.string = string;
        __block BOOL failed = NO;
        [tokenizer enumerateTokensInRange:NSMakeRange(0, string.length)
                               usingBlock:^(NSRange tokenRange, NLTokenizerAttributes __unused flags,
                                            BOOL *stop) {
            const char *utf8 = [[string substringWithRange:tokenRange] UTF8String];
            if (utf8 == NULL) {
                failed = YES;
                *stop = YES;
                return;
            }
            on_token(ctx, utf8);
        }];
        return failed ? 1 : 0;
    }
}
