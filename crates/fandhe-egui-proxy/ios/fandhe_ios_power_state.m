// 電源状態: iOS 実装（Issue #1275）。`UIDevice`（電池）と `NSProcessInfo`（省電力・熱）を読む。
//
// `src/device/ios_power_state.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の
// feature `device` 有効時のみ `cc` コンパイルする（UIKit / Foundation はリンク済み）。
//
// 契約（一発呼び出し型。結果を待つ要求は持たない）:
//   fandhe_ios_power_state_read(FandheIosPowerStateRaw *out) -> int32_t
//     0 = 成功、1 = out が NULL、2 = main スレッド以外から呼ばれた（UIKit に触れず返す。
//     呼び出し元が `Internal` へ写す）。
//   生値は OS のまま返し、値域チェックと契約値への写像は Rust 側 `from_raw` が行う:
//     battery_state = UIDeviceBatteryState（0 Unknown / 1 Unplugged / 2 Charging / 3 Full）
//     battery_level = batteryLevel（0.0..1.0、不明は -1.0）
//     low_power     = isLowPowerModeEnabled（0 / 1）
//     thermal       = NSProcessInfoThermalState（0 Nominal .. 3 Critical）
//
// 電池の監視は `batteryMonitoringEnabled` が YES のときだけ値が出る。読み取りのたびに NO なら
// YES にする（冪等）。画面を離れる通知経路が無いため元へは戻さない（守らない範囲）。有効化直後の
// 1 回目は Unknown が返りうるが、次の pull で拾える。
//
// スレッド: effect は main から同期で呼ぶ。UIKit は `@autoreleasepool` の内側で触り、main 以外は
// `dispatch_sync` せず 2 を返す（デッドロック回避、`device-bridge-contracts.md`「11」）。
// ログは出さない（security.md A09）。

#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>

// Rust 側 `Raw`（#[repr(C)]）とフィールドの順・型を揃える。
typedef struct {
    int32_t battery_state;
    float battery_level;
    int32_t low_power;
    int32_t thermal;
} FandheIosPowerStateRaw;

int32_t fandhe_ios_power_state_read(FandheIosPowerStateRaw *out) {
    if (out == NULL) {
        return 1;
    }
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        UIDevice *dev = UIDevice.currentDevice;
        if (!dev.isBatteryMonitoringEnabled) {
            dev.batteryMonitoringEnabled = YES;
        }
        out->battery_state = (int32_t)dev.batteryState;
        out->battery_level = dev.batteryLevel;
        NSProcessInfo *info = NSProcessInfo.processInfo;
        out->low_power = info.isLowPowerModeEnabled ? 1 : 0;
        out->thermal = (int32_t)info.thermalState;
    }
    return 0;
}
