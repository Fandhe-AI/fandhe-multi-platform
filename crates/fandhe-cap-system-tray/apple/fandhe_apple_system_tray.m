// Phase 11 の拡充ループ（親 #1038）型サンプル「システムトレイ」: macOS 向け実装
// （Issue #1039）。`NSStatusBar`/`NSStatusItem`/`NSMenu` を使う。iOS には
// 相当する UI が無いため macOS 専用（対応表 `docs/device/capabilities/
// system-tray.json` で iOS は `n-a`）。
//
// `crates/fandhe-egui-proxy/src/device/apple_system_tray.rs` から C ABI 経由で
// 呼ばれる。`build.rs::build_macos` がコンパイルし、追加リンクは無い（AppKit は
// `compile_apple_share(true)` が既にリンク済み）。
//
// 契約（同期呼び出し。`fandhe_ios_file_picker.m` のような completion コールバック
// 型ではなく、`apple_system_tray.rs` が呼んで戻り値を待つ）:
//   fandhe_apple_system_tray_show(uint64_t generation, const char *title,
//       const char *const *items, int32_t item_count) -> int32_t
//     0 = 成功（アイコン + メニューを生成した）。
//     2 = 生成失敗（NSStatusItem/NSString の生成失敗。実運用ではまず起きない
//         防御的分岐）。
//     3 = 取り消し済み（表示処理が main 待ちの間に終了処理の hide が先に走った。
//         項目は作らない。`g_cancel_pending_show` 参照）。
//     二重表示の判定（`AlreadyShown`）は Rust 側（`SelectionQueue::is_active`）
//     で完結させるため、本関数は常に「新規表示」のみを担当する（`.m` 側は
//     現在の表示状態を保持しない）。
//   fandhe_apple_system_tray_hide(void) -> int32_t
//     常に 0（失敗経路が無い）。
//   fandhe_rust_system_tray_selected(uint64_t generation, int64_t index)
//     -> Rust 側（`apple_system_tray.rs`）が公開する `extern "C"` 関数。
//        メニュー項目選択時に呼ぶ。
//
// 呼び出しスレッド（P1 是正、PR #1041 codex 指摘、Issue #1039）:
//   AppKit 呼び出しは main thread 必須。呼び出し元（`apple_system_tray.rs`）は
//   `fandhe-egui-proxy::device::system_tray::drive` が起こす作業スレッドから
//   呼ぶため main でないことがある。本体を block（`FandheRunOnMainAndWait`）に
//   包み、呼び出しスレッドが main ならそのまま実行、main でなければ
//   `dispatch_sync(dispatch_get_main_queue(), block)` で main へ寄せて実行する。
//   `dispatch_sync` は main thread の run loop（`NSApplication` のイベントループ、
//   eframe が main thread で回す）が処理するため、作業スレッドはブロックしても
//   main 側の処理完了で解放される（デッドロックは main 自身が呼ぶ場合のみで、
//   その経路は `isMainThread` 分岐で避ける）。
//
// タイトル・メニュー項目はログへ出力しない（security.md A09。本実装は固定文言
// のみを扱うため実害は無いが、`notification.rs` と同じ方針を踏襲する）。
//
// ponytail: 画像アイコン（SF Symbol 等）ではなくテキストタイトルのみを使う。
// アセット無しで動く最小実装のため。アイコン化が要件化したら
// `button.image`/`button.template` を追加する。

#import <Cocoa/Cocoa.h>
#import <dispatch/dispatch.h>

// Rust 側（`apple_system_tray.rs`）が公開する `extern "C"` 関数。
extern void fandhe_rust_system_tray_selected(uint64_t generation, int64_t index);

// プロセス内で 1 個だけ保持する status item / menu / target
// （`fandhe_apple_location.m` の `g_location_manager` と同じ static 保持パターン。
// `g_target` は `NSMenuItem.target` が weak 参照のため、ARC による即解放を防ぐ
// 目的で強参照を static に持つ、`device-bridge-contracts.md`「1. ライフサイクル
// 境界」の「登録トークンを握り続けているか」対策）。
static NSStatusItem *g_status_item = nil;
static NSMenu *g_menu = nil;

@interface FandheTrayTarget : NSObject
@property(nonatomic, assign) uint64_t generation;
- (void)onSelect:(id)sender;
@end

@implementation FandheTrayTarget

- (void)onSelect:(id)sender {
    if (![sender isKindOfClass:[NSMenuItem class]]) {
        return;
    }
    NSMenuItem *item = (NSMenuItem *)sender;
    fandhe_rust_system_tray_selected(self.generation, (int64_t)item.tag);
}

@end

static FandheTrayTarget *g_target = nil;

// 項目がまだ無いのに hide が来た（= 作業スレッドの show が main 待ちの間にアプリ
// 終了処理の hide が先に走った）ことを示す印。main thread だけが読み書きするため
// ロックは要らない。直後に届く show はこの印を見て項目を作らずに戻る（終了時に
// メニューバーへ項目を残さない。PR #1041 codex 指摘。Rust 側 `QUEUE` が active で
// 項目が無いのは表示処理の途中だけ）。
static BOOL g_cancel_pending_show = NO;

static void FandheRemoveStatusItemIfAny(void) {
    if (g_status_item != nil) {
        [[NSStatusBar systemStatusBar] removeStatusItem:g_status_item];
    }
    g_status_item = nil;
    g_menu = nil;
}

// 呼び出しスレッドが main ならそのまま `body` を実行し、main でなければ
// `dispatch_sync` で main へ寄せて実行する（`.m` ファイル冒頭コメント
// 「呼び出しスレッド」節参照）。`dispatch_sync` は呼び出しスレッドを `body` が
// 戻るまで block するため、同期呼び出しの契約（戻り値を待つ）は保たれる。
static void FandheRunOnMainAndWait(void (^body)(void)) {
    if ([NSThread isMainThread]) {
        body();
    } else {
        dispatch_sync(dispatch_get_main_queue(), body);
    }
}

int32_t fandhe_apple_system_tray_show(uint64_t generation, const char *title,
                                       const char *const *items, int32_t item_count) {
    __block int32_t result = 0;
    FandheRunOnMainAndWait(^{
        if (g_cancel_pending_show) {
            g_cancel_pending_show = NO;
            result = 3;
            return;
        }
        @autoreleasepool {
            NSString *titleString = [NSString stringWithUTF8String:title];
            if (titleString == nil) {
                result = 2;
                return;
            }

            NSStatusItem *statusItem =
                [[NSStatusBar systemStatusBar] statusItemWithLength:NSVariableStatusItemLength];
            if (statusItem == nil || statusItem.button == nil) {
                // レビュー指摘: `statusItem.button` が nil の場合、`statusItem` 自体は
                // 生成済み（`g_status_item` には未保持）のまま return していたため、
                // Rust 側が失敗として `QUEUE.end()` した後も項目がメニューバーに
                // 残留していた（`device-bridge-contracts.md`「9. 失敗時のリソース
                // 解放」）。生成済みなら解放してから失敗を返す。
                if (statusItem != nil) {
                    [[NSStatusBar systemStatusBar] removeStatusItem:statusItem];
                }
                result = 2;
                return;
            }
            statusItem.button.title = titleString;
            statusItem.button.toolTip = titleString;

            NSMenu *menu = [[NSMenu alloc] initWithTitle:titleString];
            if (g_target == nil) {
                g_target = [[FandheTrayTarget alloc] init];
            }
            g_target.generation = generation;

            for (int32_t i = 0; i < item_count; i++) {
                NSString *label = [NSString stringWithUTF8String:items[i]];
                if (label == nil) {
                    continue;
                }
                NSMenuItem *menuItem = [[NSMenuItem alloc] initWithTitle:label
                                                                   action:@selector(onSelect:)
                                                            keyEquivalent:@""];
                menuItem.tag = i;
                menuItem.target = g_target;
                [menu addItem:menuItem];
            }
            statusItem.menu = menu;

            // 新規表示の直前に前回分が残っていれば解放する（本来は Rust 側の
            // `SelectionQueue::is_active` が二重表示を弾くため到達しないが、
            // 防御的にリーク（`device-bridge-contracts.md`「9. 失敗時のリソース
            // 解放」）を避ける）。
            FandheRemoveStatusItemIfAny();
            g_status_item = statusItem;
            g_menu = menu;
            result = 0;
        }
    });
    return result;
}

int32_t fandhe_apple_system_tray_hide(void) {
    __block int32_t result = 0;
    FandheRunOnMainAndWait(^{
        @autoreleasepool {
            if (g_status_item == nil) {
                g_cancel_pending_show = YES;
            }
            FandheRemoveStatusItemIfAny();
        }
        result = 0;
    });
    return result;
}
