// Phase 11 の拡充ループ（親 #189）型サンプル「無線の状態と切り替え」: macOS 実装（Issue #972）。
// Wi-Fi は CoreWLAN の `CWInterface.powerOn` で読み、`setPower:error:` で切り替える。Bluetooth は
// IOBluetooth の `IOBluetoothHostController.powerState` の読み取りのみ（切り替えの公開 API はなく、
// 非公開の `IOBluetoothPreferenceSetControllerPowerState` は使わない）。接続できるかを事前に判定せず
// OS の戻り値を写す。NSError はログに出さない。
//
// `crates/fandhe-egui-proxy/src/device/macos_radio_control.rs` から C ABI 経由で呼ばれる。`build.rs` の
// `build_macos()` が feature `device` 有効時にのみコンパイルし、CoreWLAN と IOBluetooth をリンクする。
// 権限宣言は不要（`setPower` は管理者パスワードを求めることがある）。Rust のテスト実行体からは
// 実物を呼ばない（無線の状態を変えてしまうため）。
//
// 契約:
//   int32_t fandhe_macos_radio_read(int32_t kind)
//     kind: 0 = Wi-Fi / 1 = Bluetooth。ワーカースレッドから呼ぶ。期限を持たない。
//     戻り値: 0 = オン / 1 = オフ / 2 = 無線なし / 3 = 判定できない（Bluetooth の
//             UNINTIALIZED など ON/OFF 以外） / 5 = 内部失敗（不正な kind）。
//   int32_t fandhe_macos_radio_wifi_set(int32_t on)
//     ワーカースレッドから呼ぶブロッキング関数（main thread で呼ぶと 5 を返す）。管理者パスワードの
//     ダイアログの間は戻らない（期限を持たない。守らない範囲として Rust 側 doc に明記）。
//     戻り値: 0 = 切り替えた / 1 = 非対応（Wi-Fi インターフェースが無い） /
//             5 = 内部失敗（不正な引数・main thread からの呼び出し・setPower の失敗。管理者パスワードの
//             取り消しを含む）。
//
// メモリ安全: ポインタを受け取らない。ObjC クラスは作らない（クラス名の衝突を避ける）。
// FFI 規約: 全体を `@autoreleasepool` で囲む。

#import <CoreWLAN/CoreWLAN.h>
#import <Foundation/Foundation.h>
#import <IOBluetooth/IOBluetooth.h>
#include <stdint.h>

int32_t fandhe_macos_radio_read(int32_t kind) {
    @autoreleasepool {
        if (kind == 0) {
            CWInterface *interface = [CWWiFiClient sharedWiFiClient].interface;
            if (interface == nil) {
                return 2;
            }
            return interface.powerOn ? 0 : 1;
        }
        if (kind == 1) {
            IOBluetoothHostController *controller = [IOBluetoothHostController defaultController];
            if (controller == nil) {
                return 2;
            }
            BluetoothHCIPowerState state = controller.powerState;
            if (state == kBluetoothHCIPowerStateON) {
                return 0;
            }
            if (state == kBluetoothHCIPowerStateOFF) {
                return 1;
            }
            return 3;
        }
        return 5;
    }
}

int32_t fandhe_macos_radio_wifi_set(int32_t on) {
    @autoreleasepool {
        if ([NSThread isMainThread] || (on != 0 && on != 1)) {
            return 5;
        }
        CWInterface *interface = [CWWiFiClient sharedWiFiClient].interface;
        if (interface == nil) {
            return 1;
        }
        NSError *error = nil;
        return [interface setPower:(on == 1) error:&error] ? 0 : 5;
    }
}
