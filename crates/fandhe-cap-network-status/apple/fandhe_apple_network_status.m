// Phase 11 の拡充ループ（親 #1130）: ネットワーク接続状態を Apple（macOS / iOS）へ橋渡しする
// （Issue #1282）。Network.framework の `NWPathMonitor` は iOS と macOS で同一 API のため
// 1 ファイルを共用する（UIKit / AppKit 非依存）。
//
// `crates/fandhe-egui-proxy/src/device/apple_network_status.rs` から C ABI 経由で呼ばれる。
// `build.rs` が feature `device` 有効時に `cc` コンパイルし Network をリンクする。
//
// 設計判断: `NWPathMonitor` に同期取得 API は無いため、初回呼び出しで monitor を 1 つだけ
// 開始し（`dispatch_once`）、handler が最新の path を 1 個の atomic に詰める。read は
// それを返すだけ（pull 型）。monitor と queue は static strong 参照で保持し ARC で解放
// されない。停止しない（プロセス終了で OS が片付ける）。handler が届く前は 0（不明）。
// 「接続あり」は path が satisfied の場合のみ。
//
// iOS でもコンパイルされる（Issue #1283）。handler は値を atomic に確定させた後で Rust の
// 受け口 `fandhe_rust_network_status_changed` を呼ぶ（値の確定→再描画の順、契約表 2）。
// iOS では受け口が `request_repaint` を呼び次フレームを起こし、macOS では何もしない。
//
// 契約:
//   int32_t fandhe_apple_network_status_read(void);
//   戻り値のビット: bit0-1 状態 0=不明 / 1=接続あり / 2=接続なし
//                   bit2-4 種別 0=不明 / 1=Wi-Fi / 2=携帯 / 3=有線 / 4=その他
//                   bit5-6 従量制 0=不明 / 1=いいえ / 2=はい
#import <Foundation/Foundation.h>
#import <Network/Network.h>
#import <stdatomic.h>

extern void fandhe_rust_network_status_changed(void);

static _Atomic int32_t g_packed = 0;
static nw_path_monitor_t g_monitor = nil;
static dispatch_queue_t g_queue = nil;

static int32_t pack_path(nw_path_t path) {
    int32_t status = 0;
    switch (nw_path_get_status(path)) {
        case nw_path_status_satisfied:
            status = 1;
            break;
        case nw_path_status_unsatisfied:
        case nw_path_status_satisfiable:
            status = 2;
            break;
        default:
            status = 0;
            break;
    }
    if (status != 1) {
        return status;
    }
    int32_t kind = 4;
    if (nw_path_uses_interface_type(path, nw_interface_type_wifi)) {
        kind = 1;
    } else if (nw_path_uses_interface_type(path, nw_interface_type_cellular)) {
        kind = 2;
    } else if (nw_path_uses_interface_type(path, nw_interface_type_wired)) {
        kind = 3;
    }
    int32_t metered = nw_path_is_expensive(path) ? 2 : 1;
    return status | (kind << 2) | (metered << 5);
}

int32_t fandhe_apple_network_status_read(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        @autoreleasepool {
            g_queue = dispatch_queue_create("com.fandhe.network-status", DISPATCH_QUEUE_SERIAL);
            g_monitor = nw_path_monitor_create();
            nw_path_monitor_set_queue(g_monitor, g_queue);
            nw_path_monitor_set_update_handler(g_monitor, ^(nw_path_t path) {
                atomic_store(&g_packed, pack_path(path));
                fandhe_rust_network_status_changed();
            });
            nw_path_monitor_start(g_monitor);
        }
    });
    return atomic_load(&g_packed);
}
