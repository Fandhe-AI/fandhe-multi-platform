// Phase 11（端末機能の拡張ループ、親 #871）: iOS 側の実ファイル選択実装（Issue #873、案 B）。
//
// `crates/fandhe-egui-proxy/src/device/ios.rs`（`present_file_picker`/
// `fandhe_rust_file_picker_outcome`）から C ABI 経由で呼ばれる。
// `UIDocumentPickerViewController` は Rust バインディングを持たないため、この 1
// ファイルの ObjC コードに実装を閉じ、`unsafe extern "C"` 宣言 1 点のみを Rust
// 側へ公開する（`lib.rs` の iOS `deny(unsafe_code)` 緩和と対になる、
// coding-rust.md の FFI 例外規定）。
//
// `build.rs` が iOS ターゲット（`CARGO_CFG_TARGET_OS == "ios"`）かつ feature `device`
// 有効時のみ `cc` でコンパイルし、`fandhe_ios_file_picker` という静的ライブラリへ
// リンクする（他ターゲットのビルドには一切関与しない）。
//
// 案 B（旧実装からの変更点、codex-review 指摘の再発防止は次の構造で解消する）:
//   読み取りは Rust 側 `read_picked` に一本化し、本ファイルはパスを渡すだけ
//   （EOF・短い読み込みの取り違えを構造上排除）。提示成否は presentViewController
//   直前の判定のみで決め、completion に依存しない。スワイプ dismiss は
//   `presentationControllerDidDismiss:` で検知する（変更なし）。
//
// 契約（`ios.rs` のコメントと対）:
//   fandhe_ios_file_picker_pick(requestId) -> 0 で「提示を依頼できた」。
//   実際の選択結果は非同期に fandhe_rust_file_picker_outcome(requestId, code, ...)
//   経由で届く（本関数内でブロックしない。`FilePicker::pick` トレイトを通らない
//   案 B では、Rust 側も待たない。固定タイムアウトの安全網も持たない）。
//   iOS は常に 0 を返し、提示失敗（top view controller 不在等）も code 2 で
//   非同期に通知する（同期 false は Android の JNI 失敗専用。iOS 側に到達不能な
//   同期失敗経路は残さない）。
//
//   fandhe_rust_file_picker_outcome の code:
//     0 = 選択（path_ptr/path_len が tmp コピーの絶対パス、UTF-8）
//     1 = ユーザーがキャンセル（失敗ではない）
//     2 = 内部エラー（提示失敗・URL 不在・パスの UTF-8 変換失敗等）
//   0 以外では path_ptr を参照しない（NULL を渡す）。
//
// UIKit は main スレッド必須のため、提示は `dispatch_async(main)` で行う。
// `fandhe_ios_file_picker_pick` 自体は呼び出し元（`ios.rs::present_file_picker`）を
// ブロックしない。選択時の読み取り（Rust 側 `read_picked`）は dispatch_async(global)
// の別スレッドへ逃がし、main を長時間塞がない。
//
// 進行中フラグ（旧 `g_pick_in_flight`）は持たない: 単一飛行はコア
// （`fandhe_core::screens::file_pick`）がボタンを隠すことで保証済み
// （`device/file_pick.rs` モジュール doc 参照）。

#import <UIKit/UIKit.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#import "fandhe_ios_common.h"

// 提示中の picker.delegate（weak）を生かしておくための強参照（ARC 管理）。
// フラグではなくオブジェクト保持のみ（単一飛行の防止はコア側が担う、上記コメント参照）。
// main queue からのみ読み書きする（UIKit delegate 契約）。
static id _Nullable g_retainedDelegate = nil;

// `ios.rs::fandhe_rust_file_picker_outcome` の宣言（本ファイルの唯一の呼び出し先）。
extern void fandhe_rust_file_picker_outcome(uint64_t requestId, int32_t code,
                                             const uint8_t *_Nullable path_ptr, size_t path_len);

// `FandheTopmostViewController` は `fandhe_ios_common.h` へ移した（Issue #899 で
// 3 箇所目の利用（共有シート）が出たため。`fandhe_ios_camera.m` と同型の複製だった。

// `UIDocumentPickerDelegate` を実装し、結果を Rust へ橋渡しする。
// `UIAdaptivePresentationControllerDelegate` も併せて実装する:
// `UIDocumentPickerViewController` は既定で page-sheet スタイルで提示され、
// ユーザーがスワイプで閉じられるが、その操作では `documentPickerWasCancelled:`
// が呼ばれないため、`presentationController.delegate` 経由でスワイプ dismiss も
// 検知し、キャンセル同様に扱う。
@interface FandheFilePickerDelegate : NSObject <UIDocumentPickerDelegate, UIAdaptivePresentationControllerDelegate>
@property(nonatomic, assign) uint64_t requestId;
@end

@implementation FandheFilePickerDelegate

- (void)documentPicker:(UIDocumentPickerViewController *)controller
    didPickDocumentsAtURLs:(NSArray<NSURL *> *)urls {
    // programmatic dismiss（この成功パス自体が picker を閉じる遷移を引き起こす）で
    // `presentationControllerDidDismiss:` が後から届き、成功済みの結果をキャンセルへ
    // 化けさせる競合を遮断する（改訂 2、Cursor Medium 指摘）。先に delegate を外す。
    controller.presentationController.delegate = nil;

    uint64_t requestId = self.requestId;
    NSURL *url = urls.firstObject;
    // この時点でこの試行は終了（以降の delegate 呼び出しは来ない契約）。
    // circular reference（delegate ⇄ picker.delegate）を断つため先に解放する。
    // 以降 `self` に触れないこと（最後の強参照が落ちる）。
    g_retainedDelegate = nil;

    if (url == nil) {
        // 契約違反（選択イベントなのに URL が空）を検出した経路。
        fandhe_rust_file_picker_outcome(requestId, 2, NULL, 0);
        return;
    }

    // `asCopy:YES` で得た URL はアプリのサンドボックス内 tmp のコピーであり、
    // security-scoped でないことが多いため戻り値 NO を失敗扱いにしない
    // （iCloud 未ダウンロード分の解決は picker 側が済ませてから delegate を
    // 呼ぶ契約）。
    BOOL accessing = [url startAccessingSecurityScopedResource];
    NSString *path = url.path;
    NSData *pathData = [path dataUsingEncoding:NSUTF8StringEncoding];

    dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0), ^{
      @autoreleasepool {
        if (pathData == nil) {
            // ファイルパスが UTF-8 化できない（実務上ほぼ到達しない）場合は内部エラー扱い。
            fandhe_rust_file_picker_outcome(requestId, 2, NULL, 0);
        } else {
            // `read_picked`（Rust 側、`ios.rs::fandhe_rust_file_picker_outcome`）が
            // この呼び出しの中で読み取り・8 MiB 判定まで同期に終える契約
            // （本ファイル冒頭コメント参照）。戻ってから tmp コピーを削除する。
            fandhe_rust_file_picker_outcome(requestId, 0, (const uint8_t *)pathData.bytes,
                                             pathData.length);
        }
        if (accessing) {
            [url stopAccessingSecurityScopedResource];
        }
        [NSFileManager.defaultManager removeItemAtURL:url error:nil];
      }
    });
}

- (void)documentPickerWasCancelled:(UIDocumentPickerViewController *)controller {
    uint64_t requestId = self.requestId;
    // 以降 `self` に触れないこと（最後の強参照が落ちる）。
    g_retainedDelegate = nil;
    fandhe_rust_file_picker_outcome(requestId, 1, NULL, 0);
}

// スワイプ（インタラクティブ）dismiss の検知。`documentPicker:didPickDocumentsAtURLs:`
// / `documentPickerWasCancelled:` と重複して呼ばれ得るが、同一性判定は ObjC 側で
// 行わない（仕様書「ObjC 側で同一性判定しない」）。重複コールバックの畳み込みは
// Rust 側 `bridge::deliver`（request_id 不一致・受け皿消費済みは無視）に一任する。
- (void)presentationControllerDidDismiss:(UIPresentationController *)presentationController {
    uint64_t requestId = self.requestId;
    // 以降 `self` に触れないこと（最後の強参照が落ちる）。
    g_retainedDelegate = nil;
    fandhe_rust_file_picker_outcome(requestId, 1, NULL, 0);
}

@end

int32_t fandhe_ios_file_picker_pick(uint64_t requestId) {
    dispatch_async(dispatch_get_main_queue(), ^{
      @autoreleasepool {
        UIViewController *top = FandheTopmostViewController();
        // `top.view.window == nil` は `presentViewController` がデリゲートを
        // 一切呼ばずに失敗する既知の失敗系（`fandhe_ios_camera.m` と同じ）。
        // 提示成否の判定は completion に頼らず、ここで一度だけ決める（案 B）。
        if (top == nil || top.view.window == nil) {
            fandhe_rust_file_picker_outcome(requestId, 2, NULL, 0);
            return;
        }

        // `UTTypeItem` は「あらゆる種類のファイル」を表す最上位の UTType だが、
        // フォルダ（`public.folder`）もこれに適合するため、`UTTypeItem` を渡すと
        // ディレクトリまで選択可能になってしまう。`UTTypeData`（`public.data`）は
        // フォルダが適合しない一方で通常のファイルはほぼすべて適合するため、
        // これを使い「ファイルのみ」に絞る（iOS 14+ 推奨 API）。
        NSArray<UTType *> *contentTypes = @[ UTTypeData ];
        UIDocumentPickerViewController *picker =
            [[UIDocumentPickerViewController alloc] initForOpeningContentTypes:contentTypes
                                                                          asCopy:YES];
        picker.allowsMultipleSelection = NO;

        FandheFilePickerDelegate *delegate = [[FandheFilePickerDelegate alloc] init];
        delegate.requestId = requestId;
        // `picker.delegate` は弱参照のため、明示的に保持する
        // （`fandhe_ios_camera.m` の `retainedDelegate` と同じ理由）。
        picker.delegate = delegate;
        // page-sheet 提示のスワイプ dismiss は `documentPickerWasCancelled:` を
        // 経由しないため、`presentationController` 側にも同じ delegate を登録する
        // （上記 interface コメント参照）。
        picker.presentationController.delegate = delegate;
        g_retainedDelegate = delegate;

        // ponytail: 提示直前の判定のみで成否を決め、`presentViewController:` の
        // completion やモーダル遷移中の再判定は持たない（旧実装より単純化。
        // 既知の天井: 別モーダルの提示/解除と競合した稀なケースでは提示が
        // 失敗してもコールバックが来ず `Picking` のまま残り得る。固定タイムアウトは
        // 掛けない方針のため、`Picking` のまま留まる — `fandhe-web-dom::file_picker`
        // の `cancel` 非対応ブラウザと同じ割り切り）。実機で恒久的な滞留の報告が
        // 出たら見直す。
        [top presentViewController:picker animated:YES completion:nil];
      }
    });

    return 0;
}
