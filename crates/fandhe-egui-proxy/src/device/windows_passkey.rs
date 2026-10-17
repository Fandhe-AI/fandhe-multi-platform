//! Windows 向けパスキー実装（Issue #343）。`device/passkey.rs` の Windows 経路が UI スレッドで
//! [`implementation`] を呼び、返った [`Passkey`] をワーカースレッドで走らせる（`RequestSlot` による
//! 二重起動防止・世代照合は呼び出し側が持つ）。`webauthn.h`（`windows` crate の
//! `Win32::Networking::WindowsWebServices`）の `WebAuthNAuthenticatorMakeCredential` /
//! `WebAuthNAuthenticatorGetAssertion` をブロッキングで呼び、Windows Hello の UI を出す。
//! `unsafe` は FFI 呼び出しと応答のコピーに閉じ、各箇所に `// SAFETY:` を付ける（coding-rust.md）。
//!
//! # 結果の写像
//!
//! 応答は `fandhe_device::passkey::verify_response` で整合確認し、`Registered` / `AssertionReturned` に
//! する。OS のエラーは [`outcome_from_hresult`] 1 か所で DOM 例外名へ写し、
//! `outcome_from_dom_error` に渡す（キャンセル等は `NotAllowed`、非対応は `Unsupported`、未知は
//! `Internal`）。null 応答・長さ 0 は空の `Vec` にして検証へ渡し、成功に畳まない。
//! challenge・user.id・資格情報 ID・署名・HRESULT・エラー名はログにも `Debug` にも出さない。
//!
//! # 既知の天井（Issue #337 で実機確認）
//!
//! - OS は clientDataJSON を返さず、アプリが渡したものをそのまま使うため、`verify_response` の
//!   clientData 照合は Windows では循環する。要求の取り違えは世代照合で防ぐ。
//! - 署名と rpIdHash は検証しない（サーバー検証なしのデモ）。
//! - unpackaged アプリでの可否、`localhost` を RP ID にできるかは実機で確認する。
//! - 取り消しの契機（画面を離れる経路）を持たないため `WebAuthNCancelCurrentOperation` は呼ばない。
//! - webauthn.dll を持たない Windows 10 1903 未満は対象外。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 応答構造体の全経路での解放、嘘の成功の禁止。
//! 守らない: アプリ終了と OS 呼び出しの重なり、複数プロセスからの同時登録。

use std::ffi::c_void;

use windows::core::{HRESULT, HSTRING, PCWSTR};
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Win32::Foundation::HWND;
use windows::Win32::Networking::WindowsWebServices::*;

use fandhe_device::passkey::{
    client_data_json, outcome_from_dom_error, verify_response, Challenge, Passkey, PasskeyOp,
    PasskeyOutcome, CHALLENGE_LEN, DEMO_USER_NAME, NATIVE_RP_ID, RP_NAME, USER_ID_LEN,
};
use fandhe_device::DeviceError;

const FAILED: DeviceError = DeviceError::Internal("passkey failed");
/// clientDataJSON の origin。RP ID と同じ `localhost`（他者の実在ドメインを名乗らない）。
const ORIGIN: &str = "https://localhost";
/// OS 側の期限（ミリ秒）。Rust 側では期限を掛けない。
const TIMEOUT_MS: u32 = 60_000;

/// Windows の [`Passkey`] 実装を返す（UI スレッドから呼ばれる）。`HWND` は `Send` でないため
/// `isize` で控え、ワーカーで組み直す。
pub(crate) fn implementation() -> Box<dyn Passkey> {
    Box::new(WindowsPasskey {
        hwnd: super::windows_share::window().map(|h| h.0 as isize),
    })
}

struct WindowsPasskey {
    hwnd: Option<isize>,
}

/// `MakeCredential` の応答。`Drop` で必ず OS へ返す。
struct Attestation(*mut WEBAUTHN_CREDENTIAL_ATTESTATION);

impl Drop for Attestation {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: `self.0` は `WebAuthNAuthenticatorMakeCredential` が返した非 null のポインタで、
        // ここで 1 回だけ解放する。解放後は読まない。
        unsafe { WebAuthNFreeCredentialAttestation(Some(self.0)) };
    }
}

/// `GetAssertion` の応答。`Drop` で必ず OS へ返す。
struct Assertion(*mut WEBAUTHN_ASSERTION);

impl Drop for Assertion {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: `self.0` は `WebAuthNAuthenticatorGetAssertion` が返した非 null のポインタで、
        // ここで 1 回だけ解放する。解放後は読まない。
        unsafe { WebAuthNFreeAssertion(self.0) };
    }
}

/// OS が所有するバッファをコピーする。null・長さ 0 は空にし、判定は `verify_response` に任せる。
///
/// # Safety
/// `ptr` が null でなければ、`len` バイト読める有効な領域を指すこと。
#[allow(unsafe_code)]
unsafe fn copy_bytes(ptr: *const u8, len: u32) -> Vec<u8> {
    if ptr.is_null() || len == 0 {
        return Vec::new();
    }
    // SAFETY: 呼び出し側の契約どおり `ptr` は `len` バイト有効。
    unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec()
}

/// OS の CSPRNG から `N` バイトを得る。失敗や長さ違いは固定値へ倒さず失敗にする。
fn random<const N: usize>() -> Result<[u8; N], DeviceError> {
    let buf = CryptographicBuffer::GenerateRandom(N as u32).map_err(|_| FAILED)?;
    let mut arr = windows::core::Array::<u8>::new();
    CryptographicBuffer::CopyToByteArray(&buf, &mut arr).map_err(|_| FAILED)?;
    <[u8; N]>::try_from(&*arr).map_err(|_| FAILED)
}

/// OS の HRESULT を契約上の結果へ写す SSOT（登録・サインインで共有）。
/// 生の HRESULT でも DOM 例外へ写した HRESULT でも名前が取れるよう、両方を試す。
#[allow(unsafe_code)]
fn outcome_from_hresult(hr: HRESULT) -> Result<PasskeyOutcome, DeviceError> {
    // SAFETY: 引数は値渡しの HRESULT だけで、ポインタを受け取らない。
    let dom = unsafe { WebAuthNGetW3CExceptionDOMError(hr) }
        .err()
        .map(|e| e.code());
    for h in [Some(hr), dom].into_iter().flatten() {
        // SAFETY: 返るのは webauthn.dll の静的な NUL 終端文字列（null なら使わない）。
        let p = unsafe { WebAuthNGetErrorName(h) };
        if p.is_null() {
            continue;
        }
        // SAFETY: 上で null でないことを確かめた静的文字列。
        if let Ok(name) = unsafe { p.to_string() } {
            if let r @ (Ok(_) | Err(DeviceError::Unsupported(_))) = outcome_from_dom_error(&name) {
                return r;
            }
        }
    }
    Err(FAILED)
}

impl Passkey for WindowsPasskey {
    #[allow(unsafe_code)]
    fn run(&self, op: PasskeyOp) -> Result<PasskeyOutcome, DeviceError> {
        let raw = self.hwnd.ok_or(DeviceError::Internal("passkey window"))?;
        let hwnd = HWND(raw as *mut c_void);
        // SAFETY: 引数なしの版数取得。
        if unsafe { WebAuthNGetApiVersionNumber() } < WEBAUTHN_API_VERSION_1 {
            return Err(DeviceError::Unsupported("passkey"));
        }

        let challenge = Challenge::from_bytes(random::<CHALLENGE_LEN>()?);
        let json = client_data_json(op, &challenge, ORIGIN);
        let client_data = WEBAUTHN_CLIENT_DATA {
            dwVersion: WEBAUTHN_CLIENT_DATA_CURRENT_VERSION,
            cbClientDataJSON: json.len() as u32,
            pbClientDataJSON: json.as_ptr() as *mut u8,
            pwszHashAlgId: WEBAUTHN_HASH_ALGORITHM_SHA_256,
        };
        let rp_id = HSTRING::from(NATIVE_RP_ID);

        let (cred_id, auth_data, sig) = match op {
            PasskeyOp::Register => {
                let mut user_id = random::<USER_ID_LEN>()?;
                let rp_name = HSTRING::from(RP_NAME);
                let user_name = HSTRING::from(DEMO_USER_NAME);
                let rp = WEBAUTHN_RP_ENTITY_INFORMATION {
                    dwVersion: WEBAUTHN_RP_ENTITY_INFORMATION_CURRENT_VERSION,
                    pwszId: PCWSTR(rp_id.as_ptr()),
                    pwszName: PCWSTR(rp_name.as_ptr()),
                    pwszIcon: PCWSTR::null(),
                };
                let user = WEBAUTHN_USER_ENTITY_INFORMATION {
                    dwVersion: WEBAUTHN_USER_ENTITY_INFORMATION_CURRENT_VERSION,
                    cbId: USER_ID_LEN as u32,
                    pbId: user_id.as_mut_ptr(),
                    pwszName: PCWSTR(user_name.as_ptr()),
                    pwszIcon: PCWSTR::null(),
                    pwszDisplayName: PCWSTR(user_name.as_ptr()),
                };
                // Windows Hello の TPM 鍵が RS256 のことがあるため ES256 と両方を渡す。
                let mut algs = [-7, -257].map(|alg| WEBAUTHN_COSE_CREDENTIAL_PARAMETER {
                    dwVersion: WEBAUTHN_COSE_CREDENTIAL_PARAMETER_CURRENT_VERSION,
                    pwszCredentialType: WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY,
                    lAlg: alg,
                });
                let params = WEBAUTHN_COSE_CREDENTIAL_PARAMETERS {
                    cCredentialParameters: algs.len() as u32,
                    pCredentialParameters: algs.as_mut_ptr(),
                };
                // SAFETY: 全フィールドが整数かポインタの `repr(C)` 構造体で、0 は有効な初期値。
                let mut opts: WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS =
                    unsafe { std::mem::zeroed() };
                opts.dwVersion = WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS_VERSION_3;
                opts.dwTimeoutMilliseconds = TIMEOUT_MS;
                opts.dwAuthenticatorAttachment = WEBAUTHN_AUTHENTICATOR_ATTACHMENT_ANY;
                // allow list を空にしたサインイン（discoverable）のため常駐鍵にする。
                opts.bRequireResidentKey = true.into();
                opts.dwUserVerificationRequirement =
                    WEBAUTHN_USER_VERIFICATION_REQUIREMENT_REQUIRED;
                opts.dwAttestationConveyancePreference =
                    WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_NONE;
                // SAFETY: `hwnd` は `remember_window` が控えた有効なウィンドウ（`ProxyApp` の生存中は有効）。
                // rp / user / params / client_data / opts と、それが指す HSTRING・配列・`json` は
                // この呼び出しより長く同じ関数内で生きる。
                let res = unsafe {
                    WebAuthNAuthenticatorMakeCredential(
                        hwnd,
                        &rp,
                        &user,
                        &params,
                        &client_data,
                        Some(&opts),
                    )
                };
                let att = match res {
                    Ok(p) if !p.is_null() => Attestation(p),
                    Ok(_) => return Err(FAILED),
                    Err(e) => return outcome_from_hresult(e.code()),
                };
                // SAFETY: `att` は生存中で、構造体のポインタ・長さの組は OS が設定した有効な領域。
                unsafe {
                    let a = &*att.0;
                    (
                        copy_bytes(a.pbCredentialId, a.cbCredentialId),
                        copy_bytes(a.pbAuthenticatorData, a.cbAuthenticatorData),
                        None,
                    )
                }
            }
            PasskeyOp::SignIn => {
                // SAFETY: 全フィールドが整数かポインタの `repr(C)` 構造体で、0 は有効な初期値。
                let mut opts: WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS =
                    unsafe { std::mem::zeroed() };
                opts.dwVersion = WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS_VERSION_4;
                opts.dwTimeoutMilliseconds = TIMEOUT_MS;
                opts.dwAuthenticatorAttachment = WEBAUTHN_AUTHENTICATOR_ATTACHMENT_ANY;
                opts.dwUserVerificationRequirement =
                    WEBAUTHN_USER_VERIFICATION_REQUIREMENT_REQUIRED;
                // SAFETY: `hwnd` は登録側と同じ。`rp_id`・`client_data`・`opts`・`json` は呼び出しより長く生きる。
                let res = unsafe {
                    WebAuthNAuthenticatorGetAssertion(
                        hwnd,
                        PCWSTR(rp_id.as_ptr()),
                        &client_data,
                        Some(&opts),
                    )
                };
                let asr = match res {
                    Ok(p) if !p.is_null() => Assertion(p),
                    Ok(_) => return Err(FAILED),
                    Err(e) => return outcome_from_hresult(e.code()),
                };
                // SAFETY: `asr` は生存中で、構造体のポインタ・長さの組は OS が設定した有効な領域。
                unsafe {
                    let a = &*asr.0;
                    (
                        copy_bytes(a.Credential.pbId, a.Credential.cbId),
                        copy_bytes(a.pbAuthenticatorData, a.cbAuthenticatorData),
                        Some(copy_bytes(a.pbSignature, a.cbSignature)),
                    )
                }
            }
        };

        // OS は clientDataJSON を返さない。渡したバッファで照合する（モジュール doc の天井）。
        let summary = verify_response(
            op,
            &challenge,
            NATIVE_RP_ID,
            cred_id,
            json.as_bytes(),
            &auth_data,
            sig.as_deref(),
        )?;
        Ok(match op {
            PasskeyOp::Register => PasskeyOutcome::Registered(summary),
            PasskeyOp::SignIn => PasskeyOutcome::AssertionReturned(summary),
        })
    }
}
