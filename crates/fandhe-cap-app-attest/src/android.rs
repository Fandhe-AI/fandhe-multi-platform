//! Android 向け端末・アプリの真正性証明（Issue #677 で骨格、#678 で実体。親 #132 の b-android）。
//! `driver` の Android 経路から、ドライバのワーカースレッド（`app-attest`）で呼ばれ、
//! `AppAttestHelper.random` / `AppAttestHelper.attest`（static）を `fandhe_platform_android::with_activity_class` 経由の同期 JNI で呼ぶ。
//! Java 側は `AppAttestHelper`（AndroidKeyStore の EC 鍵 + `setAttestationChallenge` の Key Attestation）。
//! Java から Rust への native コールバックは無いので、JNI 署名の照合先は Java 委譲メソッド・
//! 本ファイルの呼び出し箇所・ホスト用の形の 3 か所。契約の SSOT は `crate::contract`。
//!
//! # コード表（SSOT は Java `AppAttestHelper`）
//!
//! 戻りの `byte[]` の先頭 1 バイト: 0 = 取得（残りは証明書ごとに `[u32 big-endian 長さ][DER]` を葉から順に連結）/
//! 1 = 期限切れ / 2 = 鍵ストアの失敗 / 3 = 空のチェーン・0 バイトの証明書 / 4 = 件数・合計バイト超過 /
//! 5 = それ以外。null・空・未知のコード・長さ表記の不整合は `Internal` に倒し、成功にはしない。
//! 0 バイトの証明書の判定は復号では行わず、ドライバの `check_result` だけが決める。
//!
//! # 期限・ライフサイクル
//!
//! 期限は呼び出しに渡す残り時間で、Java が単調時計で確かめる（期限後に届いた成功は捨てて 1 を返す）。
//! Activity の作り直しでは Rust 側の保留記録を `reset_pending_tracking`（`fandhe-gallery` の `android_main` が呼ぶ） が捨てる
//! （`android.rs` から配線済み。要求 id は単調に増えるので、古い要求の完了が新しい要求を消費しない）。
//! 鍵は要求ごとに一意の alias で作り、Java が finally で削除する。前回プロセスの残りは初回に掃除する。
//!
//! 守る範囲: 1 プロセス内の要求と完了の順序、失敗を成功値へ畳まないこと、鍵エントリの解放、
//! FFI に渡す長さの上限。
//! 守らない範囲: チェーンの信頼性（判断はサーバーの責務）、StrongBox の不使用、端末ごとの所要時間、
//! 期限後も鍵生成が走り切ること、複数プロセス間。
//! alias・challenge・証明データ・OS の生エラー値はログに出さない（security.md A09）。実機確認は #674。

#[cfg(any(target_os = "android", test))]
use crate::contract::{Attestation, KEYSTORE, MAX_CERTS, MAX_TOTAL_BYTES, TIMEOUT, TOO_LARGE};
#[cfg(any(target_os = "android", test))]
use crate::error::AppAttestError;

/// 画面側へ見せる失敗の識別子（文言はドライバの `to_failure` が固定文言にする）。
#[cfg(any(target_os = "android", test))]
const FAILED: &str = "app_attest";
/// 空のチェーン・0 バイトの証明書（Java のコード 3）。
#[cfg(any(target_os = "android", test))]
use crate::contract::EMPTY;

/// Java の戻りとして受け取る最大長（コード 1 + 長さ表記 + 本体）。確保の前に検査する。
#[cfg(any(target_os = "android", test))]
const MAX_REPLY_BYTES: usize = 1 + MAX_CERTS * 4 + MAX_TOTAL_BYTES;

/// Java の戻り（先頭 1 バイト = コード）を契約へ写す。成功以外はすべて `Err`。
#[cfg(any(target_os = "android", test))]
fn decode_reply(reply: &[u8]) -> Result<Attestation, AppAttestError> {
    match reply.split_first() {
        Some((0, body)) => decode_body(body),
        Some((1, _)) => Err(AppAttestError::Internal(TIMEOUT)),
        Some((2, _)) => Err(AppAttestError::Internal(KEYSTORE)),
        Some((3, _)) => Err(AppAttestError::Internal(EMPTY)),
        Some((4, _)) => Err(AppAttestError::Internal(TOO_LARGE)),
        _ => Err(AppAttestError::Internal(FAILED)),
    }
}

/// `[u32 big-endian 長さ][DER]` の連結を証明書の列へ戻す。途中で切れる・余る・件数超過は失敗。
#[cfg(any(target_os = "android", test))]
fn decode_body(mut body: &[u8]) -> Result<Attestation, AppAttestError> {
    let mut certs: Vec<Vec<u8>> = Vec::new();
    while !body.is_empty() {
        if certs.len() >= MAX_CERTS {
            return Err(AppAttestError::Internal(TOO_LARGE));
        }
        let (head, rest) = body
            .split_first_chunk::<4>()
            .ok_or(AppAttestError::Internal(FAILED))?;
        let len = usize::try_from(u32::from_be_bytes(*head))
            .map_err(|_| AppAttestError::Internal(FAILED))?;
        if len > rest.len() {
            return Err(AppAttestError::Internal(FAILED));
        }
        let (der, rest) = rest.split_at(len);
        certs.push(der.to_vec());
        body = rest;
    }
    if certs.is_empty() {
        return Err(AppAttestError::Internal(FAILED));
    }
    Ok(Attestation { certs })
}

#[cfg(target_os = "android")]
mod imp {
    use std::time::Duration;

    use crate::contract::{
        AppAttest, Attestation, Challenge, MAX_CHALLENGE_BYTES, TIMEOUT, TOO_LARGE,
    };
    use crate::error::AppAttestError;
    use jni::objects::{JByteArray, JValue};

    use super::{FAILED, MAX_REPLY_BYTES, decode_reply};

    pub(super) struct AndroidAppAttest;

    /// `AppAttestHelper` の static メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`（例外は消す）。
    /// unsafe は `fandhe_platform_android::with_activity_class` に集約されている。
    fn call_helper<T>(
        f: impl FnOnce(
            &mut jni::Env,
            &jni::objects::JClass,
        ) -> Result<T, fandhe_platform_android::JniCallFailed>,
    ) -> Option<T> {
        let app = fandhe_platform_android::android_app()?;
        fandhe_platform_android::with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.AppAttestHelper"),
            |env, _activity, class| {
                let r = f(env, class);
                if r.is_err() && env.exception_check() {
                    env.exception_clear();
                }
                r
            },
        )
        .ok()
    }

    impl AppAttest for AndroidAppAttest {
        fn fill_random(&self, buf: &mut [u8]) -> Result<(), AppAttestError> {
            if buf.is_empty() || buf.len() > MAX_CHALLENGE_BYTES {
                return Err(AppAttestError::InvalidInput);
            }
            let len = i32::try_from(buf.len()).map_err(|_| AppAttestError::InvalidInput)?;
            let bytes = call_helper(|env, class| {
                let ret = env.call_static_method(
                    class,
                    jni::jni_str!("random"),
                    jni::jni_sig!((int) -> byte[]),
                    &[JValue::Int(len)],
                )?;
                let arr = env.cast_local::<JByteArray>(ret.l()?)?;
                if arr.is_null() {
                    return Ok(None);
                }
                if arr.len(env)? != buf.len() {
                    return Ok(None);
                }
                Ok(Some(env.convert_byte_array(&arr)?))
            })
            .flatten()
            .ok_or(AppAttestError::Internal(FAILED))?;
            // 長さは検査済みだが、固定値で埋めないため一致しなければ失敗にする。
            if bytes.len() != buf.len() {
                return Err(AppAttestError::Internal(FAILED));
            }
            buf.copy_from_slice(&bytes);
            Ok(())
        }

        fn attest(
            &self,
            challenge: &Challenge,
            remaining: Duration,
        ) -> Result<Attestation, AppAttestError> {
            let ms = i64::try_from(remaining.as_millis()).unwrap_or(i64::MAX);
            if ms == 0 {
                return Err(AppAttestError::Internal(TIMEOUT));
            }
            let reply = call_helper(|env, class| {
                let j_challenge = env.byte_array_from_slice(challenge.as_bytes())?;
                let ret = env.call_static_method(
                    class,
                    jni::jni_str!("attest"),
                    jni::jni_sig!((byte[], long) -> byte[]),
                    &[
                        JValue::Object(&j_challenge),
                        JValue::Long(ms as jni::sys::jlong),
                    ],
                )?;
                let arr = env.cast_local::<JByteArray>(ret.l()?)?;
                if arr.is_null() {
                    return Ok(None);
                }
                // 長さを検証してから確保・コピーする。
                if arr.len(env)? > MAX_REPLY_BYTES {
                    return Ok(Some(Err(AppAttestError::Internal(TOO_LARGE))));
                }
                Ok(Some(Ok(env.convert_byte_array(&arr)?)))
            })
            .flatten()
            .ok_or(AppAttestError::Internal(FAILED))??;
            decode_reply(&reply)
        }
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn crate::contract::AppAttest> {
    std::sync::Arc::new(imp::AndroidAppAttest)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn crate::contract::AppAttest> {
    std::sync::Arc::new(crate::contract::UnsupportedAppAttest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{CHALLENGE_LEN, check_result, validate_challenge};
    use std::time::Duration;

    fn body(certs: &[&[u8]]) -> Vec<u8> {
        let mut v = vec![0u8];
        for c in certs {
            v.extend_from_slice(&(c.len() as u32).to_be_bytes());
            v.extend_from_slice(c);
        }
        v
    }

    #[test]
    fn decodes_chain_in_order() {
        let a = decode_reply(&body(&[&[1, 2, 3], &[4, 5]])).unwrap();
        assert_eq!(a.count(), 2);
        assert_eq!(a.total_bytes(), 5);
        assert_eq!(a.certs[0], vec![1, 2, 3]);
    }

    #[test]
    fn error_codes_never_fake_success() {
        assert!(matches!(
            decode_reply(&[1]),
            Err(AppAttestError::Internal(TIMEOUT))
        ));
        assert!(matches!(
            decode_reply(&[2]),
            Err(AppAttestError::Internal(KEYSTORE))
        ));
        assert!(matches!(
            decode_reply(&[3]),
            Err(AppAttestError::Internal(EMPTY))
        ));
        assert!(matches!(
            decode_reply(&[4]),
            Err(AppAttestError::Internal(TOO_LARGE))
        ));
        assert!(matches!(
            decode_reply(&[5]),
            Err(AppAttestError::Internal(FAILED))
        ));
    }

    #[test]
    fn malformed_replies_fail() {
        let mut short = body(&[&[1, 2, 3]]);
        short.pop();
        let mut extra = body(&[&[1, 2, 3]]);
        extra.push(9);
        let huge_len = [0u8, 0xff, 0xff, 0xff, 0xff, 1];
        for bad in [
            &[][..],
            &[0][..],
            &[99, 1][..],
            &short[..],
            &extra[..],
            &huge_len[..],
        ] {
            assert!(matches!(
                decode_reply(bad),
                Err(AppAttestError::Internal(FAILED))
            ));
        }
    }

    #[test]
    fn too_many_certs_is_too_large() {
        let one: &[u8] = &[7];
        let many = [one; MAX_CERTS + 1];
        assert!(matches!(
            decode_reply(&body(&many)),
            Err(AppAttestError::Internal(TOO_LARGE))
        ));
    }

    #[test]
    fn zero_byte_cert_is_rejected_by_driver_check() {
        let a = decode_reply(&body(&[&[]])).unwrap();
        assert!(matches!(
            check_result(a),
            Err(AppAttestError::Internal(EMPTY))
        ));
    }

    #[test]
    fn host_implementation_never_succeeds() {
        let s = implementation();
        let c = validate_challenge(&[1; CHALLENGE_LEN]).unwrap();
        assert!(matches!(
            s.attest(&c, Duration::from_secs(1)),
            Err(AppAttestError::Unsupported)
        ));
        assert!(matches!(
            s.fill_random(&mut [0; 8]),
            Err(AppAttestError::Unsupported)
        ));
    }
}
