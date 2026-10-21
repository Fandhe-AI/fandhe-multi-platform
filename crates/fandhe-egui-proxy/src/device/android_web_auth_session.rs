//! Android 向け Web 認証セッション実装（Issue #534、親 #113）。`device/web_auth_session.rs::implementation`
//! の Android 経路から呼ばれる。Java 側 `WebAuthSessionHelper` が Custom Tabs（開けなければ
//! `ACTION_VIEW`）で認可ページを開く。
//!
//! 流れ: `start` → JNI `MainActivity.requestWebAuthStart(id, url)` → Java が UI スレッドで起動 →
//! ブラウザが `fandhe://web-auth/callback?...` へ遷移 → 既存の intent-filter + `singleTask` の
//! `onNewIntent` で Java が確定 → `nativeOnWebAuthSessionResult(id, kind, url)` → 本ファイルが
//! [`KIND_URL`] 等を `RawResult` / `WebAuthFailure` へ写像して `web_auth_session::bridge::complete_from_os`
//! へ渡す。コールバック URL の検査（state・パス・code/error）は bridge 側 `verify_callback` が行う。
//! 乱数は Java の `SecureRandom`、SHA-256 は `MessageDigest`（`forbid(unsafe_code)` の
//! `fandhe-device` は OS の部品を持てないため OS 層が受ける）。
//!
//! 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止（未知の結果コードを成功へ倒さない）・Activity
//! 再生成時の旧要求の破棄（[`reset_pending_tracking`]）。守らない範囲: Custom Tab を閉じられないこと・
//! 既定ブラウザの UI の差・複数プロセス・プロセス死後のコールバック・他アプリによる `fandhe` scheme の
//! 横取り（PKCE で緩和）・トークン交換。URL・state・code はログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestWebAuthStart` を経た後でしか呼ばれない
//! （`compose_mail_sms` と同じ）。

use std::sync::Arc;

use fandhe_device::web_auth_session::{
    AuthRequest, RedirectKind, WebAuthFailure, WebAuthOutcome, WebAuthSession,
};

use super::web_auth_session::bridge::{self, RawResult};

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

/// Activity 再生成時に旧要求を捨てる（`android.rs::android_main` から呼ばれる）。`ACTIVE` を外して
/// デモサーバーを止め、結果の置き場も空にする。Java 側は新しいヘルパーが `NONE` から始まり対になる。
/// 旧要求の遅延コールバックは id 不一致で捨てられる。JNI は呼ばない。
pub(crate) fn reset_pending_tracking() {
    bridge::conclude_with(None, |_| Some(Ok(WebAuthOutcome::Cancelled)), None);
    bridge::SLOT.reset();
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
    use jni::objects::{JByteArray, JValue};

    use super::super::android::JniCallFailed;

    /// `MainActivity` のメソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`（呼び出しの例外は握る）。
    #[allow(unsafe_code)]
    fn call_activity<T>(
        f: impl FnOnce(&mut jni::Env, &jni::objects::JObject) -> Result<T, JniCallFailed>,
    ) -> Option<T> {
        let app = crate::android::android_app()?;
        // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
        // `app` は本呼び出しの間生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<T, JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            f(env, &activity)
        })
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
        call_activity(|env, a| {
            let ret = env.call_method(
                a,
                jni::jni_str!("webAuthRandom"),
                jni::jni_sig!((int) -> byte[]),
                &[JValue::Int(len as i32)],
            )?;
            read_bytes(env, ret)
        })
        .flatten()
    }

    pub(super) fn sha256(data: &[u8]) -> Option<Vec<u8>> {
        call_activity(|env, a| {
            let j = env.byte_array_from_slice(data)?;
            let ret = env.call_method(
                a,
                jni::jni_str!("webAuthSha256"),
                jni::jni_sig!((byte[]) -> byte[]),
                &[JValue::Object(&j)],
            )?;
            read_bytes(env, ret)
        })
        .flatten()
    }

    pub(super) fn start(id: u64, url: &str) -> bool {
        call_activity(|env, a| {
            let j = env.new_string(url)?;
            env.call_method(
                a,
                jni::jni_str!("requestWebAuthStart"),
                jni::jni_sig!((long, java.lang.String) -> ()),
                &[JValue::Long(id as jni::sys::jlong), JValue::Object(&j)],
            )?;
            Ok(())
        })
        .is_some()
    }

    pub(super) fn cancel(id: u64) {
        let _ = call_activity(|env, a| {
            env.call_method(
                a,
                jni::jni_str!("requestWebAuthCancel"),
                jni::jni_sig!((long) -> ()),
                &[JValue::Long(id as jni::sys::jlong)],
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
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_WebAuthSessionHelper_nativeOnWebAuthSessionResult<
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
        bridge::complete_from_os(request_id as u64, map_result(kind, value));
        Ok(())
    });
    let _ = outcome.into_outcome();
    // 入力が無いと egui が描画せず「処理中」で止まるため次フレームを起こす。
    crate::android::request_repaint();
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
