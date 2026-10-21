// Phase 11 の拡充ループ（親 #116）型サンプル「バーコード・QR 読み取り」: macOS 実装（Issue #557）。
// iOS 実装（#558）と共用できるよう `TARGET_OS` では分けない（Vision と Foundation だけを使い、
// UIKit / AppKit に触れない）。
//
// `crates/fandhe-egui-proxy/src/device/apple_barcode_scan.rs` から C ABI 経由で呼ばれる。
// `build.rs` が macOS・iOS ターゲットの feature `device` 有効時に `cc` コンパイルする
// （Rust 側 cfg と同じ OS 集合に保つ）。
//
// 契約（一発呼び出し型。結果を待つ要求は持たない）:
//   fandhe_apple_barcode_scan_read(const uint8_t *bytes, size_t len, void *ctx,
//                                  void (*on_payload)(void *ctx, const char *utf8)) -> int32_t
//     bytes/len : JPEG / PNG 等のバイト列（形式・8 MiB 上限・辺の上限は Rust 側で検査済み）。
//                 復号は Vision（ImageIO）に任せる。コピーせず呼び出しの間だけ参照する
//     on_payload: 読み取った中身を Vision が返した順に 1 件ずつ同期で呼ぶ。`ctx` と `utf8` は
//                 呼び出しの間だけ有効。この関数は `ctx` / `utf8` を保持しない
//     0 = 成功（0 件も成功 = コードなし）
//     1 = 失敗（復号できない・`performRequests` の失敗・UTF-8 変換の失敗。呼び出し元が
//         `Internal` へ畳む）
//
// 形式は絞らない（symbologies は既定の全形式）。`payloadStringValue` が nil の値（文字列に
// できないバイナリ等）は飛ばす。既知の上限: 中身に NUL を含む値は `UTF8String` の NUL で切れる。
// 空の値の除去は Rust 側の `normalize_payloads` が行う（契約の SSOT は
// `fandhe_device::barcode_scan`）。
//
// スレッド: 呼び出しごとにリクエストとハンドラを作るため共有状態もロックも持たない。
// 呼ばれたスレッド（Rust のワーカー）で同期に実行する（`dispatch_sync` は使わない。main 側が
// 呼び出し元を待つとデッドロックするため、`device-bridge-contracts.md`「11」）。Cocoa 呼び出しは
// `@autoreleasepool` の内側で行う。
//
// 画像・読み取った中身はログへ出さない（security.md A09）。権限・entitlement は不要。

#import <Foundation/Foundation.h>
#import <Vision/Vision.h>

int32_t fandhe_apple_barcode_scan_read(const uint8_t *bytes, size_t len, void *ctx,
                                       void (*on_payload)(void *ctx, const char *utf8)) {
    @autoreleasepool {
        NSData *data = [NSData dataWithBytesNoCopy:(void *)bytes length:len freeWhenDone:NO];
        VNDetectBarcodesRequest *request = [[VNDetectBarcodesRequest alloc] init];
        VNImageRequestHandler *handler =
            [[VNImageRequestHandler alloc] initWithData:data options:@{}];
        NSError *error = nil;
        if (![handler performRequests:@[ request ] error:&error]) {
            return 1;
        }
        for (VNBarcodeObservation *observation in request.results) {
            NSString *payload = observation.payloadStringValue;
            if (payload == nil) {
                continue;
            }
            const char *utf8 = [payload UTF8String];
            if (utf8 == NULL) {
                return 1;
            }
            on_payload(ctx, utf8);
        }
        return 0;
    }
}
