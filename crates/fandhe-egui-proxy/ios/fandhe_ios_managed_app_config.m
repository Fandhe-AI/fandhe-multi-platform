// Issue #945: iOS 向け MDM 管理アプリ設定の読み取り。
// `src/device/ios_managed_app_config.rs` から呼ばれる C ABI（契約の SSOT は
// `fandhe_device::managed_app_config`）。MDM が `NSUserDefaults` のキー
// `com.apple.configuration.managed` に置いた辞書から固定 3 キーを読み、数値と記号だけの
// ワイヤ文字列にして返す。文字列の中身・固定外のキー名は読まず、返さず、ログにも出さない
// （`NSLog` を使わない）。
//
// ワイヤ形式: <bool> US <integer> US <text> US <other>（US = 0x1f）。
//   各キー欄: "-" 未設定 / "?" 型違い / 値（bool は 1・0、integer は 10 進 i64、text は文字数）。
//   other 欄: 固定 3 キー以外のキーの件数（10 進）。辞書自体が無いときは "0"（Rust 側で NoConfig）。
//
// 戻り値: 0 = 成功、1 = 引数不正またはバッファ不足、2 = キーの値が辞書でない。
// `dictionaryForKey:` は「無い」と「辞書でない」を同じ nil で返すため使わず、
// `objectForKey:` + `isKindOfClass:` で区別する。`NSUserDefaults` はスレッド安全で UIKit に
// 触れないため、ワーカースレッドから同期で呼んでよい（main queue 不要）。ポインタは保持しない。

#import <Foundation/Foundation.h>
#import <CoreFoundation/CoreFoundation.h>
#include <stdint.h>
#include <stdio.h>

static BOOL is_boolean(id v) {
    return CFGetTypeID((__bridge CFTypeRef)v) == CFBooleanGetTypeID();
}

static NSString *bool_field(NSDictionary *d, NSString *key) {
    id v = d[key];
    if (v == nil) return @"-";
    return is_boolean(v) ? ([v boolValue] ? @"1" : @"0") : @"?";
}

static NSString *integer_field(NSDictionary *d, NSString *key) {
    id v = d[key];
    if (v == nil) return @"-";
    if (![v isKindOfClass:[NSNumber class]] || is_boolean(v) ||
        CFNumberIsFloatType((__bridge CFNumberRef)v)) {
        return @"?";
    }
    int64_t n = 0;
    if (!CFNumberGetValue((__bridge CFNumberRef)v, kCFNumberSInt64Type, &n)) return @"?";
    return [NSString stringWithFormat:@"%lld", (long long)n];
}

static NSString *text_field(NSDictionary *d, NSString *key) {
    id v = d[key];
    if (v == nil) return @"-";
    if (![v isKindOfClass:[NSString class]]) return @"?";
    NSUInteger chars = [(NSString *)v lengthOfBytesUsingEncoding:NSUTF32LittleEndianStringEncoding] / 4;
    if (chars > UINT32_MAX) chars = UINT32_MAX;
    return [NSString stringWithFormat:@"%lu", (unsigned long)chars];
}

int32_t fandhe_ios_managed_app_config_read(char *out, size_t cap) {
    if (out == NULL || cap == 0) return 1;
    out[0] = '\0';
    @autoreleasepool {
        id raw = [[NSUserDefaults standardUserDefaults] objectForKey:@"com.apple.configuration.managed"];
        NSString *s;
        if (raw == nil) {
            s = @"-\037-\037-\0370";
        } else if (![raw isKindOfClass:[NSDictionary class]]) {
            return 2;
        } else {
            NSDictionary *d = raw;
            // キー名は `fandhe_device::managed_app_config` の定数と同じ。
            NSArray<NSString *> *known = @[@"fandheSampleBool", @"fandheSampleInteger", @"fandheSampleText"];
            NSUInteger other = d.count;
            for (NSString *k in known) {
                if (d[k] != nil) other--;
            }
            if (other > UINT32_MAX) other = UINT32_MAX;
            s = [NSString stringWithFormat:@"%@\037%@\037%@\037%lu",
                 bool_field(d, known[0]), integer_field(d, known[1]), text_field(d, known[2]),
                 (unsigned long)other];
        }
        int n = snprintf(out, cap, "%s", [s UTF8String]);
        if (n < 0 || (size_t)n >= cap) {
            out[0] = '\0';
            return 1;
        }
    }
    return 0;
}
