// Phase 11 の拡充ループ（親 #1013）型サンプル「ドラッグ & ドロップ」の iOS 実装
// （Issue #1015、macOS/Windows 実装 #1014 の横展開）。受け取り側のみ（送り側は
// 親 issue の方針でスコープ外、`fandhe-device` 側 doc 参照）。
//
// `crates/fandhe-egui-proxy/src/device/ios_drag_drop.rs` から C ABI 経由で呼ばれる。
// `UIDropInteraction`/`NSItemProvider` は Rust バインディングを持たないため、この 1
// ファイルの ObjC コードに実装を閉じ、`unsafe extern "C"` 宣言 1 点
// （`fandhe_ios_drag_drop_install`）のみを Rust 側へ公開する（`lib.rs` の iOS
// `deny(unsafe_code)` 緩和と対になる、coding-rust.md の FFI 例外規定）。
//
// `build.rs` が iOS ターゲット・feature `device` 有効時のみ `cc` でコンパイルする。
//
// 契約（`ios_drag_drop.rs` のコメントと対）:
//   int32_t fandhe_ios_drag_drop_install(void)
//     -> 0 = winit の base view へ `UIDropInteraction` を取り付けられた
//        （既に取り付け済みなら何もせず 0 を返す、冪等）。
//        1 = base view を取得できなかった（rootViewController/view 不在）。
//        2 = main スレッド以外から呼ばれた（呼び出し元契約違反）。
//   fandhe_rust_drag_drop_hover(int32_t entered)
//     -> ドラッグがウィンドウ領域へ入った（1）/出た・終了した（0）。
//   fandhe_rust_drag_drop_begin(size_t total, uint64_t *out_session, size_t *out_to_load)
//     -> `total` 件のドロップを開始し、実際に読み込むべき件数（上限は Rust 側
//        `MAX_DROP_FILES` が SSOT、本ファイルには複製しない）を受け取る。
//   fandhe_rust_drag_drop_item(uint64_t session, size_t index, int32_t code,
//                               const uint8_t *path_ptr, size_t path_len,
//                               const uint8_t *name_ptr, size_t name_len)
//     -> 1 件ぶんの読み込み結果。code: 0=通常ファイル（path が一時ファイルの
//        絶対パス、UTF-8）、1=フォルダ、2=読めない。
//
// # ライフサイクル・登録トークン（契約表 1）
//
// `UIDropInteraction.delegate` は weak プロパティ（Apple ドキュメント）のため、
// `UIDocumentPickerViewController`/`UIAdaptivePresentationController` の delegate
// （`fandhe_ios_file_picker.m::g_retainedDelegate`）と同じく、delegate と
// interaction 本体の両方を static で強参照し続ける必要がある。プロセスの寿命の間
// 保持し続けることで、取り付け後に delegate が解放されて通知が届かなくなるのを
// 防ぐ。Activity 再生成に相当するものは iOS には無い（画面遷移で view が破棄
// されない winit ベースの単一ウィンドウ構成、`fandhe_ios_common.h` doc 参照）。
//
// # 複数アイテム・並行性・キャンセル（契約表 2・3）
//
// `performDrop:` は `fandhe_rust_drag_drop_begin` で世代（session id）を同期的に
// 確定させてから、先頭 `to_load` 件それぞれについて `loadFileRepresentation...`
// を呼ぶ。前回の `performDrop:` が返した `NSProgress` の配列を保持しておき、新しい
// ドロップ開始時に main 上で全件 `cancel` する（キャンセルが起動中の全ワーカーへ
// 伝わるようにする）。キャンセルされた completion は Rust 側で古いセッションの
// 完了として世代照合により無視される（`drop_session.rs` 参照）。
//
// # FFI 境界（契約表 5）
//
// completion（GCD の背景スレッド、`loadFileRepresentation...` の実行スレッドは
// 未規定）から Rust 側を呼ぶ本体は `@autoreleasepool` で囲む。UIKit の操作
// （install・`sessionDidUpdate:` の proposal 決定・`progress cancel`）は
// delegate が呼ばれるスレッド（UIKit が保証する main）でのみ行い、completion の
// 中では UIKit に一切触れない（Rust 側の集約器へ積むだけ、`ios_drag_drop.rs`
// モジュール doc 参照）。
//
// # 一時ファイル・情報非保持
//
// 自前の一時ファイル・コピーは作らない（`loadFileRepresentation...` が渡す URL は
// OS 管理の一時ファイルで、completion が return すると OS が削除する契約。
// `ios_drag_drop.rs` モジュール doc 参照）。ファイル名・パス・`NSError` の詳細を
// ログへ出力しない（security.md A09）。`NSLog` は使わない。

#import <UIKit/UIKit.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#import "fandhe_ios_common.h"

// Rust 側（`ios_drag_drop.rs`）が公開する `extern "C"` 関数群。
extern void fandhe_rust_drag_drop_hover(int32_t entered);
extern void fandhe_rust_drag_drop_begin(size_t total, uint64_t *out_session, size_t *out_to_load);
extern void fandhe_rust_drag_drop_item(uint64_t session, size_t index, int32_t code,
                                        const uint8_t *_Nullable path_ptr, size_t path_len,
                                        const uint8_t *_Nullable name_ptr, size_t name_len);
// ドラッグ & ドロップ画面が現在表示中か（`ios_drag_drop.rs::mark_active`/
// `fandhe_rust_drag_drop_is_active` doc 参照、codex 指摘 #1019）。`UIDropInteraction`
// はウィンドウ全体へ一度取り付けたら取り外さないため、画面の有効期間はこの問い合わせで
// 都度確認する（非表示中は 0）。
extern int32_t fandhe_rust_drag_drop_is_active(void);

// completion（背景スレッド）から呼ばれる、読み込み結果 1 件ぶんの報告。実装は
// ファイル末尾。`@implementation FandheDropDelegate`（`performDrop:`）より前方で
// 参照するため前方宣言する。
static void FandheReportDropItem(uint64_t sessionId, size_t index, NSURL *_Nullable url,
                                  NSString *_Nullable suggestedName);

// 取り付け済みの `UIDropInteraction` 本体への強参照（view は追加した interaction を
// 弱参照でしか保持しないため、これが無いと ARC が解放し得る）。main queue からのみ
// 読み書きする（install の冪等判定）。
static UIDropInteraction *_Nullable g_installedInteraction = nil;

// `UIDropInteraction.delegate` は weak プロパティ（モジュール doc「ライフサイクル・
// 登録トークン」参照）のため、delegate 自体を別途強参照する。main queue からのみ
// 読み書きする。
static id<UIDropInteractionDelegate> _Nullable g_retainedDelegate = nil;

// 直前の `performDrop:` が返した `NSProgress`（読み込み中のアイテムごとに 1 個）。
// 新しいドロップ開始時にキャンセルしてから差し替える（契約表 3）。main queue
// からのみ読み書きする。
static NSArray<NSProgress *> *g_activeProgresses = nil;

@interface FandheDropDelegate : NSObject <UIDropInteractionDelegate>
@end

@implementation FandheDropDelegate

// フォルダ・非ファイルも含め、1 件でもアイテムがあれば受け付ける。黙って
// 受け付けない（`canHandleSession:` で NO を返す）と、ユーザーには何も起きて
// いないように見えて失敗理由が伝わらない（契約表 6「嘘の成功」の逆: 黙った
// 失敗もここでは避ける）。実際の失敗判定は `performDrop:` 側で行う。
//
// ただし画面が非表示中（`fandhe_rust_drag_drop_is_active()` が 0）は NO を返す。
// `UIDropInteraction` はウィンドウ全体へ一度取り付けたきり取り外されないため
// （モジュール冒頭コメント「ライフサイクル・登録トークン」参照）、ここで断らないと
// 別画面表示中に受けたドロップが、後で本画面へ戻ったときの結果として誤って
// 処理される（codex 指摘 #1019、契約表 1）。`canHandleSession:` で NO を返せば
// UIKit はこのセッションに対して以降の delegate 呼び出し（`sessionDidEnter:`/
// `performDrop:` 等）を行わない契約（Apple ドキュメント）。
- (BOOL)dropInteraction:(UIDropInteraction *)interaction canHandleSession:(id<UIDropSession>)session {
    if (!fandhe_rust_drag_drop_is_active()) {
        return NO;
    }
    return session.items.count > 0;
}

- (UIDropProposal *)dropInteraction:(UIDropInteraction *)interaction
                    sessionDidUpdate:(id<UIDropSession>)session {
    return [[UIDropProposal alloc] initWithDropOperation:UIDropOperationCopy];
}

- (void)dropInteraction:(UIDropInteraction *)interaction sessionDidEnter:(id<UIDropSession>)session {
    fandhe_rust_drag_drop_hover(1);
}

- (void)dropInteraction:(UIDropInteraction *)interaction sessionDidExit:(id<UIDropSession>)session {
    fandhe_rust_drag_drop_hover(0);
}

- (void)dropInteraction:(UIDropInteraction *)interaction sessionDidEnd:(id<UIDropSession>)session {
    // `performDrop:` 経路（ドロップ成立）でも `sessionDidEnd:` は続けて呼ばれる。
    // Rust 側の `hover` は「領域内に入った/出た」の 2 値でしかなくホバー一覧の
    // 二重送信を起こさない（`drop_session.rs::DropSessions::hover` は単なる
    // push、`fandhe_core::screens::drag_drop::cancel_hover` が
    // 「既に hovering でなければ何もしない」を担保する）。
    fandhe_rust_drag_drop_hover(0);
}

- (void)dropInteraction:(UIDropInteraction *)interaction performDrop:(id<UIDropSession>)session {
    // `canHandleSession:` から `performDrop:` までの間に画面が非表示になった場合の
    // 保険（同一セッション内では `canHandleSession:` は 1 回しか呼ばれないため）。
    // 何もせず戻る＝このドロップは黙って捨てる（画面外イベントを破棄、契約表 1）。
    if (!fandhe_rust_drag_drop_is_active()) {
        return;
    }

    NSArray<UIDragItem *> *items = session.items;
    size_t total = items.count;

    uint64_t sessionId = 0;
    size_t toLoad = 0;
    fandhe_rust_drag_drop_begin(total, &sessionId, &toLoad);

    // 前回のドロップが読み込み中のまま新しいドロップが来た場合、古いワーカーへ
    // キャンセルを伝える（契約表 3）。古いセッション宛ての completion は Rust 側
    // 世代照合でどのみち無視されるが、実際の I/O・ネットワーク待ちを止められるなら
    // 止める。
    NSArray<NSProgress *> *previousProgresses = g_activeProgresses;
    for (NSProgress *progress in previousProgresses) {
        [progress cancel];
    }

    NSMutableArray<NSProgress *> *progresses = [NSMutableArray arrayWithCapacity:toLoad];

    for (size_t index = 0; index < toLoad; index++) {
        NSItemProvider *itemProvider = items[index].itemProvider;
        uint64_t capturedSession = sessionId;
        size_t capturedIndex = index;

        if ([itemProvider hasItemConformingToTypeIdentifier:UTTypeData.identifier]) {
            // `error` は使わない: 失敗理由の分類は `FandheReportDropItem`（`url == nil`
            // かどうか）だけで足りる（`NSError` の詳細をログへ出さない方針、security.md
            // A09 とも整合する）。
            // `suggestedName` はドロップ元が渡す本来のファイル名（`loadFileRepresentation...`
            // が生成する一時ファイルの名前とは別物）。取得できる場合はこちらを表示名として
            // 優先し、`url.lastPathComponent`（一時ファイル名）へは取得できないときのみ
            // フォールバックする。
            NSString *suggestedName = itemProvider.suggestedName;
            NSProgress *progress = [itemProvider
                loadFileRepresentationForTypeIdentifier:UTTypeData.identifier
                                       completionHandler:^(NSURL *_Nullable url, NSError *_Nullable error) {
                                         (void)error;
                                         @autoreleasepool {
                                           FandheReportDropItem(capturedSession, capturedIndex, url, suggestedName);
                                         }
                                       }];
            if (progress != nil) {
                [progresses addObject:progress];
            }
        } else if ([itemProvider hasItemConformingToTypeIdentifier:UTTypeDirectory.identifier]) {
            // フォルダは読み込みを試みず、その場で「フォルダ」として報告する
            // （`loadFileRepresentation...` はフォルダに対して呼べない）。
            fandhe_rust_drag_drop_item(capturedSession, capturedIndex, 1, NULL, 0, NULL, 0);
        } else {
            // 対応する型が無い（ファイルでもフォルダでもない、例: 文字列のみの
            // アイテム）。「読めない」として報告する（黙って無視しない、契約表 6）。
            fandhe_rust_drag_drop_item(capturedSession, capturedIndex, 2, NULL, 0, NULL, 0);
        }
    }

    g_activeProgresses = progresses;
}

@end

// completion（背景スレッド）から呼ばれる、読み込み結果 1 件ぶんの報告。
// `@autoreleasepool` の内側から呼ぶ契約（呼び出し元参照）。
static void FandheReportDropItem(uint64_t sessionId, size_t index, NSURL *_Nullable url,
                                  NSString *_Nullable suggestedName) {
    if (url == nil) {
        // `loadFileRepresentation...` がエラーを返した経路（読めない）。
        fandhe_rust_drag_drop_item(sessionId, index, 2, NULL, 0, NULL, 0);
        return;
    }

    NSString *path = url.path;
    NSData *pathData = [path dataUsingEncoding:NSUTF8StringEncoding];
    if (pathData == nil) {
        // パスが UTF-8 化できない（実務上ほぼ到達しない）場合は読めない扱い。
        fandhe_rust_drag_drop_item(sessionId, index, 2, NULL, 0, NULL, 0);
        return;
    }

    // ドロップ元本来のファイル名（`itemProvider.suggestedName`）を優先する
    // （codex 指摘、#1019）。取得できない場合のみ一時ファイル名へフォールバックする。
    NSString *displayName = suggestedName != nil ? suggestedName : url.lastPathComponent;
    NSData *nameData = [displayName dataUsingEncoding:NSUTF8StringEncoding];
    const uint8_t *namePtr = nameData != nil ? nameData.bytes : NULL;
    size_t nameLen = nameData != nil ? nameData.length : 0;

    // `read_dropped`（Rust 側、`ios_drag_drop.rs::build_item`）がこの呼び出しの
    // 中で読み取り・8 MiB 判定まで同期に終える契約（本ファイル冒頭コメント参照）。
    fandhe_rust_drag_drop_item(sessionId, index, 0, (const uint8_t *)pathData.bytes, pathData.length,
                                namePtr, nameLen);
}

int32_t fandhe_ios_drag_drop_install(void) {
    if (!NSThread.isMainThread) {
        return 2;
    }
    if (g_installedInteraction != nil) {
        return 0;
    }

    // `FandheTopmostViewController()` は「一番深く presented された」VC を返すが、
    // ドロップ領域はウィンドウ全体（winit の base view）に固定したいため、そこから
    // `view.window.rootViewController.view` を辿って base view へ正規化する
    // （実装計画 §2.4）。取り付け時点でモーダルが被っていても、以後そのモーダルが
    // 閉じられれば正しく base view 上のドロップとして機能する。
    UIViewController *topmost = FandheTopmostViewController();
    UIView *baseView = topmost.view.window.rootViewController.view;
    if (baseView == nil) {
        return 1;
    }

    FandheDropDelegate *delegate = [[FandheDropDelegate alloc] init];
    UIDropInteraction *interaction = [[UIDropInteraction alloc] initWithDelegate:delegate];
    [baseView addInteraction:interaction];
    // `delegate` は weak プロパティ経由でしか interaction から参照されないため
    // （モジュール doc「ライフサイクル・登録トークン」参照）、ここで強参照する。
    g_retainedDelegate = delegate;
    g_installedInteraction = interaction;
    return 0;
}
