// Phase 11 の拡充ループ（親 #150）型サンプル「グローバルホットキー」: macOS 実装（Issue #766）。
// Carbon の `RegisterEventHotKey` で Control + Shift + Option + K を排他登録する。アクセシビリティ・
// 入力監視の許可は不要（`NSEvent` の global monitor は許可が要るため採らない）。
//
// `crates/fandhe-egui-proxy/src/device/macos_global_hotkey.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルし、Carbon をリンクする。
//
// 契約:
//   fandhe_macos_global_hotkey_register(uint32_t token) -> int32_t
//     main thread でのみ呼ぶ。それ以外は 1（番兵、`MACOS_NOT_MAIN_THREAD`）を返す。
//     登録中なら OS を呼ばず 0 を返し、押下通知の token だけ差し替える。
//     未登録なら `kEventHotKeyExclusive` 付きで登録する。戻り値は OSStatus そのまま
//     （0 = 登録できた、-9878 = 他アプリの排他登録と重複）。成功時だけ EventHotKeyRef を保持する。
//     イベントハンドラの導入に失敗したら、その OSStatus を返し何も残さない。
//   fandhe_macos_global_hotkey_unregister(void) -> int32_t
//     main thread でのみ呼ぶ（それ以外は 1）。未登録は 2（番兵、`MACOS_NOT_REGISTERED`）。
//     `UnregisterEventHotKey` が noErr のときだけ EventHotKeyRef を手放す。失敗なら保持したまま
//     その OSStatus を返す（登録中のまま再試行できる）。
//   Rust から受ける: void fandhe_rust_global_hotkey_fire(uint32_t token)
//     押下のたびに main thread で呼ぶ。登録中（EventHotKeyRef あり）かつ ID が一致するときだけ。
//
// イベントハンドラは一度導入したらプロセス終了まで保持する（終了時は OS が片付ける）。登録の記録には
// 使わず、記録は EventHotKeyRef 1 つだけ。押されたキーの内容・OS のコードはログに出さない。
// FFI 規約: 各関数を `@autoreleasepool` で囲む。static は main thread だけで触るためロックは持たない。

#import <Carbon/Carbon.h>
#import <Foundation/Foundation.h>
#include <stdint.h>

extern void fandhe_rust_global_hotkey_fire(uint32_t token);

static const OSType kFandheHotKeySignature = 'FGHK';
static EventHotKeyRef g_hotkey_ref = NULL;
static uint32_t g_token = 0;
static EventHandlerRef g_handler_ref = NULL;

static OSStatus fandhe_hotkey_handler(EventHandlerCallRef next, EventRef event, void *user) {
    (void)next;
    (void)user;
    @autoreleasepool {
        EventHotKeyID id;
        OSStatus status = GetEventParameter(event, kEventParamDirectObject, typeEventHotKeyID, NULL,
                                            sizeof(id), NULL, &id);
        if (status != noErr) {
            return eventNotHandledErr;
        }
        if (g_hotkey_ref == NULL || id.signature != kFandheHotKeySignature || id.id != g_token) {
            return eventNotHandledErr;
        }
        fandhe_rust_global_hotkey_fire(g_token);
        return noErr;
    }
}

int32_t fandhe_macos_global_hotkey_register(uint32_t token) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 1;
        }
        if (g_hotkey_ref != NULL) {
            g_token = token;
            return 0;
        }
        if (g_handler_ref == NULL) {
            EventTypeSpec spec = {kEventClassKeyboard, kEventHotKeyPressed};
            OSStatus status = InstallEventHandler(GetApplicationEventTarget(), fandhe_hotkey_handler,
                                                  1, &spec, NULL, &g_handler_ref);
            if (status != noErr) {
                g_handler_ref = NULL;
                return (int32_t)status;
            }
        }
        EventHotKeyID id = {kFandheHotKeySignature, token};
        EventHotKeyRef ref = NULL;
        OSStatus status = RegisterEventHotKey(kVK_ANSI_K, controlKey | shiftKey | optionKey, id,
                                              GetApplicationEventTarget(), kEventHotKeyExclusive, &ref);
        if (status != noErr) {
            return (int32_t)status;
        }
        g_hotkey_ref = ref;
        g_token = token;
        return 0;
    }
}

int32_t fandhe_macos_global_hotkey_unregister(void) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 1;
        }
        if (g_hotkey_ref == NULL) {
            return 2;
        }
        OSStatus status = UnregisterEventHotKey(g_hotkey_ref);
        if (status != noErr) {
            return (int32_t)status;
        }
        g_hotkey_ref = NULL;
        return 0;
    }
}
