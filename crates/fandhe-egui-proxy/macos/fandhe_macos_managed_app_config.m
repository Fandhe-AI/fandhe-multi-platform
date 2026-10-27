// Issue #944: macOS 向け MDM 管理アプリ設定の読み取り。
// `src/device/macos_managed_app_config.rs` から呼ばれる C ABI（契約の SSOT は
// `fandhe_device::managed_app_config`）。固定 3 キーを CFPreferences で読み、**強制されている値**
// （構成プロファイル由来）だけを管理値として、数値と記号だけのワイヤ文字列にして返す。
// 文字列の中身・固定外のキー名は読まず、返さず、ログにも出さない。
//
// ワイヤ形式: <bool> US <integer> US <text> US <other>（US = 0x1f）。
//   各キー欄: "-" 未設定 / "?" 型違い / 値（bool は 1・0、integer は 10 進 i64、text は文字数）。
//   other 欄: "-"（列挙しないため数えない）。
//
// 戻り値: 0 = 成功、1 = 引数不正またはバッファ不足、2 = main bundle の識別子が取れない
// （domain を取り違えて「設定なし」と嘘を言わないため失敗にする）。ポインタは保持しない。

#import <Foundation/Foundation.h>
#import <CoreFoundation/CoreFoundation.h>
#include <stdint.h>
#include <stdio.h>

// 強制されている値だけを返す（ユーザーの `defaults write` 由来は nil）。所有は ARC へ移す。
static id forced_value(NSString *key) {
    CFStringRef k = (__bridge CFStringRef)key;
    if (!CFPreferencesAppValueIsForced(k, kCFPreferencesCurrentApplication)) {
        return nil;
    }
    return CFBridgingRelease(CFPreferencesCopyAppValue(k, kCFPreferencesCurrentApplication));
}

static BOOL is_boolean(id v) {
    return CFGetTypeID((__bridge CFTypeRef)v) == CFBooleanGetTypeID();
}

static NSString *bool_field(NSString *key) {
    id v = forced_value(key);
    if (v == nil) return @"-";
    return is_boolean(v) ? ([v boolValue] ? @"1" : @"0") : @"?";
}

static NSString *integer_field(NSString *key) {
    id v = forced_value(key);
    if (v == nil) return @"-";
    if (![v isKindOfClass:[NSNumber class]] || is_boolean(v) ||
        CFNumberIsFloatType((__bridge CFNumberRef)v)) {
        return @"?";
    }
    int64_t n = 0;
    if (!CFNumberGetValue((__bridge CFNumberRef)v, kCFNumberSInt64Type, &n)) return @"?";
    return [NSString stringWithFormat:@"%lld", (long long)n];
}

static NSString *text_field(NSString *key) {
    id v = forced_value(key);
    if (v == nil) return @"-";
    if (![v isKindOfClass:[NSString class]]) return @"?";
    NSUInteger chars = [(NSString *)v lengthOfBytesUsingEncoding:NSUTF32LittleEndianStringEncoding] / 4;
    if (chars > UINT32_MAX) chars = UINT32_MAX;
    return [NSString stringWithFormat:@"%lu", (unsigned long)chars];
}

int32_t fandhe_macos_managed_app_config_read(char *out, size_t cap) {
    if (out == NULL || cap == 0) return 1;
    out[0] = '\0';
    @autoreleasepool {
        if (CFBundleGetIdentifier(CFBundleGetMainBundle()) == NULL) return 2;
        // キー名は `fandhe_device::managed_app_config` の定数と同じ。
        NSString *s = [NSString stringWithFormat:@"%@\x1f%@\x1f%@\x1f-",
                       bool_field(@"fandheSampleBool"),
                       integer_field(@"fandheSampleInteger"),
                       text_field(@"fandheSampleText")];
        int n = snprintf(out, cap, "%s", [s UTF8String]);
        if (n < 0 || (size_t)n >= cap) {
            out[0] = '\0';
            return 1;
        }
    }
    return 0;
}
