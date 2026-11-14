package com.fandhe.platform;

import android.content.Context;
import android.content.SharedPreferences;

import androidx.security.crypto.EncryptedSharedPreferences;
import androidx.security.crypto.MasterKey;

import java.io.IOException;
import java.security.GeneralSecurityException;

// 資格情報保存（Issue #924）の Android 実装。Rust の `fandhe-cap-secure-credential/src/android.rs` が
// static メソッド `request(Context, int, String, String)` を JNI で呼ぶ（Context は Activity を渡し、
// 先頭で `getApplicationContext()` を取る）。状態は `EncryptedSharedPreferences` の static 1 つだけで、
// Activity のライフサイクルに参加しないため provider は作らない。
//
// バックアップ除外: 保存ファイル（`SECURE_CREDENTIAL_FILE`）は端末の Keystore 鍵に紐づくため、別端末へ復元されても
// 復号できない。アプリ側の `res/xml/data_extraction_rules.xml` が同名のファイルをバックアップから外す責務を持つ
// （`<application android:dataExtractionRules>` は機能の宣言では表せない。crate doc 参照）。
final class SecureCredentialHelper {
    // 資格情報保存の op 定数。Rust 側 `android.rs` の同名定数と対で決める契約。
    private static final int SECURE_CREDENTIAL_OP_STORE = 0;
    private static final int SECURE_CREDENTIAL_OP_LOAD = 1;
    private static final int SECURE_CREDENTIAL_OP_DELETE = 2;

    // `EncryptedSharedPreferences` の保存ファイル名（固定）。`data_extraction_rules.xml` と一致させる。
    private static final String SECURE_CREDENTIAL_FILE = "fandhe_secure_credential";

    // 保存値に付ける固定プレフィックス。`EncryptedSharedPreferences.getDecryptedObject`
    // （androidx.security:security-crypto の内部実装）は復号後の文字列が `"__NULL__"`
    // と一致すると null 番兵として扱い、`getString` の戻り値を強制的に null にする
    // （codex-review 指摘、PR #931）。値が偶然この文字列と一致すると LOAD が `Ok(None)`
    // になり Rust 側の「無い」と区別が付かなくなるため、任意の値が番兵と衝突しない
    // 可逆表現として全値にこのプレフィックスを付けて保存する。
    private static final String SECURE_CREDENTIAL_VALUE_PREFIX = "v1:";

    private SecureCredentialHelper() {}

    // 保存直前に呼ぶ。プレフィックスを付けるだけの純粋関数。
    private static String encodeSecureCredentialValue(String value) {
        return SECURE_CREDENTIAL_VALUE_PREFIX + value;
    }

    // 読み出し直後に呼ぶ。プレフィックスを検証して外す。プレフィックスが無い値は
    // 想定外（このプレフィックス方式導入前の値、または外部からの改変）のため
    // 「破損」として `null` ではなく例外で失敗させる。呼び出し元でそのまま
    // `Ok(None)` として扱われる（＝「無い」と誤認される）ことを防ぐ。
    private static String decodeSecureCredentialValue(String stored) {
        if (!stored.startsWith(SECURE_CREDENTIAL_VALUE_PREFIX)) {
            throw new IllegalStateException("secure credential unavailable");
        }
        return stored.substring(SECURE_CREDENTIAL_VALUE_PREFIX.length());
    }

    // 遅延生成した `EncryptedSharedPreferences` インスタンス。Activity 再生成
    // （設定変更等）を跨いで生き残るよう `static` に保持し、初回生成時の
    // Keystore 初期化コストを 1 回に抑える。`securePrefs()` の `synchronized`
    // が並行アクセス（複数の背景スレッドからの同時要求）に対する排他を担う。
    private static SharedPreferences securePrefsInstance;

    // `securePrefsInstance` を遅延生成して返す。呼び出しは Rust 側の背景スレッド
    // （effect ドライバが `std::thread::spawn` した先、UI スレッドではない）から到達するため、
    // 生成中のブロッキング（Keystore 初期化）は画面を固めない。
    //
    // バックアップ除外が効かない経路（旧 OS・未対応の OEM 実装等）で万一 `fandhe_secure_credential.xml` だけが
    // 復元された場合、Keystore に対応する鍵が無いため `EncryptedSharedPreferences.create` は
    // `GeneralSecurityException`/`IOException` を投げる。この例外は `request` の catch 節で
    // `IllegalStateException` に変換され Rust 側は `Err` として扱う契約のため、偽の成功（`Ok`）にはならない。
    @SuppressWarnings("deprecation")
    private static synchronized SharedPreferences securePrefs(Context context)
            throws GeneralSecurityException, IOException {
        if (securePrefsInstance == null) {
            MasterKey masterKey =
                    new MasterKey.Builder(context)
                            .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
                            .build();
            securePrefsInstance =
                    EncryptedSharedPreferences.create(
                            context,
                            SECURE_CREDENTIAL_FILE,
                            masterKey,
                            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
                            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM);
        }
        return securePrefsInstance;
    }

    // Rust 側から JNI 経由で呼ばれる同期メソッド。`op` は上記 3 定数のいずれか。
    // 戻り値: STORE/DELETE は常に null。LOAD はヒット時の値、無い場合は null。
    // 内部エラー（Keystore/暗号化失敗等）は `IllegalStateException`（固定文言、
    // 値を含めない）を投げる。Rust 側はこの例外を `exception_clear` するだけで
    // 文言を読まない契約（security.md A09、秘密値・例外メッセージをログに残さない）。
    //
    // `EncryptedSharedPreferences`（androidx.security:security-crypto）は
    // 1.1.0-alpha07 で全 API が deprecated 済み（Android Keystore 直接利用が
    // 推奨）だが、動作はするため Issue #924 の明示要件どおり採用する。
    @SuppressWarnings("deprecation")
    static String request(Context activity, int op, String key, String value) {
        // pref キーは `"fandhe." + key`（Apple Keychain 実装の `kSecAttrAccount`
        // 接頭辞規約と同じ）。`key` は Rust 側 `validate_key` で検証済み
        // （英数字・`.`/`_`/`-` のみ）のためそのまま連結してよい。
        String prefKey = "fandhe." + key;
        try {
            SharedPreferences prefs = securePrefs(activity.getApplicationContext());
            switch (op) {
                case SECURE_CREDENTIAL_OP_STORE:
                    // `commit()`（同期）を使う。`apply()` は非同期のため、直後の
                    // LOAD 要求で未反映になりうる（Rust 側は往復を保証しない契約）。
                    if (!prefs.edit()
                            .putString(prefKey, encodeSecureCredentialValue(value))
                            .commit()) {
                        throw new IllegalStateException("secure credential unavailable");
                    }
                    return null;
                case SECURE_CREDENTIAL_OP_LOAD:
                    String stored = prefs.getString(prefKey, null);
                    return stored == null ? null : decodeSecureCredentialValue(stored);
                case SECURE_CREDENTIAL_OP_DELETE:
                    // キーが無くても `remove().commit()` は成功する（冪等）。
                    if (!prefs.edit().remove(prefKey).commit()) {
                        throw new IllegalStateException("secure credential unavailable");
                    }
                    return null;
                default:
                    throw new IllegalStateException("secure credential unavailable");
            }
        } catch (GeneralSecurityException | IOException | RuntimeException e) {
            throw new IllegalStateException("secure credential unavailable");
        }
    }
}
