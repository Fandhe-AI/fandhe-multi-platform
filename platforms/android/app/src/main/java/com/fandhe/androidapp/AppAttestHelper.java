package com.fandhe.androidapp;

import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.security.GeneralSecurityException;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.ProviderException;
import java.security.SecureRandom;
import java.security.cert.Certificate;
import java.security.spec.ECGenParameterSpec;
import java.util.Enumeration;
import java.util.HashSet;
import java.util.Iterator;
import java.util.Set;
import java.util.UUID;

// 端末・アプリの真正性証明（Issue #678、親 #132 の b-android）の Android 実装ヘルパー。
// Rust の `device/android_app_attest.rs` がドライバ（`device/app_attest.rs`）のワーカースレッドから
// `MainActivity.appAttest` / `appAttestRandom` 経由で同期 JNI で呼ぶ。Java から Rust への native
// コールバックは無く、JNI 署名の照合先は Java 委譲メソッド・Rust の呼び出し箇所・ホスト用の形の 3 か所。
// 扱う範囲は「要求ごとに EC P-256 鍵を AndroidKeyStore に作り、challenge を結び付けた Key Attestation の
// 証明書チェーンを 1 回取得して返す」ことだけ（assertion・鍵の持ち越し・StrongBox は範囲外）。
//
// コード表の SSOT はここで、Rust 側 `android_app_attest.rs` が同じ値で解釈する。
//   戻り値 byte[] の先頭 1 バイト:
//     0=取得（残りは証明書ごとに [u32 big-endian 長さ][DER] を葉から順に連結）
//     1=期限切れ / 2=鍵ストアの失敗 / 3=空のチェーン・0 バイトの証明書 / 4=件数・合計バイト超過 / 5=それ以外
//   Java は証明の信頼性を判定しない（根・security level は解釈しない。判断はサーバーの責務）。
//
// スレッド: 呼び出し元スレッドで同期実行する（main Looper は使わない）。鍵の生成から削除までを
// LOCK で直列化し、旧 Activity のワーカーと新しい要求が同じ KeyStore を同時に触らないようにする。
// 期限: `remainingMs` をロック取得前の `System.nanoTime()`（単調時計）に足して締切とする。
// ロック待ちも期限に数え、鍵生成後に締切を過ぎていれば成功を捨てて 1 を返す。
// ライフサイクル: Activity の再生成でも中断しない。結果の破棄は Rust 側の保留記録の破棄で行う。
// 鍵: alias は要求ごとに一意（UUID のみ。外部文字列を含めない）。finally で必ず削除する。
// 削除に失敗した場合は成功コードを返さず 2（鍵ストアの失敗）にし、alias を LEFTOVER に保持して
// 次の要求の冒頭で再試行する。前回プロセスが残した alias の掃除は、列挙と全削除が成功するまで
// 要求ごとに再試行する（途中で失敗したら swept を立てない）。
//
// 守る範囲: 失敗を成功値へ畳まないこと・鍵エントリの全経路での解放・FFI に渡す長さの上限。
// 守らない範囲: チェーンの信頼性、StrongBox の不使用（非搭載端末で例外になるため）、端末ごとの
// 所要時間、期限後も鍵生成が走り切ること、複数プロセス間。
// alias・challenge・証明データ・例外メッセージはログに出さない（ログ API を使わない）。
final class AppAttestHelper {
    private static final int OK = 0, TIMEOUT = 1, KEYSTORE = 2, EMPTY = 3, TOO_LARGE = 4, FAILED = 5;
    // SSOT は `fandhe_device::app_attest`。Rust 側と同じ上限。
    private static final int MAX_CHALLENGE_BYTES = 128;
    private static final int MAX_CERTS = 10;
    private static final int MAX_TOTAL_BYTES = 64 * 1024;
    private static final String ALIAS_PREFIX = "fandhe.app_attest.";

    private static final SecureRandom RANDOM = new SecureRandom();
    private static final Object LOCK = new Object();
    // LOCK の内側でだけ読み書きする。
    private static boolean swept = false;
    // 削除に失敗して残っている alias。LOCK の内側でだけ読み書きする。
    private static final Set<String> LEFTOVER = new HashSet<>();

    private AppAttestHelper() {}

    // challenge 用の乱数。長さが範囲外なら null（Rust 側は固定値で埋めず失敗にする）。
    static byte[] random(int len) {
        if (len <= 0 || len > MAX_CHALLENGE_BYTES) {
            return null;
        }
        byte[] out = new byte[len];
        RANDOM.nextBytes(out);
        return out;
    }

    static byte[] attest(byte[] challenge, long remainingMs) {
        if (remainingMs <= 0) {
            return code(TIMEOUT);
        }
        final long deadline = System.nanoTime() + remainingMs * 1_000_000L;
        if (challenge == null || challenge.length == 0 || challenge.length > MAX_CHALLENGE_BYTES) {
            return code(FAILED);
        }
        synchronized (LOCK) {
            if (System.nanoTime() >= deadline) {
                return code(TIMEOUT);
            }
            KeyStore ks;
            try {
                ks = KeyStore.getInstance("AndroidKeyStore");
                ks.load(null);
            } catch (GeneralSecurityException | IOException | ProviderException e) {
                return code(KEYSTORE);
            } catch (RuntimeException e) {
                return code(FAILED);
            }
            sweepOnce(ks);
            final String alias = ALIAS_PREFIX + UUID.randomUUID();
            byte[] result;
            try {
                KeyGenParameterSpec spec = new KeyGenParameterSpec.Builder(
                        alias, KeyProperties.PURPOSE_SIGN)
                        .setAlgorithmParameterSpec(new ECGenParameterSpec("secp256r1"))
                        .setDigests(KeyProperties.DIGEST_SHA256)
                        .setAttestationChallenge(challenge)
                        .build();
                KeyPairGenerator gen = KeyPairGenerator.getInstance(
                        KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore");
                gen.initialize(spec);
                gen.generateKeyPair();
                Certificate[] chain = ks.getCertificateChain(alias);
                // 期限切れでも return せず、下の release で鍵を消してから返す。
                result = System.nanoTime() >= deadline ? code(TIMEOUT) : encode(chain);
            } catch (GeneralSecurityException | ProviderException e) {
                result = code(KEYSTORE);
            } catch (RuntimeException e) {
                result = code(FAILED);
            } catch (Error e) {
                // Error は握りつぶさないが、鍵は消してから投げ直す（消せなければ LEFTOVER で次回再試行）。
                release(ks, alias);
                throw e;
            }
            // 鍵を消せなかったときは成功を返さない（鍵が残ったまま 0 を返す嘘の成功の禁止）。
            if (!release(ks, alias) && result[0] == OK) {
                return code(KEYSTORE);
            }
            return result;
        }
    }

    // チェーンを [コード 0][u32 BE 長さ][DER]... へ直す。null・空・0 バイト・上限超過は成功にしない。
    private static byte[] encode(Certificate[] chain) throws GeneralSecurityException {
        if (chain == null || chain.length == 0) {
            return code(EMPTY);
        }
        if (chain.length > MAX_CERTS) {
            return code(TOO_LARGE);
        }
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        out.write(OK);
        int total = 0;
        for (Certificate c : chain) {
            byte[] der = c == null ? null : c.getEncoded();
            if (der == null || der.length == 0) {
                return code(EMPTY);
            }
            total += der.length;
            if (total > MAX_TOTAL_BYTES) {
                return code(TOO_LARGE);
            }
            out.write((der.length >>> 24) & 0xff);
            out.write((der.length >>> 16) & 0xff);
            out.write((der.length >>> 8) & 0xff);
            out.write(der.length & 0xff);
            out.write(der, 0, der.length);
        }
        return out.toByteArray();
    }

    // 前回プロセスと過去の削除失敗の残りを掃除する。LOCK の内側で呼ぶ。
    // 今回の要求の成否には影響させない。列挙・削除のどれかが失敗したら swept を立てず次回再試行する。
    private static void sweepOnce(KeyStore ks) {
        Iterator<String> it = LEFTOVER.iterator();
        while (it.hasNext()) {
            if (deleteEntry(ks, it.next())) {
                it.remove();
            }
        }
        if (swept) {
            return;
        }
        boolean clean = true;
        try {
            Enumeration<String> aliases = ks.aliases();
            while (aliases.hasMoreElements()) {
                String a = aliases.nextElement();
                if (a != null && a.startsWith(ALIAS_PREFIX) && !deleteEntry(ks, a)) {
                    LEFTOVER.add(a);
                    clean = false;
                }
            }
        } catch (Exception e) {
            clean = false;
        }
        swept = clean;
    }

    // 鍵を削除し、失敗した alias は LEFTOVER へ保持する。成功なら true。
    private static boolean release(KeyStore ks, String alias) {
        if (deleteEntry(ks, alias)) {
            return true;
        }
        LEFTOVER.add(alias);
        return false;
    }

    private static boolean deleteEntry(KeyStore ks, String alias) {
        try {
            ks.deleteEntry(alias);
            return true;
        } catch (Exception e) {
            return false;
        }
    }

    private static byte[] code(int c) {
        return new byte[] {(byte) c};
    }
}
