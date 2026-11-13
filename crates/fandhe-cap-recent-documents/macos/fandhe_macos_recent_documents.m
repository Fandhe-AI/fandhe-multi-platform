// Phase 11 の拡充ループ（親 #193）型サンプル「最近使った項目への登録」: macOS 実装（Issue #988）。
// `NSDocumentController` の `noteNewRecentDocumentURL:` で登録し、`recentDocumentURLs` で読み戻す。
// 権限は不要。一覧の消去（`clearRecentDocuments:`）は呼ばない。
//
// `crates/fandhe-egui-proxy/src/device/macos_recent_documents.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_recent_documents_register(const char *path) -> int32_t   登録して読み戻す
//   fandhe_macos_recent_documents_check(const char *path) -> int32_t      読み戻すだけ
//     path は NUL 終端の UTF-8 で、Rust 側（`ShellPath`）で検査・正規化済みの絶対パスだけが渡される。
//     ポインタは呼び出し中だけ読み、保持しない。NSDocumentController は main thread で呼ぶ必要が
//     あるため main thread から呼ぶこと（dispatch_sync は使わない）。
//     0 = 読み戻した一覧にパスがある
//     1 = 一覧にパスが無い（設定で無効・反映待ち・件数上限 0 等）
//     2 = main thread ではない
//     3 = 文字列変換の失敗・nil
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ログは出さない（パス・他の項目を含むため）。

#import <AppKit/AppKit.h>
#include <stdint.h>

static NSURL *fandhe_recent_url_from_path(const char *path) {
    if (path == NULL) {
        return nil;
    }
    NSString *s = [NSString stringWithUTF8String:path];
    if (s == nil) {
        return nil;
    }
    return [NSURL fileURLWithPath:s isDirectory:NO];
}

// 両側に同じ正規化（シンボリックリンク解決）を掛けて比べる。片側だけだと `/private` の有無でずれる。
static NSString *fandhe_recent_normalized(NSURL *url) {
    return [[url URLByResolvingSymlinksInPath] path];
}

// 読み戻した一覧に url があれば 0、無ければ 1、比較対象を作れなければ 3。
static int32_t fandhe_recent_listed(NSURL *url) {
    NSString *want = fandhe_recent_normalized(url);
    if (want == nil) {
        return 3;
    }
    for (NSURL *item in [[NSDocumentController sharedDocumentController] recentDocumentURLs]) {
        if (![item isFileURL]) {
            continue;
        }
        if ([fandhe_recent_normalized(item) isEqualToString:want]) {
            return 0;
        }
    }
    return 1;
}

static int32_t fandhe_recent_run(const char *path, bool do_register) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 2;
        }
        NSURL *url = fandhe_recent_url_from_path(path);
        if (url == nil) {
            return 3;
        }
        if (do_register) {
            [[NSDocumentController sharedDocumentController] noteNewRecentDocumentURL:url];
        }
        return fandhe_recent_listed(url);
    }
}

int32_t fandhe_macos_recent_documents_register(const char *path) {
    return fandhe_recent_run(path, true);
}

int32_t fandhe_macos_recent_documents_check(const char *path) {
    return fandhe_recent_run(path, false);
}
