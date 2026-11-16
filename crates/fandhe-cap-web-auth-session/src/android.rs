//! Android 向け Web 認証セッション実装（Issue #534、親 #113）。`driver::platform_web_auth_session`
//! の Android 経路から呼ばれる。Java 側 `WebAuthSessionHelper` が Custom Tabs（開けなければ
//! `ACTION_VIEW`）で認可ページを開く。
//!
//! 流れ: `start` → JNI `WebAuthSessionHelper.requestStart(activity, id, url)`（static）→ Java が UI スレッドで起動 →
//! ブラウザが `fandhe://web-auth/callback?...` へ遷移 → 既存の intent-filter + `singleTask` の
//! `onNewIntent`（`WebAuthSessionLifecycle` が足したリスナー）で Java が確定 → `nativeOnWebAuthSessionResult(id, kind, url)` → 本ファイルが
//! [`KIND_URL`] 等を `RawResult` / `WebAuthFailure` へ写像して `web_auth_session::bridge::complete_from_os`
//! へ渡す。コールバック URL の検査（state・パス・code/error）は bridge 側 `verify_callback` が行う。
//! 乱数は Java の `SecureRandom`、SHA-256 は `MessageDigest`（`forbid(unsafe_code)` の
//! `fandhe-device` は OS の部品を持てないため OS 層が受ける）。
//!
//! ライフサイクル（引き継ぎ型）: 認証中に Activity が再生成されても、要求の状態は Java 側が static に持ち、
//! コールバックは新しい Activity の `onNewIntent`（singleTask）へ届いて確定する。Rust 側の `ACTIVE`・`SLOT` は残したままにし、
//! 呼び出し側の掃除は要らない。結果が返らない間（Activity が再生成されずに終了・プロセス死）は期限
//! （[`crate::SESSION_TIMEOUT`]）が期限切れとして確定する（期限の判定は取り出す側の呼び出しで行う）。
//!
//! 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止（未知の結果コードを成功へ倒さない）。守らない範囲: Custom Tab を閉じられないこと・
//! 既定ブラウザの UI の差・複数プロセス・プロセス死後のコールバック・他アプリによる `fandhe` scheme の
//! 横取り（PKCE で緩和）・トークン交換。URL・state・code はログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestWebAuthStart` を経た後でしか呼ばれない
//! （`compose_mail_sms` と同じ）。

use std::sync::Arc;

use crate::contract::{AuthRequest, RedirectKind, WebAuthFailure, WebAuthSession};

use crate::driver::bridge::RawResult;

/// 結果コード（Java 側 `WebAuthSessionHelper.KIND_*` と対）。
const KIND_URL: i32 = 0;
const KIND_CANCELLED: i32 = 1;
const KIND_NO_BROWSER: i32 = 2;

/// Java から届いた結果コードと URL を契約の結果へ写像する。URL を伴うのに値がない場合と
/// 未知のコードは `Internal` にし、成功へ倒さない。
fn map_result(kind: i32, url: Option<String>) -> Result<RawResult, WebAuthFailure> {
    match (kind, url) {
        (KIND_URL, Some(u)) => Ok(RawResult::Url(u)),
        (KIND_CANCELLED, _) => Ok(RawResult::Cancelled),
        (KIND_NO_BROWSER, _) => Err(WebAuthFailure::NoBrowser),
        _ => Err(WebAuthFailure::Internal),
    }
}

/// Android の `WebAuthSession` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn WebAuthSession> {
    Arc::new(AndroidWebAuthSession)
}

struct AndroidWebAuthSession;

impl WebAuthSession for AndroidWebAuthSession {
    fn redirect_kind(&self) -> RedirectKind {
        RedirectKind::CustomScheme
    }

    fn fill_random(&self, buf: &mut [u8]) -> Result<(), WebAuthFailure> {
        let bytes = imp::random(buf.len()).ok_or(WebAuthFailure::Internal)?;
        if bytes.len() != buf.len() {
            return Err(WebAuthFailure::Internal);
        }
        buf.copy_from_slice(&bytes);
        Ok(())
    }

    fn sha256(&self, data: &[u8]) -> Result<[u8; 32], WebAuthFailure> {
        imp::sha256(data)
            .and_then(|v| <[u8; 32]>::try_from(v).ok())
            .ok_or(WebAuthFailure::Internal)
    }

    /// 受理は「UI スレッドへ渡した」だけを意味し、結果は後から届く。
    fn start(&self, request_id: u64, req: &AuthRequest) -> Result<(), WebAuthFailure> {
        if imp::start(request_id, &req.start_url) {
            Ok(())
        } else {
            Err(WebAuthFailure::Internal)
        }
    }

    /// 失敗は握る（冪等・開いた Custom Tab は閉じられない OS の扱い）。
    fn cancel(&self, request_id: u64) {
        imp::cancel(request_id);
    }
}

#[cfg(target_os = "android")]
mod imp {
    use fandhe_platform_android::{JniCallFailed, with_activity_class};
    use jni::objects::{JByteArray, JClass, JObject, JValue};

    /// `WebAuthSessionHelper` の static メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`（呼び出しの例外は握る）。
    /// `unsafe` は共通層 `fandhe_platform_android::with_activity_class` に閉じており、本ファイルには無い。
    fn call_helper<T>(
        f: impl FnOnce(&mut jni::Env, &JObject, &JClass) -> Result<T, JniCallFailed>,
    ) -> Option<T> {
        let app = fandhe_platform_android::android_app()?;
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.WebAuthSessionHelper"),
            |env, activity, class| {
                let r = f(env, activity, class);
                if r.is_err() && env.exception_check() {
                    env.exception_clear();
                }
                r
            },
        )
        .ok()
    }

    /// `byte[]` を返す Java メソッド共通の後処理。null は `None`。
    fn read_bytes(
        env: &mut jni::Env,
        ret: jni::objects::JValueOwned,
    ) -> Result<Option<Vec<u8>>, JniCallFailed> {
        let arr = env.cast_local::<JByteArray>(ret.l()?)?;
        if arr.is_null() {
            return Ok(None);
        }
        Ok(Some(env.convert_byte_array(&arr)?))
    }

    pub(super) fn random(len: usize) -> Option<Vec<u8>> {
        call_helper(|env, _a, class| {
            let ret = env.call_static_method(
                class,
                jni::jni_str!("random"),
                jni::jni_sig!((int) -> byte[]),
                &[JValue::Int(len as i32)],
            )?;
            read_bytes(env, ret)
        })
        .flatten()
    }

    pub(super) fn sha256(data: &[u8]) -> Option<Vec<u8>> {
        call_helper(|env, _a, class| {
            let j = env.byte_array_from_slice(data)?;
            let ret = env.call_static_method(
                class,
                jni::jni_str!("sha256"),
                jni::jni_sig!((byte[]) -> byte[]),
                &[JValue::Object(&j)],
            )?;
            read_bytes(env, ret)
        })
        .flatten()
    }

    /// Helper が無い（Activity の生成前・破棄後）なら Java が `false` を返す。受理しない要求は失敗で確定させる。
    pub(super) fn start(id: u64, url: &str) -> bool {
        call_helper(|env, a, class| {
            let j = env.new_string(url)?;
            let ret = env.call_static_method(
                class,
                jni::jni_str!("requestStart"),
                jni::jni_sig!((android.app.Activity, long, java.lang.String) -> boolean),
                &[
                    JValue::Object(a),
                    JValue::Long(id as jni::sys::jlong),
                    JValue::Object(&j),
                ],
            )?;
            Ok(ret.z()?)
        })
        .unwrap_or(false)
    }

    pub(super) fn cancel(id: u64) {
        let _ = call_helper(|env, a, class| {
            env.call_static_method(
                class,
                jni::jni_str!("requestCancel"),
                jni::jni_sig!((android.app.Activity, long) -> ()),
                &[JValue::Object(a), JValue::Long(id as jni::sys::jlong)],
            )?;
            Ok(())
        });
    }
}

/// `device.rs` が Android 限定で載せるため、ホストでは使われない。呼ばれても成功へ倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
mod imp {
    pub(super) fn random(_len: usize) -> Option<Vec<u8>> {
        None
    }
    pub(super) fn sha256(_data: &[u8]) -> Option<Vec<u8>> {
        None
    }
    pub(super) fn start(_id: u64, _url: &str) -> bool {
        false
    }
    pub(super) fn cancel(_id: u64) {}
}

/// `WebAuthSessionHelper.nativeOnWebAuthSessionResult` から呼ばれる JNI エクスポート。
/// URL・state・code は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` による panic の遮断込み）を担う。`#[no_mangle]` は JNI 命名規約上の
/// 固定シンボル名で、既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_WebAuthSessionHelper_nativeOnWebAuthSessionResult<
    'caller,
>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    kind: jni::sys::jint,
    url: jni::objects::JString<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let value = if url.is_null() {
            None
        } else {
            url.try_to_string(env).ok()
        };
        crate::driver::bridge::complete_from_os(request_id as u64, map_result(kind, value));
        Ok(())
    });
    let _ = outcome.into_outcome();
    // 入力が無いと egui が描画せず「処理中」で止まるため次フレームを起こす。
    fandhe_runtime::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_result_never_folds_failure_into_success() {
        assert!(matches!(
            map_result(0, Some("u".into())),
            Ok(RawResult::Url(_))
        ));
        assert!(matches!(map_result(0, None), Err(WebAuthFailure::Internal)));
        assert!(matches!(map_result(1, None), Ok(RawResult::Cancelled)));
        assert!(matches!(
            map_result(2, None),
            Err(WebAuthFailure::NoBrowser)
        ));
        assert!(matches!(map_result(3, None), Err(WebAuthFailure::Internal)));
        assert!(matches!(
            map_result(99, None),
            Err(WebAuthFailure::Internal)
        ));
    }
}
