// Phase 11（親 #858）型サンプル「クリップボード」: iOS 側の実実装（Issue #859）。
//
// `crates/fandhe-egui-proxy/src/device/ios.rs`（`IosClipboard`）から C ABI 経由で
// 呼ばれる。`UIPasteboard` は Rust バインディングを持たないため、この 1 ファイルの
// ObjC コードに実装を閉じ、`extern "C"` 宣言のみを Rust 側へ公開する
// （`fandhe_ios_motion.m` と同型、coding-rust.md の FFI 例外規定）。
//
// `build.rs` が iOS ターゲット（`CARGO_CFG_TARGET_OS == "ios"`）かつ feature `device`
// 有効時のみ `cc` でコンパイルする（他ターゲットのビルドには一切関与しない）。
//
// 契約（`ios.rs` のコメントと対）:
//   fandhe_ios_clipboard_read(&out_text, &out_len) -> 0 で成功。*out_text に UTF-8
//     バイト列（malloc 済み・末尾に NUL を置くが、内容自体に NUL を含み得る。所有権は
//     呼び出し側へ移り `fandhe_ios_clipboard_free` で解放する）を、*out_len にその
//     バイト数（終端 NUL を含まない）を書く。1 = クリップボードにテキストが無い
//     （out 引数は書き換えない）。2 = 内部エラー（UTF-8 変換・malloc 失敗。
//     out 引数は書き換えない）。
//
//     長さを別途返すのは、埋め込み NUL を含む文字列で `strlen` が途中までしか数えず、
//     Rust 側が黙って切り詰めた結果を「成功」として返していたため（codex-review 指摘
//     P1、PR #860）。長さを渡せば Rust 側が全バイトを検査でき、書き込み側
//     （`CString::new` が内部 NUL を弾く）と同じ判定へ揃えられる。
//   fandhe_ios_clipboard_write(text) -> 0 で成功。`text` は NUL 終端の UTF-8 で、
//     本関数はコピーを取るため呼び出し後に Rust 側が解放してよい。
//     2 = 内部エラー（NULL 引数・UTF-8 として解釈できない）。
//   いずれも呼び出しは egui の update ループ（iOS ではメインスレッド）からのみを
//   想定する（`UIPasteboard` は UIKit の main スレッド前提 API。スレッド検査は
//   呼び出し側 `ios.rs` の `is_main_thread` が担う）。
//
// iOS 16 以降、**他アプリがコピーした内容**を `UIPasteboard.general.string` で読むと
// OS の許可アラートが出る（拒否されると nil が返る）。自アプリがコピーした内容の
// 読み取りはアラート無しで通るため、型サンプルは「コピー → 貼り付け」の往復で
// ダイアログに阻まれずデモできる（`fandhe_core::screens::clipboard_text` のモジュール
// doc 参照）。読み取り前の `hasStrings` 判定は Apple が「アラートを出さない検出 API」
// として用意しているもので、テキスト不在の場合に無駄なアラートを出さずに済む。
//
// クリップボード内容はプロセス内で画面へ表示するだけで、ファイル保存・ネットワーク
// 送信・ログ出力は一切行わない（security.md A01/A09。クリップボードは他アプリの
// パスワード等が載り得る機密経路のため、内容を痕跡として残さない）。

#import <UIKit/UIKit.h>
#include <stdlib.h>
#include <string.h>

int32_t fandhe_ios_clipboard_read(char **out_text, size_t *out_len) {
    UIPasteboard *pasteboard = UIPasteboard.generalPasteboard;

    // `hasStrings` は許可アラートを出さない検出 API（ファイル冒頭コメント参照）。
    // テキストが無い場合はここで返し、`string` へ触れない。
    if (!pasteboard.hasStrings) {
        return 1;
    }

    NSString *text = pasteboard.string;
    if (text == nil) {
        // `hasStrings` が真でも、iOS 16 以降の許可アラートでユーザーが拒否すると
        // nil が返る。異常ではなく「読めなかった」ため不在と同じ扱いにする。
        return 1;
    }

    const char *utf8 = text.UTF8String;
    if (utf8 == NULL) {
        return 2;
    }

    // `strlen` ではなく実際の UTF-8 バイト数を使う（埋め込み NUL があると `strlen` は
    // そこで止まり、以降を黙って捨ててしまう。ファイル冒頭の契約コメント参照）。
    size_t len = [text lengthOfBytesUsingEncoding:NSUTF8StringEncoding];

    // 終端 NUL 分を足して複製し、所有権を呼び出し側へ渡す。
    char *copy = malloc(len + 1);
    if (copy == NULL) {
        return 2;
    }
    memcpy(copy, utf8, len);
    copy[len] = '\0';
    *out_text = copy;
    *out_len = len;
    return 0;
}

void fandhe_ios_clipboard_free(char *ptr) {
    // `free(NULL)` は no-op のため NULL 許容（`fandhe_ios_camera_free` と同じ契約）。
    free(ptr);
}

int32_t fandhe_ios_clipboard_write(const char *text) {
    if (text == NULL) {
        return 2;
    }
    NSString *value = [NSString stringWithUTF8String:text];
    if (value == nil) {
        // UTF-8 として解釈できないバイト列（Rust 側の `CString` 経由では起きないが、
        // 契約違反を検出した経路として安全側で返す）。
        return 2;
    }
    // `setString:` は内部でコピーを取るため、呼び出し後に `text` を解放してよい。
    UIPasteboard.generalPasteboard.string = value;
    return 0;
}
