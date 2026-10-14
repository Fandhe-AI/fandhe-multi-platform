// Phase 11 の拡充ループ（親 #921）型サンプル「資格情報」: iOS / macOS Keychain 実装
// （Issue #922・#924）。Security.framework（Keychain Services、`SecItem*` C API）で
// `kSecClassGenericPassword` 項目を保存・読み出し・削除する。
//
// `crates/fandhe-egui-proxy/src/device/apple_secure_credential.rs` から C ABI 経由で
// 呼ばれる。`build.rs` が iOS・macOS 双方のターゲット（feature `device` 有効時）で
// `cc` コンパイルし、Security フレームワークをリンクする（`SecItem*` API は iOS/macOS
// 同一のため本ファイルはそのまま共用、分岐なし）。
//
// 契約（同期・out 引数、`fandhe_apple_text_scale_read` と同型の pull 方式。ただし
// 戻り値コードは複数値を持つ）:
//   fandhe_apple_secure_credential_store(const char *key,
//                                         const uint8_t *secret, size_t secret_len)
//     -> int32_t。0=成功、2=入力不正（key が NULL/空）、その他=内部エラー。
//   fandhe_apple_secure_credential_load(const char *key,
//                                        uint8_t *out, size_t cap, size_t *out_len)
//     -> int32_t。0=成功（*out_len に書いたバイト数、out へコピー済み）、
//        1=該当項目が無い（out/out_len は書き換えない）、2=入力不正、
//        3=cap 不足（*out_len に必要なバイト数を書く。out は書き換えない）、
//        その他=内部エラー。
//   fandhe_apple_secure_credential_delete(const char *key) -> int32_t。
//     0=成功（削除できた）、1=該当項目が無い（Rust 側は冪等 `Ok(())` として扱う契約、
//     `secure_credential.rs::SecureCredential::delete` の doc 参照）、
//     2=入力不正、その他=内部エラー。
//
// 識別子: `kSecAttrService` は固定文字列（下記 `kFandheService`、
// `macos/Info.plist` の `CFBundleIdentifier` と同じ値）、`kSecAttrAccount` は
// `"fandhe." + key`（`key` は呼び出し元 `apple_secure_credential.rs` が
// `fandhe_device::secure_credential::validate_key` で検証済みの
// 英数字・`.`/`_`/`-` のみの文字列、外部入力をそのまま OS 識別子へ連結しない
// 契約、security.md A03）。
//
// `kSecAttrAccessible = kSecAttrAccessibleWhenUnlockedThisDeviceOnly`
// （同期・バックアップ対象外、security.md A05）を付ける。
// `kSecUseDataProtectionKeychain` は設定しない: 素の実行体（未署名・非バンドル）で
// data-protection keychain を使うと `errSecMissingEntitlement` になるため、
// ログインキーチェーン（ファイルベース）へ保存する。実機確認手順は
// `docs/device/capabilities/secure-credential.json` の `note` 参照。
// ログインキーチェーンでは `kSecUseDataProtectionKeychain` 無指定のとき
// `kSecAttrAccessible` の "ThisDeviceOnly" 側面（バックアップ対象外）は
// Apple の仕様上そのままでは保証されない（codex-review P1 指摘）。
// `ThisDeviceOnly` 系の値は `kSecAttrSynchronizable` が真の項目には設定できない
// （相互排他）ため、`kSecAttrSynchronizable = false` を明示して項目を非同期
// （iCloud キーチェーン対象外）に固定し、"この端末限定" の意図を
// data-protection keychain なしでも実現可能な範囲で担保する。
//
// 秘密値・OSStatus の生値・キーチェーン項目の中身をログへ出力しない
// （`NSLog` を置かない、security.md A09）。

#import <Security/Security.h>
#import <Foundation/Foundation.h>

static NSString *const kFandheService = @"com.fandhe.egui-proxy-desktop";
static NSString *const kFandheAccountPrefix = @"fandhe.";

// 検証済み `key`（C 文字列、UTF-8）から Keychain のクエリ辞書を組み立てる。
// `key` が NULL/空の場合は nil を返す（呼び出し元が入力不正コードへ写す）。
static NSMutableDictionary *FandheBaseQuery(const char *key) {
    if (key == NULL || key[0] == '\0') {
        return nil;
    }
    NSString *keyString = [NSString stringWithUTF8String:key];
    if (keyString == nil) {
        // UTF-8 として不正（呼び出し元は検証済みの ASCII サブセットしか渡さない
        // 契約だが、契約違反時にクラッシュさせず入力不正として扱う）。
        return nil;
    }
    NSString *account = [kFandheAccountPrefix stringByAppendingString:keyString];

    return [@{
        (__bridge id)kSecClass : (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService : kFandheService,
        (__bridge id)kSecAttrAccount : account,
    } mutableCopy];
}

int32_t fandhe_apple_secure_credential_store(const char *key, const uint8_t *secret,
                                              size_t secret_len) {
    // `drive`（Rust 側 effect ドライバ）が素の `std::thread` 上でこの関数を呼ぶため
    // （メインキューへホップしない、モジュール doc・drive の doc 参照）、この
    // スレッドには Cocoa のランループが張る autorelease pool が存在しない。
    // `FandheBaseQuery` の `NSString`/`NSDictionary` リテラルや `NSData`
    // 生成物は autorelease されうるため、関数本体を明示的な pool で囲む
    // （Bugbot 指摘: pool が無いと呼び出しのたびに秘密値を保持するオブジェクトが
    // リークしうる）。
    @autoreleasepool {
        NSMutableDictionary *query = FandheBaseQuery(key);
        if (query == nil || secret == NULL) {
            return 2;
        }

        NSData *secretData = [NSData dataWithBytes:secret length:secret_len];

        NSMutableDictionary *addQuery = [query mutableCopy];
        addQuery[(__bridge id)kSecValueData] = secretData;
        addQuery[(__bridge id)kSecAttrAccessible] =
            (__bridge id)kSecAttrAccessibleWhenUnlockedThisDeviceOnly;
        addQuery[(__bridge id)kSecAttrSynchronizable] = @NO;

        OSStatus status = SecItemAdd((__bridge CFDictionaryRef)addQuery, NULL);
        if (status == errSecSuccess) {
            return 0;
        }
        if (status == errSecDuplicateItem) {
            // 既存項目がある場合は値のみ更新する（削除 + 再作成ではなく `SecItemUpdate`
            // で上書きを明示する、security.md A08）。
            // `kSecAttrSynchronizable` は `SecItemUpdate` の attributes 辞書には含めない:
            // この属性は更新不可能で、含めると `errSecParam` で失敗する（Bugbot 指摘）。
            // 新規追加時（`SecItemAdd`）に既に `@NO` を設定済みのため、更新時は
            // 変更不要（値の書き換えのみ行う）。
            NSDictionary *update = @{
                (__bridge id)kSecValueData : secretData,
            };
            OSStatus updateStatus =
                SecItemUpdate((__bridge CFDictionaryRef)query, (__bridge CFDictionaryRef)update);
            return updateStatus == errSecSuccess ? 0 : 4;
        }
        return 4;
    }
}

int32_t fandhe_apple_secure_credential_load(const char *key, uint8_t *out, size_t cap,
                                             size_t *out_len) {
    // `store` と同じ理由（上記 doc 参照）で pool を明示する。
    @autoreleasepool {
        NSMutableDictionary *query = FandheBaseQuery(key);
        if (query == nil || out == NULL || out_len == NULL) {
            return 2;
        }

        NSMutableDictionary *matchQuery = [query mutableCopy];
        matchQuery[(__bridge id)kSecReturnData] = @YES;
        matchQuery[(__bridge id)kSecMatchLimit] = (__bridge id)kSecMatchLimitOne;

        CFTypeRef result = NULL;
        OSStatus status = SecItemCopyMatching((__bridge CFDictionaryRef)matchQuery, &result);
        if (status == errSecItemNotFound) {
            return 1;
        }
        if (status != errSecSuccess || result == NULL) {
            return 4;
        }

        NSData *data = (__bridge_transfer NSData *)result;
        if (data.length > cap) {
            *out_len = data.length;
            return 3;
        }
        memcpy(out, data.bytes, data.length);
        *out_len = data.length;
        return 0;
    }
}

int32_t fandhe_apple_secure_credential_delete(const char *key) {
    // `store` と同じ理由（上記 doc 参照）で pool を明示する。
    @autoreleasepool {
        NSMutableDictionary *query = FandheBaseQuery(key);
        if (query == nil) {
            return 2;
        }

        OSStatus status = SecItemDelete((__bridge CFDictionaryRef)query);
        if (status == errSecSuccess) {
            return 0;
        }
        if (status == errSecItemNotFound) {
            return 1;
        }
        return 4;
    }
}
