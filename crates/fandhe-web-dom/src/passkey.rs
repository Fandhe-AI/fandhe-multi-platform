//! 型サンプル「パスキー・サインイン連携」の Web 経路（Issue #344、親 #92、共通部分は #342）。
//!
//! `fandhe_core::screens::passkey_signin`（端末依存ゼロ）の要求を [`super::mount::mount_with_effect`]
//! の effect として観測し、`navigator.credentials.create()` / `get()` でパスキーを登録・サインインして
//! 結果を `finish` へ書き戻す。サーバー検証なしのデモで、サインインの成立は意味しない。
//!
//! # `Passkey` トレイトを通らない理由
//!
//! `fandhe_device::passkey::Passkey::run` はブロッキングで、wasm のメインスレッドでは呼べない。
//! `biometric_auth.rs`・`print_page.rs` と同じく Web は effect で非同期に処理し、
//! `fandhe_device::passkey` の純関数（`outcome_from_dom_error`・`verify_response`）だけを共有する。
//!
//! # RP ID と origin
//!
//! `rp.id` / `rpId` は指定せず、ブラウザがオリジンのドメインから決める。応答の整合確認に渡す
//! RP ID は `location.hostname` で、利用者の入力や応答の値は使わない。`clientDataJSON` の
//! `origin` と `crossOrigin` は照合しない（照合するのは `type` と `challenge` だけ。サーバーを持たない
//! デモで、origin と RP ID の関係の強制はブラウザ自身が行う。既知の天井）。署名と rpIdHash も
//! 検証しない。
//!
//! # 結果の写像（三値）
//!
//! - 応答が返り `verify_response` を通った: `Registered` / `AssertionReturned`
//! - キャンセル・該当なし・取り消し（`NotAllowedError`・`AbortError`）: `NotAllowed`
//! - secure context 外・`navigator.credentials` / `PublicKeyCredential` 無し・`NotSupportedError`
//!   等: `Unsupported`
//! - 応答の不整合: 固定文言の失敗。それ以外の例外も固定文言の失敗
//!
//! 例外は `name` だけを見る。`message`・challenge・資格情報 ID・署名・`clientDataJSON` は
//! 画面にも `console` にも `localStorage` にも出さない（security.md A09）。
//!
//! # ライフサイクル
//!
//! `create` / `get` はクリックから始まる同期チェーンの中で呼ぶ（呼ぶ前に `await` を挟まない）。
//! 要求ごとに `AbortController` を持ち、マウント破棄（`release_mount`）と `pagehide`
//! （bfcache 退避を含む）で `abort()` する。取り消された Promise は `AbortError` → `NotAllowed`
//! で `finish` され、復帰後に Running のまま固まらない。同じ `(mount_id, generation)` は二度発送しない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 ページ内の要求と完了の順序（世代照合）、応答の整合確認、失敗を成功に見せないこと、
//! マウント破棄・ページ離脱での取り消し。守らない: 複数タブ・複数プロセス、署名の正しさ、
//! OS やパスワードマネージャーに残るデモ用パスキーの削除、OS 側の確定との時間差。

use fandhe_core::screens::passkey_signin::{
    CoreOutcome, CoreSummary, CredentialMemo, MISMATCH_TEXT,
};
use fandhe_device::passkey::{PasskeyOutcome, PasskeySummary};
use fandhe_device::DeviceError;

/// 失敗の固定文言（内部名・例外の内容は含めない）。
const FAILED_TEXT: &str = "パスキーの操作を完了できませんでした";

fn to_summary(s: PasskeySummary) -> CoreSummary {
    CoreSummary {
        rp_id: s.rp_id,
        credential_id: CredentialMemo(s.credential_id.as_bytes().to_vec()),
        user_present: s.user_present,
        user_verified: s.user_verified,
        signature_len: s.signature_len,
    }
}

/// `DeviceError` を画面用の結果へ写す。`fandhe-egui-proxy/src/device/passkey.rs` の
/// `to_outcome` と同期する（同クレートに依存しないため同じ写像をここに置く）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn to_outcome(result: Result<PasskeyOutcome, DeviceError>) -> Result<CoreOutcome, String> {
    match result {
        Ok(PasskeyOutcome::Registered(s)) => Ok(CoreOutcome::Registered(to_summary(s))),
        Ok(PasskeyOutcome::AssertionReturned(s)) => {
            Ok(CoreOutcome::AssertionReturned(to_summary(s)))
        }
        Ok(PasskeyOutcome::NotAllowed) => Ok(CoreOutcome::NotAllowed),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(DeviceError::InvalidInput(_)) => Err(MISMATCH_TEXT.to_string()),
        Err(_) => Err(FAILED_TEXT.to_string()),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::passkey_signin::{
        finish, mark_running, take_request, PasskeyAction, PasskeySigninState,
    };
    use fandhe_device::passkey::{
        outcome_from_dom_error, verify_response, Challenge, PasskeyOp, PasskeyOutcome,
        CHALLENGE_LEN, DEMO_USER_NAME, RP_NAME, USER_ID_LEN,
    };
    use fandhe_device::DeviceError;
    use js_sys::{Array, Reflect, Uint8Array};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        AbortController, AuthenticatorAssertionResponse, AuthenticatorAttestationResponse,
        AuthenticatorResponse, AuthenticatorSelectionCriteria, CredentialCreationOptions,
        CredentialRequestOptions, PublicKeyCredential, PublicKeyCredentialCreationOptions,
        PublicKeyCredentialParameters, PublicKeyCredentialRequestOptions,
        PublicKeyCredentialRpEntity, PublicKeyCredentialType, PublicKeyCredentialUserEntity,
        UserVerificationRequirement,
    };

    use super::to_outcome;
    use crate::mount::Applier;

    /// 発送済みの（世代, 取り消し用コントローラ）。同じ世代を二重に発送しない。
    type Entry = (u64, Option<AbortController>);

    /// `pagehide` 購読クロージャの型。
    type PagehideClosure = Closure<dyn FnMut(web_sys::Event)>;

    thread_local! {
        static DISPATCHED: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
        /// `pagehide` 購読（初回の要求で 1 度だけ登録し、ページの寿命まで持つ）。
        static PAGEHIDE: RefCell<Option<PagehideClosure>> =
            const { RefCell::new(None) };
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に飛行中の要求を取り消し、記録を解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        if let Some((_, Some(ctrl))) = DISPATCHED.with(|d| d.borrow_mut().remove(&mount_id)) {
            ctrl.abort();
        }
    }

    /// 全マウントの飛行中の要求を取り消す（`pagehide`）。記録は残す（復帰後の二重発送防止）。
    fn abort_all() {
        let ctrls: Vec<AbortController> = DISPATCHED.with(|d| {
            d.borrow_mut()
                .values_mut()
                .filter_map(|e| e.1.take())
                .collect()
        });
        for c in ctrls {
            c.abort();
        }
    }

    fn register_pagehide_once(w: &web_sys::Window) {
        PAGEHIDE.with(|cell| {
            if cell.borrow().is_some() {
                return;
            }
            let closure =
                Closure::<dyn FnMut(web_sys::Event)>::new(|_e: web_sys::Event| abort_all());
            if w.add_event_listener_with_callback("pagehide", closure.as_ref().unchecked_ref())
                .is_ok()
            {
                *cell.borrow_mut() = Some(closure);
            }
        });
    }

    fn has_property(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|v| !v.is_undefined() && !v.is_null())
            .unwrap_or(false)
    }

    /// 例外の `name` だけを結果へ写す（`message` は読まない）。無ければ `Internal` になる。
    fn outcome_of_error(err: &JsValue) -> Result<PasskeyOutcome, DeviceError> {
        let name = Reflect::get(err, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        outcome_from_dom_error(&name)
    }

    fn random(w: &web_sys::Window, buf: &mut [u8]) -> bool {
        w.crypto()
            .ok()
            .is_some_and(|c| c.get_random_values_with_u8_array(buf).is_ok())
    }

    /// 応答を取り出して整合確認を通す。通ったときだけ成功を返す。
    fn check(
        op: PasskeyOp,
        challenge: &Challenge,
        rp_id: &str,
        cred: &PublicKeyCredential,
    ) -> Result<PasskeyOutcome, DeviceError> {
        const BAD: DeviceError = DeviceError::InvalidInput("passkey flags");
        let response: AuthenticatorResponse = cred.response();
        let client_data = Uint8Array::new(&response.client_data_json()).to_vec();
        let raw_id = Uint8Array::new(&cred.raw_id()).to_vec();
        let (auth_data, signature) = match op {
            PasskeyOp::Register => {
                let r: &AuthenticatorAttestationResponse = response.unchecked_ref();
                // 取れない古いブラウザは失敗にする（成功にしない）。
                let buf = r.get_authenticator_data().map_err(|_| BAD)?;
                (Uint8Array::new(&buf).to_vec(), None)
            }
            PasskeyOp::SignIn => {
                let r: &AuthenticatorAssertionResponse = response.unchecked_ref();
                (
                    Uint8Array::new(&r.authenticator_data()).to_vec(),
                    Some(Uint8Array::new(&r.signature()).to_vec()),
                )
            }
        };
        let summary = verify_response(
            op,
            challenge,
            rp_id,
            raw_id,
            &client_data,
            &auth_data,
            signature.as_deref(),
        )?;
        Ok(match op {
            PasskeyOp::Register => PasskeyOutcome::Registered(summary),
            PasskeyOp::SignIn => PasskeyOutcome::AssertionReturned(summary),
        })
    }

    /// 完了した世代のコントローラを外す（世代が違えば新しい要求のものなので触らない）。
    fn clear(mount_id: u64, generation: u64) {
        DISPATCHED.with(|d| {
            if let Some(e) = d.borrow_mut().get_mut(&mount_id) {
                if e.0 == generation {
                    e.1 = None;
                }
            }
        });
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ OS の UI を依頼する。
    pub fn passkey_signin_effect(
        state: &PasskeySigninState,
        applier: &Applier<PasskeySigninState>,
    ) {
        let Some(view) = take_request(state) else {
            return;
        };
        let generation = view.generation;
        let mount_id = applier.mount_id();
        let fresh = DISPATCHED.with(|d| {
            let mut d = d.borrow_mut();
            if d.get(&mount_id).map(|e| e.0) == Some(generation) {
                return false;
            }
            d.insert(mount_id, (generation, None));
            true
        });
        if !fresh {
            return;
        }
        applier.apply(mark_running);

        let done = move |applier: &Applier<PasskeySigninState>,
                         r: Result<PasskeyOutcome, DeviceError>| {
            let out = to_outcome(r);
            applier.apply(move |s| finish(s, generation, out));
        };
        let unsupported = DeviceError::Unsupported("passkey");
        let internal = DeviceError::Internal("passkey failed");

        let Some(window) = web_sys::window() else {
            return done(applier, Err(unsupported));
        };
        let nav_value: JsValue = window.navigator().into();
        let win_value: JsValue = window.clone().into();
        if !window.is_secure_context()
            || !has_property(&nav_value, "credentials")
            || !has_property(&win_value, "PublicKeyCredential")
        {
            return done(applier, Err(unsupported));
        }
        // RP ID はページの origin から決める。取れなければ成功にしない。
        let Some(rp_id) = window.location().hostname().ok().filter(|h| !h.is_empty()) else {
            return done(applier, Err(internal));
        };

        // バッファは create/get の呼び出しが返るまで生かす（wasm メモリのビューを渡すため）。
        let mut challenge = [0u8; CHALLENGE_LEN];
        let mut user_id = [0u8; USER_ID_LEN];
        if !random(&window, &mut challenge) || !random(&window, &mut user_id) {
            return done(applier, Err(internal));
        }
        let expected = Challenge::from_bytes(challenge);

        let Ok(ctrl) = AbortController::new() else {
            return done(applier, Err(internal));
        };
        DISPATCHED.with(|d| {
            if let Some(e) = d.borrow_mut().get_mut(&mount_id) {
                e.1 = Some(ctrl.clone());
            }
        });
        register_pagehide_once(&window);

        let credentials = window.navigator().credentials();
        let (op, promise) = match view.action {
            PasskeyAction::Register => {
                let rp = PublicKeyCredentialRpEntity::new(RP_NAME);
                let user = PublicKeyCredentialUserEntity::new_with_u8_slice(
                    DEMO_USER_NAME,
                    DEMO_USER_NAME,
                    &mut user_id,
                );
                let params = Array::new();
                params.push(&PublicKeyCredentialParameters::new(
                    -7,
                    PublicKeyCredentialType::PublicKey,
                ));
                params.push(&PublicKeyCredentialParameters::new(
                    -257,
                    PublicKeyCredentialType::PublicKey,
                ));
                let selection = AuthenticatorSelectionCriteria::new();
                selection.set_user_verification(UserVerificationRequirement::Required);
                selection.set_resident_key("required");
                selection.set_require_resident_key(true);
                let options = PublicKeyCredentialCreationOptions::new_with_u8_slice(
                    &mut challenge,
                    &params.into(),
                    &rp,
                    &user,
                );
                options.set_authenticator_selection(&selection);
                let req = CredentialCreationOptions::new();
                req.set_public_key(&options);
                req.set_signal(&ctrl.signal());
                (PasskeyOp::Register, credentials.create_with_options(&req))
            }
            PasskeyAction::SignIn => {
                let options = PublicKeyCredentialRequestOptions::new_with_u8_slice(&mut challenge);
                options.set_user_verification(UserVerificationRequirement::Required);
                let req = CredentialRequestOptions::new();
                req.set_public_key(&options);
                req.set_signal(&ctrl.signal());
                (PasskeyOp::SignIn, credentials.get_with_options(&req))
            }
        };

        let promise = match promise {
            Ok(p) => p,
            Err(err) => {
                clear(mount_id, generation);
                return done(applier, outcome_of_error(&err));
            }
        };

        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match JsFuture::from(promise).await {
                Ok(v) => match v.dyn_into::<PublicKeyCredential>() {
                    Ok(cred) => check(op, &expected, &rp_id, &cred),
                    Err(_) => Err(DeviceError::Internal("passkey failed")),
                },
                Err(err) => outcome_of_error(&err),
            };
            clear(mount_id, generation);
            done(&applier, result);
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::passkey_signin_effect;

/// マウント破棄時に飛行中の要求を取り消す。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::passkey::{outcome_from_dom_error, CredentialId};

    fn summary() -> PasskeySummary {
        PasskeySummary {
            rp_id: "localhost".into(),
            credential_id: CredentialId::from_bytes(vec![1, 2]),
            user_present: true,
            user_verified: true,
            signature_len: Some(64),
        }
    }

    #[test]
    fn maps_every_arm() {
        assert!(matches!(
            to_outcome(Ok(PasskeyOutcome::Registered(summary()))),
            Ok(CoreOutcome::Registered(s)) if s.credential_id.0 == vec![1, 2] && s.rp_id == "localhost"
        ));
        assert!(matches!(
            to_outcome(Ok(PasskeyOutcome::AssertionReturned(summary()))),
            Ok(CoreOutcome::AssertionReturned(_))
        ));
        assert_eq!(
            to_outcome(Ok(PasskeyOutcome::NotAllowed)),
            Ok(CoreOutcome::NotAllowed)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("passkey"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::InvalidInput("passkey flags"))),
            Err(MISMATCH_TEXT.to_string())
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Internal("passkey failed"))),
            Err(FAILED_TEXT.to_string())
        );
    }

    #[test]
    fn failure_text_hides_internal_names() {
        assert!(!FAILED_TEXT.contains("passkey"));
        assert!(!MISMATCH_TEXT.contains("passkey"));
    }

    #[test]
    fn dom_errors_never_succeed() {
        for name in [
            "NotAllowedError",
            "AbortError",
            "SecurityError",
            "Weird",
            "",
        ] {
            let r = to_outcome(outcome_from_dom_error(name));
            assert!(!matches!(
                r,
                Ok(CoreOutcome::Registered(_)) | Ok(CoreOutcome::AssertionReturned(_))
            ));
        }
    }
}
