//! Android 向けアプリ内ブラウザ（Custom Tabs）の実装（親 #135、Issue #703）。
//! `device/in_app_browser.rs` の Android 経路（dispatch）から呼ばれる。Java 側
//! `InAppBrowserHelper` が `CustomTabsIntent` で http / https の URL を開く。
//!
//! 流れ: `open` → JNI `MainActivity.requestInAppBrowserOpen(id, url)` → Java が UI スレッドで検査して
//! `nativeTryBeginPresent`（本ファイルの export が `in_app_browser::try_begin_present` を呼ぶ）で
//! 期限切れ・差し替えと排他にしてから表示 → 離脱後の復帰（`onPause` → `onResume`）で
//! `nativeOnInAppBrowserResult(id, kind)` → [`map_result`] が契約の結果へ写して
//! `in_app_browser::complete_from_os` へ 1 回だけ渡す。
//!
//! 結果コードの写像: `KIND_CLOSED` は `Ok(Closed)`、`KIND_NO_BROWSER`（`ActivityNotFoundException`）は
//! `NoBrowser`、`KIND_NO_PRESENTER`（非前景）は `NoPresenter`、`KIND_INTERRUPTED`（再生成・終了）は
//! `Interrupted`、`KIND_INVALID_URL` は `InvalidUrl`、それ以外と未知のコードは `Internal`
//! （成功へ倒さない）。`Closed` は bridge 側が表示開始を記録した要求にしか認めない。
//!
//! 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止・Activity 再生成時の旧要求の破棄
//! （`android.rs` が `reset_pending_tracking` を呼ぶ。id は単調増加で旧結果は id 不一致で捨てられる）。
//! 守らない範囲: 開いた後に Rust から閉じること・閲覧内容と読み込み失敗の検知・Custom Tabs
//! 非対応ブラウザでの表示の差・複数プロセス・プロセス死後の結果。
//! URL と OS のエラー文はログに出さない。新しい権限・依存・`<queries>` は足さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestInAppBrowserOpen` を経た後でしか
//! 呼ばれない（`web_auth_session`・`compose_mail_sms` と同じ）。

use std::sync::Arc;

use fandhe_device::in_app_browser::{
    BrowserUrl, InAppBrowser, InAppBrowserFailure, InAppBrowserOutcome,
};

/// 結果コード（Java 側 `InAppBrowserHelper.KIND_*` と対）。
const KIND_CLOSED: i32 = 0;
const KIND_NO_BROWSER: i32 = 1;
const KIND_NO_PRESENTER: i32 = 2;
const KIND_INTERRUPTED: i32 = 3;
const KIND_INVALID_URL: i32 = 4;

/// Java から届いた結果コードを契約の結果へ写像する。成功は `KIND_CLOSED` だけで、
/// 未知のコードは `Internal` にして成功へ倒さない。
fn map_result(kind: i32) -> Result<InAppBrowserOutcome, InAppBrowserFailure> {
    match kind {
        KIND_CLOSED => Ok(InAppBrowserOutcome::Closed),
        KIND_NO_BROWSER => Err(InAppBrowserFailure::NoBrowser),
        KIND_NO_PRESENTER => Err(InAppBrowserFailure::NoPresenter),
        KIND_INTERRUPTED => Err(InAppBrowserFailure::Interrupted),
        KIND_INVALID_URL => Err(InAppBrowserFailure::InvalidUrl),
        _ => Err(InAppBrowserFailure::Internal),
    }
}

/// Android の `InAppBrowser` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn InAppBrowser> {
    Arc::new(AndroidInAppBrowser)
}

struct AndroidInAppBrowser;

impl InAppBrowser for AndroidInAppBrowser {
    /// 受理は「UI スレッドへ渡した」だけを意味し、結果は後から届く。
    fn open(&self, request_id: u64, url: &BrowserUrl) -> Result<(), InAppBrowserFailure> {
        imp::open(request_id, url.as_str())
    }
}

#[cfg(target_os = "android")]
mod imp {
    use jni::objects::JValue;

    use super::super::android::JniCallFailed;
    use super::InAppBrowserFailure;

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

    pub(super) fn open(id: u64, url: &str) -> Result<(), InAppBrowserFailure> {
        if crate::android::android_app().is_none() {
            return Err(InAppBrowserFailure::NoPresenter);
        }
        call_activity(|env, a| {
            let j = env.new_string(url)?;
            env.call_method(
                a,
                jni::jni_str!("requestInAppBrowserOpen"),
                jni::jni_sig!((long, java.lang.String) -> ()),
                &[JValue::Long(id as jni::sys::jlong), JValue::Object(&j)],
            )?;
            Ok(())
        })
        .ok_or(InAppBrowserFailure::Internal)
    }
}

/// `device.rs` が Android 限定で載せるため、ホストでは使われない。呼ばれても成功へ倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
mod imp {
    use super::InAppBrowserFailure;

    pub(super) fn open(_id: u64, _url: &str) -> Result<(), InAppBrowserFailure> {
        Err(InAppBrowserFailure::Unsupported)
    }
}

/// `InAppBrowserHelper.nativeTryBeginPresent` から呼ばれる JNI エクスポート。表示の直前に呼ばれ、
/// `false`（期限切れ確定済み・古い要求）なら Java は表示しない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名で、
/// 本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
/// （既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_InAppBrowserHelper_nativeTryBeginPresent<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
) -> jni::sys::jboolean {
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let outcome = unowned_env.with_env(|_env| -> Result<bool, jni::errors::Error> {
        Ok(super::in_app_browser::try_begin_present(request_id as u64))
    });
    matches!(outcome.into_outcome(), jni::Outcome::Ok(true))
}

/// `InAppBrowserHelper.nativeOnInAppBrowserResult` から呼ばれる JNI エクスポート。結果を 1 回だけ
/// bridge へ渡し、次フレームを起こす（契約表 4 節）。URL は受け取らずログにも出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名で、
/// 既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_InAppBrowserHelper_nativeOnInAppBrowserResult<
    'caller,
>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    kind: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|_env| -> Result<(), jni::errors::Error> {
        super::in_app_browser::complete_from_os(request_id as u64, map_result(kind));
        Ok(())
    });
    let _ = outcome.into_outcome();
    // 入力が無いと egui が描画せず「処理中」で止まるため次フレームを起こす。
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::in_app_browser::validate_url;

    #[test]
    fn map_result_covers_every_code() {
        assert_eq!(map_result(0), Ok(InAppBrowserOutcome::Closed));
        assert_eq!(map_result(1), Err(InAppBrowserFailure::NoBrowser));
        assert_eq!(map_result(2), Err(InAppBrowserFailure::NoPresenter));
        assert_eq!(map_result(3), Err(InAppBrowserFailure::Interrupted));
        assert_eq!(map_result(4), Err(InAppBrowserFailure::InvalidUrl));
        assert_eq!(map_result(5), Err(InAppBrowserFailure::Internal));
    }

    #[test]
    fn unknown_codes_never_become_success() {
        for kind in [6, 99, -1, i32::MAX, i32::MIN] {
            assert_eq!(map_result(kind), Err(InAppBrowserFailure::Internal));
        }
    }

    #[test]
    fn host_open_never_succeeds() {
        let u = validate_url("https://example.com/").unwrap();
        assert!(implementation().open(1, &u).is_err());
    }
}
