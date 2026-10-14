// Phase 11 の拡充ループ（親 #1128）型サンプル「電源状態」: macOS 実装（Issue #1274）。
// IOKit `IOPSCopyPowerSourcesInfo` で電池の有無・残量・充電状態を、`NSProcessInfo` で
// 省電力モード・熱状態を同期で読む。macOS 専用（iOS は UIDevice を使い API が異なるため
// `apple/*.m` ではなく `macos/` に置く、`fandhe_macos_url_scheme.m` と同じ判断）。
//
// `crates/fandhe-egui-proxy/src/device/macos_power_state.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルし、IOKit をリンクする。
//
// 契約:
//   fandhe_macos_power_state_read(FandhePowerStateRaw *out) -> int32_t
//     UI スレッドから同期で呼ぶ（ブロックしない）。0 = 成功、非 0 = 失敗（out は無効）。
//     out の各フィールドは「取れない」を番兵で表す（Rust 側が範囲外も Unknown へ落とす）:
//       battery:   -1 不明 / 0 電池なし / 1 あり
//       level:     -1 不明 / 0..100
//       charging:  0 不明 / 1 充電中 / 2 放電中 / 3 満充電（kIOPSIsChargedKey が true のときのみ）
//       low_power: -1 不明 / 0 オフ / 1 オン
//       thermal:   -1 不明 / 0 Nominal / 1 Fair / 2 Serious / 3 Critical
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。Copy 系で得た CF オブジェクトは全経路で
// `CFRelease` する（`IOPSGetPowerSourceDescription` の戻りは Get 規則で解放しない）。

#import <Foundation/Foundation.h>
#import <IOKit/ps/IOPSKeys.h>
#import <IOKit/ps/IOPowerSources.h>
#include <stdint.h>

typedef struct {
    int32_t battery;
    int32_t level;
    int32_t charging;
    int32_t low_power;
    int32_t thermal;
} FandhePowerStateRaw;

int32_t fandhe_macos_power_state_read(FandhePowerStateRaw *out) {
    if (out == NULL) {
        return 1;
    }
    @autoreleasepool {
        out->battery = -1;
        out->level = -1;
        out->charging = 0;
        out->low_power = -1;
        out->thermal = -1;

        // 取得失敗（NULL）を「電池なし」や成功へ畳まない。確保済みの CF は解放して非 0 を返す。
        CFTypeRef blob = IOPSCopyPowerSourcesInfo();
        if (blob == NULL) {
            return 2;
        }
        CFArrayRef list = IOPSCopyPowerSourcesList(blob);
        if (list == NULL) {
            CFRelease(blob);
            return 3;
        }
        // 一覧を正常に得たうえで内蔵電池が無ければ「電池なし」（UPS などは無視）。
        out->battery = 0;
        CFIndex count = CFArrayGetCount(list);
        for (CFIndex i = 0; i < count; i++) {
            CFDictionaryRef desc =
                IOPSGetPowerSourceDescription(blob, CFArrayGetValueAtIndex(list, i));
            if (desc == NULL) {
                // 一覧にある電源の詳細が取れない = 電池の有無を断定できない。
                CFRelease(list);
                CFRelease(blob);
                return 4;
            }
            NSDictionary *d = (__bridge NSDictionary *)desc;
            if (![d[@kIOPSTypeKey] isEqual:@kIOPSInternalBatteryType]) {
                continue;
            }
            out->battery = 1;
            NSNumber *cur = d[@kIOPSCurrentCapacityKey];
            NSNumber *max = d[@kIOPSMaxCapacityKey];
            if ([cur isKindOfClass:[NSNumber class]] && [max isKindOfClass:[NSNumber class]] &&
                max.intValue > 0 && cur.intValue >= 0) {
                out->level = (int32_t)((cur.intValue * 100) / max.intValue);
            }
            if ([d[@kIOPSIsChargedKey] boolValue]) {
                out->charging = 3;
            } else if ([d[@kIOPSIsChargingKey] boolValue]) {
                out->charging = 1;
            } else if ([d[@kIOPSPowerSourceStateKey] isEqual:@kIOPSBatteryPowerValue]) {
                out->charging = 2;
            }
            break;
        }
        CFRelease(list);
        CFRelease(blob);

        NSProcessInfo *info = NSProcessInfo.processInfo;
        if (@available(macOS 12.0, *)) {
            out->low_power = info.isLowPowerModeEnabled ? 1 : 0;
        }
        out->thermal = (int32_t)info.thermalState;
    }
    return 0;
}
