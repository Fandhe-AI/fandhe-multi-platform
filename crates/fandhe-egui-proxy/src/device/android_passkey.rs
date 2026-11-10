//! Issue #1280（親 #92）: Android 向けパスキー（Credential Manager）の実装。`device/passkey.rs` の
//! Android 経路（`drive_callback`）から呼ばれ、パスキーの登録・サインイン要求を出す。Java 側は
//! `PasskeyHelper`（`androidx.credentials`）。契約の SSOT は `fandhe_device::passkey`。
//!
//! 流れ: `begin(id, op)` → RP ID を検証 → 同期 JNI `MainActivity.passkeyRandom` で challenge 32 + user.id 16
//! （`SecureRandom`）→ [`PENDING`] に要求を置く → JNI `MainActivity.requestPasskey(id, register, json)` →
//! Java が結果の種別と本文を `nativeOnPasskeyResult` で返す → [`decode_reply`] で契約の結果へ写像 →
//! `passkey::bridge::deliver`。要求 JSON の組み立てと応答の照合は Rust だけで行い、Java に定数を複製しない。
//!
//! RP ID: ビルド時の環境変数 `FANDHE_ANDROID_PASSKEY_RP_ID`（値は本リポジトリに書かない）。未設定・不正
//! （[`valid_rp_id`]）は `Unsupported` で、OS は呼ばない。
//!
//! 写像: kind 0 の応答は `verify_response`（challenge・type・UP/UV/AT・署名の有無）を通ったものだけが成功。
//! Android では clientDataJSON をプロバイダが組み立てて返すため、`check_client_data` は取り違えの検出として働く
//! （origin の `android:apk-key-hash:…` は照合しない）。登録の authData は `attestationObject`（CBOR）から、
//! サインインは `authenticatorData` から取り出す。取れなければ `InvalidInput("passkey flags")`（嘘の成功にはしない）。
//!
//! ライフサイクル: Activity 再生成（`onDestroy`）で Java が追跡を外して `CancellationSignal` を取り消し、結果は送らない。
//! Rust 側は `passkey::reset_pending_tracking()` が slot と [`PENDING`] を捨てる。id は単調増加で、旧結果は
//! 新しい要求を消費しない。期限は掛けない（利用者操作の UI で OS の期限に従う）。
//!
//! 守る範囲: 1 プロセス内の要求と完了の順序・古い世代の破棄・嘘の成功の禁止。守らない範囲: プロセス死を
//! またぐ結果・Credential Manager のコールバックが返らない場合の処理中表示・複数プロセス・
//! パスワードマネージャーの一覧への反映の時間差・署名と rpIdHash の検証。
//! challenge・user.id・資格情報 ID・応答 JSON はログに出さない（security.md A09）。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestPasskey` を経た後でしか呼ばれない
//! （android_payment_request と同じ）。

use std::sync::Mutex;

use fandhe_device::passkey::{
    base64url_decode, base64url_encode, outcome_from_dom_error, verify_response, Challenge,
    PasskeyOp, PasskeyOutcome, CHALLENGE_LEN, DEMO_USER_NAME, RP_NAME, USER_ID_LEN,
};
use fandhe_device::DeviceError;

/// 結果の種別（SSOT は Java 側 `PasskeyHelper.KIND_*`）。
const KIND_RESPONSE: i32 = 0;
const KIND_NOT_ALLOWED: i32 = 1;
const KIND_DOM_ERROR: i32 = 2;
const KIND_UNSUPPORTED: i32 = 3;
// KIND_FAILED（4）と未知の値は、写像の既定（失敗）に入るため定数にしない。

/// 応答 JSON・DOM 名の UTF-16 単位数の上限。超えたら読まずに拒否する。
#[cfg(any(target_os = "android", test))]
const PAYLOAD_MAX_UNITS: i32 = 64 * 1024;
/// 応答 JSON のバイト上限。解析前に検査する。
const RESPONSE_MAX_BYTES: usize = 64 * 1024;

const FAILED: DeviceError = DeviceError::Internal("passkey failed");
const BAD_CLIENT_DATA: DeviceError = DeviceError::InvalidInput("passkey client data");
const BAD_FLAGS: DeviceError = DeviceError::InvalidInput("passkey flags");

/// ビルド時に渡す RP ID。値は本リポジトリに書かない（public-repo.md）。
#[cfg(any(target_os = "android", test))]
const RP_ID: Option<&str> = option_env!("FANDHE_ANDROID_PASSKEY_RP_ID");

/// 保留中の要求（同時に 1 件）。Java へ渡す前に置き、結果の照合に使う。
struct Pending {
    id: u64,
    op: PasskeyOp,
    challenge: Challenge,
    rp_id: String,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// 保留中の要求を id 照合で取り出す。不一致なら取り出さず `None`（古い世代の結果を捨てる）。
fn take_pending(id: u64) -> Option<Pending> {
    let mut g = PENDING.lock().ok()?;
    match g.as_ref() {
        Some(p) if p.id == id => g.take(),
        _ => None,
    }
}

/// 保留を捨てる（Activity 再生成・失敗経路）。
pub(crate) fn reset() {
    if let Ok(mut g) = PENDING.lock() {
        *g = None;
    }
}

/// RP ID として通す文字列か。ASCII 小文字・数字・ハイフンのラベル 2 個以上（各 1〜63 文字、
/// 先頭末尾はハイフン不可）、全体 253 文字以下。末尾ラベルが数字だけ（IPv4）と `localhost` は拒否する。
fn valid_rp_id(s: &str) -> bool {
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = s.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let ok = labels.iter().all(|l| {
        (1..=63).contains(&l.len())
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    });
    ok && !labels
        .last()
        .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
}

/// 要求 JSON（Web の `fandhe-web-dom/src/passkey.rs` と同じ項目）。`json!` で組み、文字列連結はしない。
fn request_json(
    op: PasskeyOp,
    challenge: &Challenge,
    user_id: &[u8; USER_ID_LEN],
    rp_id: &str,
) -> String {
    let challenge = base64url_encode(challenge.as_bytes());
    match op {
        PasskeyOp::Register => serde_json::json!({
            "challenge": challenge,
            "rp": { "id": rp_id, "name": RP_NAME },
            "user": {
                "id": base64url_encode(user_id),
                "name": DEMO_USER_NAME,
                "displayName": DEMO_USER_NAME,
            },
            "pubKeyCredParams": [
                { "type": "public-key", "alg": -7 },
                { "type": "public-key", "alg": -257 },
            ],
            "authenticatorSelection": {
                "residentKey": "required",
                "requireResidentKey": true,
                "userVerification": "required",
            },
            "attestation": "none",
        }),
        PasskeyOp::SignIn => serde_json::json!({
            "challenge": challenge,
            "rpId": rp_id,
            "allowCredentials": [],
            "userVerification": "required",
        }),
    }
    .to_string()
}

/// JSON の文字列値を base64url として復号する。末尾の `=` だけ許容する
/// （プロバイダの出力差で常に失敗する実装を避ける）。
fn b64(v: &serde_json::Value, path: &[&str]) -> Option<Vec<u8>> {
    let mut cur = v;
    for k in path {
        cur = cur.get(k)?;
    }
    base64url_decode(cur.as_str()?.trim_end_matches('='))
}

/// CBOR の先頭ヘッダを読み、(major type, 引数, 読み進めた長さ) を返す。不定長・予約値は `None`。
fn cbor_head(b: &[u8]) -> Option<(u8, u64, usize)> {
    let first = *b.first()?;
    let n = match first & 0x1f {
        v @ 0..=23 => return Some((first >> 5, u64::from(v), 1)),
        24 => 1,
        25 => 2,
        26 => 4,
        27 => 8,
        _ => return None,
    };
    let arg = b
        .get(1..=n)?
        .iter()
        .fold(0u64, |acc, x| (acc << 8) | u64::from(*x));
    Some((first >> 5, arg, 1 + n))
}

/// CBOR の 1 要素を読み飛ばし、その長さを返す。深さは `depth` で制限する。
fn cbor_skip(b: &[u8], depth: u8) -> Option<usize> {
    let (major, arg, mut off) = cbor_head(b)?;
    match major {
        0 | 1 | 7 => {}
        2 | 3 => off = off.checked_add(usize::try_from(arg).ok()?)?,
        4 | 5 => {
            if depth == 0 {
                return None;
            }
            let count = if major == 5 { arg.checked_mul(2)? } else { arg };
            for _ in 0..count {
                off += cbor_skip(b.get(off..)?, depth - 1)?;
            }
        }
        6 => off += cbor_skip(b.get(off..)?, depth.checked_sub(1)?)?,
        _ => return None,
    }
    (off <= b.len()).then_some(off)
}

/// attestationObject（キー `fmt`・`attStmt`・`authData` の CBOR マップ）から `authData` を取り出す。
fn auth_data_from_attestation(b: &[u8]) -> Option<Vec<u8>> {
    let (major, count, mut off) = cbor_head(b)?;
    if major != 5 {
        return None;
    }
    for _ in 0..count {
        let (km, klen, koff) = cbor_head(b.get(off..)?)?;
        let key_end = off
            .checked_add(koff)?
            .checked_add(usize::try_from(klen).ok()?)?;
        let key = b.get(off + koff..key_end)?;
        let is_text = km == 3;
        off = key_end;
        let (vm, vlen, voff) = cbor_head(b.get(off..)?)?;
        if is_text && key == b"authData" {
            if vm != 2 {
                return None;
            }
            let start = off.checked_add(voff)?;
            let end = start.checked_add(usize::try_from(vlen).ok()?)?;
            return b.get(start..end).map(<[u8]>::to_vec);
        }
        off += cbor_skip(b.get(off..)?, 8)?;
    }
    None
}

/// 応答 JSON（`registrationResponseJson` / `authenticationResponseJson`）を整合確認して結果にする。
fn parse_response(
    op: PasskeyOp,
    challenge: &Challenge,
    rp_id: &str,
    json: &str,
) -> Result<PasskeyOutcome, DeviceError> {
    if json.len() > RESPONSE_MAX_BYTES {
        return Err(BAD_CLIENT_DATA);
    }
    let v: serde_json::Value = serde_json::from_str(json).map_err(|_| BAD_CLIENT_DATA)?;
    let raw_id = b64(&v, &["rawId"]).unwrap_or_default();
    let client_data = b64(&v, &["response", "clientDataJSON"]).ok_or(BAD_CLIENT_DATA)?;
    // 登録応答の authData は attestationObject（CBOR）の中。サインインは authenticatorData を直接持つ。
    let auth_data = match op {
        PasskeyOp::Register => {
            b64(&v, &["response", "attestationObject"]).and_then(|a| auth_data_from_attestation(&a))
        }
        PasskeyOp::SignIn => b64(&v, &["response", "authenticatorData"]),
    }
    .ok_or(BAD_FLAGS)?;
    let signature = b64(&v, &["response", "signature"]);
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

/// Java からの結果を契約の結果へ写す。成功になるのは `KIND_RESPONSE` で整合確認を通ったときだけ。
fn decode_reply(
    p: &Pending,
    kind: i32,
    payload: Option<&str>,
) -> Result<PasskeyOutcome, DeviceError> {
    match kind {
        KIND_RESPONSE => match payload {
            Some(json) => parse_response(p.op, &p.challenge, &p.rp_id, json),
            None => Err(BAD_CLIENT_DATA),
        },
        KIND_NOT_ALLOWED => Ok(PasskeyOutcome::NotAllowed),
        KIND_DOM_ERROR => outcome_from_dom_error(payload.unwrap_or("")),
        KIND_UNSUPPORTED => Err(DeviceError::Unsupported("passkey")),
        // KIND_FAILED・未知の値。
        _ => Err(FAILED),
    }
}

/// RP ID を検証し、乱数から [`PENDING`] と要求 JSON を作る。`Err` のとき [`PENDING`] は触らない。
/// `random` は challenge 32 + user.id 16 バイト。長さが違えば失敗にする（固定値で埋めない）。
#[cfg(any(target_os = "android", test))]
fn prepare(
    id: u64,
    op: PasskeyOp,
    rp_id: Option<&str>,
    random: &[u8],
) -> Result<String, DeviceError> {
    let rp_id = rp_id
        .filter(|s| valid_rp_id(s))
        .ok_or(DeviceError::Unsupported("passkey"))?;
    if random.len() != CHALLENGE_LEN + USER_ID_LEN {
        return Err(FAILED);
    }
    let challenge = Challenge::from_bytes(random[..CHALLENGE_LEN].try_into().map_err(|_| FAILED)?);
    let user_id: [u8; USER_ID_LEN] = random[CHALLENGE_LEN..].try_into().map_err(|_| FAILED)?;
    let json = request_json(op, &challenge, &user_id, rp_id);
    *PENDING.lock().map_err(|_| FAILED)? = Some(Pending {
        id,
        op,
        challenge,
        rp_id: rp_id.to_string(),
    });
    Ok(json)
}

/// OS へ依頼する。`Ok` は受理済みで結果は後から `bridge::deliver` で届く。`Err` は呼び出し自体の失敗で、
/// 呼び出し元がその場で結果を置く。
#[cfg(target_os = "android")]
pub(crate) fn begin(request_id: u64, op: PasskeyOp) -> Result<(), DeviceError> {
    // 未設定・不正な RP ID は乱数を作る前に弾く（OS を呼ばない）。
    if !RP_ID.is_some_and(valid_rp_id) {
        return Err(DeviceError::Unsupported("passkey"));
    }
    let random = call_random().ok_or(FAILED)?;
    let json = prepare(request_id, op, RP_ID, &random)?;
    if call_request(request_id, matches!(op, PasskeyOp::Register), &json) {
        Ok(())
    } else {
        reset();
        Err(FAILED)
    }
}

/// ホストのテストビルド用。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
pub(crate) fn begin(_request_id: u64, _op: PasskeyOp) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("passkey"))
}

/// `MainActivity.passkeyRandom(int)`（同期 JNI）で `SecureRandom` の 48 バイトを得る。
/// `unsafe` 2 箇所は `device/android_payment_request.rs::call_request` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_random() -> Option<Vec<u8>> {
    use super::android::JniCallFailed;
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    let len = CHALLENGE_LEN + USER_ID_LEN;
    vm.attach_current_thread(
        |env: &mut jni::Env| -> Result<Option<Vec<u8>>, JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            let ret = env.call_method(
                &activity,
                jni::jni_str!("passkeyRandom"),
                jni::jni_sig!((int) -> byte[]),
                &[jni::objects::JValue::Int(len as i32)],
            )?;
            let arr = env.cast_local::<jni::objects::JByteArray>(ret.l()?)?;
            if arr.is_null() || arr.len(env)? != len {
                return Ok(None);
            }
            Ok(Some(env.convert_byte_array(&arr)?))
        },
    )
    .ok()
    .flatten()
}

/// `MainActivity.requestPasskey(long, boolean, String)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `device/android_payment_request.rs::call_request` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_request(id: u64, register: bool, json: &str) -> bool {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let j_json = env.new_string(json)?;
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("requestPasskey"),
            jni::jni_sig!((long, boolean, java.lang.String) -> ()),
            &[
                jni::objects::JValue::Long(id as jni::sys::jlong),
                jni::objects::JValue::Bool(register),
                jni::objects::JValue::Object(&j_json),
            ],
        )?;
        Ok(())
    })
    .is_ok()
}

/// `PasskeyHelper.nativeOnPasskeyResult`（Java 側）から呼ばれる JNI エクスポート。
/// `payload` は kind 0 で応答 JSON、kind 2 で DOM 例外名、それ以外は null。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` による panic の遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名で、既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_platform_PasskeyHelper_nativeOnPasskeyResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    kind: jni::sys::jint,
    payload: jni::objects::JString<'caller>,
) {
    let _ = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        // 保留と id が合わなければ（作り直し後の旧結果）何もしない。
        if let Some(p) = take_pending(request_id as u64) {
            let text = super::android::read_ble_string(env, &payload, PAYLOAD_MAX_UNITS);
            let result = decode_reply(&p, kind, text.as_deref());
            super::passkey::bridge::deliver(request_id as u64, result);
            // 入力が無いと egui が描画せず「処理中」で止まるため次フレームを起こす。
            crate::android::request_repaint();
        }
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::passkey::client_data_json;

    const RP: &str = "example.com";
    const UP_UV_AT: u8 = 0x45;
    const UP_UV: u8 = 0x05;

    fn rnd() -> Vec<u8> {
        (0..48u8).collect()
    }

    fn pending(op: PasskeyOp) -> Pending {
        let random = rnd();
        Pending {
            id: 7,
            op,
            challenge: Challenge::from_bytes(random[..32].try_into().unwrap()),
            rp_id: RP.into(),
        }
    }

    /// 整合する応答 JSON。`auth_flags` は authenticatorData の flags。
    fn response(p: &Pending, auth_flags: u8, with_sig: bool, with_auth: bool) -> String {
        let cd = client_data_json(p.op, &p.challenge, "android:apk-key-hash:x");
        let mut ad = vec![0u8; 32];
        ad.push(auth_flags);
        ad.extend_from_slice(&[0; 4]);
        if p.op == PasskeyOp::Register {
            // attestedCredentialData（aaguid 16 + 長さ 2 + ID 1 バイト）。
            ad.extend_from_slice(&[0; 16]);
            ad.extend_from_slice(&[0, 1, 9]);
        }
        let mut resp = serde_json::json!({ "clientDataJSON": base64url_encode(cd.as_bytes()) });
        if with_auth {
            if p.op == PasskeyOp::Register {
                // {"fmt":"none","attStmt":{},"authData":<bstr>}
                let mut ao = vec![0xa3, 0x63];
                ao.extend_from_slice(b"fmt");
                ao.extend_from_slice(&[0x64]);
                ao.extend_from_slice(b"none");
                ao.push(0x67);
                ao.extend_from_slice(b"attStmt");
                ao.push(0xa0);
                ao.push(0x68);
                ao.extend_from_slice(b"authData");
                ao.extend_from_slice(&[0x58, ad.len() as u8]);
                ao.extend_from_slice(&ad);
                resp["attestationObject"] = base64url_encode(&ao).into();
            } else {
                resp["authenticatorData"] = base64url_encode(&ad).into();
            }
        }
        if with_sig {
            resp["signature"] = base64url_encode(&[1, 2, 3]).into();
        }
        serde_json::json!({ "rawId": base64url_encode(&[9]), "response": resp }).to_string()
    }

    #[test]
    fn rp_id_validation() {
        for ok in ["example.com", "a-b.example.org", "1a.example.com"] {
            assert!(valid_rp_id(ok), "{ok}");
        }
        let long_label = format!("{}.com", "a".repeat(64));
        let long_all = format!("{}.com", vec!["a".repeat(60); 5].join("."));
        for ng in [
            "",
            "localhost",
            "192.168.0.1",
            "Example.com",
            "a..b",
            "-a.com",
            "a-.com",
            "example.com.",
            "ドメイン.com",
            long_label.as_str(),
            long_all.as_str(),
        ] {
            assert!(!valid_rp_id(ng), "{ng}");
        }
    }

    #[test]
    fn request_json_has_matching_challenge_and_rp() {
        let random = rnd();
        let ch = Challenge::from_bytes(random[..32].try_into().unwrap());
        let uid: [u8; 16] = random[32..].try_into().unwrap();
        let reg: serde_json::Value =
            serde_json::from_str(&request_json(PasskeyOp::Register, &ch, &uid, RP)).unwrap();
        let get: serde_json::Value =
            serde_json::from_str(&request_json(PasskeyOp::SignIn, &ch, &uid, RP)).unwrap();
        for v in [&reg, &get] {
            let c = base64url_decode(v["challenge"].as_str().unwrap()).unwrap();
            assert_eq!(c, ch.as_bytes());
        }
        assert_eq!(reg["rp"]["id"], RP);
        assert_eq!(
            reg["authenticatorSelection"]["userVerification"],
            "required"
        );
        assert_eq!(get["rpId"], RP);
        assert_eq!(get["allowCredentials"].as_array().unwrap().len(), 0);
        assert_eq!(get["userVerification"], "required");
    }

    #[test]
    fn consistent_responses_succeed() {
        let p = pending(PasskeyOp::Register);
        let json = response(&p, UP_UV_AT, false, true);
        assert!(matches!(
            decode_reply(&p, KIND_RESPONSE, Some(&json)),
            Ok(PasskeyOutcome::Registered(_))
        ));
        let p = pending(PasskeyOp::SignIn);
        let json = response(&p, UP_UV, true, true);
        assert!(matches!(
            decode_reply(&p, KIND_RESPONSE, Some(&json)),
            Ok(PasskeyOutcome::AssertionReturned(s)) if s.signature_len == Some(3)
        ));
    }

    #[test]
    fn inconsistent_responses_never_succeed() {
        let p = pending(PasskeyOp::SignIn);
        let other = pending(PasskeyOp::Register);
        let cases = [
            // type 違い
            response(&other, UP_UV_AT, true, true),
            // authenticatorData 欠落・signature 欠落・UV 欠落
            response(&p, UP_UV, true, false),
            response(&p, UP_UV, false, true),
            response(&p, 0x01, true, true),
            "not json".to_string(),
            "x".repeat(RESPONSE_MAX_BYTES + 1),
        ];
        for c in cases {
            assert!(
                matches!(
                    decode_reply(&p, KIND_RESPONSE, Some(&c)),
                    Err(DeviceError::InvalidInput(_))
                ),
                "{}",
                c.len()
            );
        }
        // challenge 違い
        let mut wrong = pending(PasskeyOp::SignIn);
        wrong.challenge = Challenge::from_bytes([0; 32]);
        let json = response(&p, UP_UV, true, true);
        assert!(decode_reply(&wrong, KIND_RESPONSE, Some(&json)).is_err());
        assert!(decode_reply(&p, KIND_RESPONSE, None).is_err());
    }

    #[test]
    fn kinds_map_and_only_response_succeeds() {
        let p = pending(PasskeyOp::SignIn);
        assert!(matches!(
            decode_reply(&p, KIND_NOT_ALLOWED, None),
            Ok(PasskeyOutcome::NotAllowed)
        ));
        assert!(matches!(
            decode_reply(&p, KIND_DOM_ERROR, Some("NotAllowedError")),
            Ok(PasskeyOutcome::NotAllowed)
        ));
        assert!(matches!(
            decode_reply(&p, KIND_DOM_ERROR, Some("SecurityError")),
            Err(DeviceError::Unsupported("passkey"))
        ));
        assert!(matches!(
            decode_reply(&p, KIND_DOM_ERROR, Some("")),
            Err(DeviceError::Internal(_))
        ));
        assert!(matches!(
            decode_reply(&p, KIND_UNSUPPORTED, None),
            Err(DeviceError::Unsupported("passkey"))
        ));
        let good = response(&p, UP_UV, true, true);
        for k in [-1, 4, 5, 99, i32::MIN, i32::MAX] {
            assert!(
                matches!(
                    decode_reply(&p, k, Some(&good)),
                    Err(DeviceError::Internal(_))
                ),
                "{k}"
            );
        }
    }

    // PENDING は static のため、触るテストは 1 本にまとめて並列実行の干渉を避ける。
    #[test]
    fn pending_is_id_matched_and_reset() {
        assert!(prepare(1, PasskeyOp::SignIn, None, &rnd()).is_err());
        assert!(prepare(1, PasskeyOp::SignIn, Some("localhost"), &rnd()).is_err());
        assert!(prepare(1, PasskeyOp::SignIn, Some(RP), &rnd()[..47]).is_err());
        assert!(take_pending(1).is_none());
        assert!(prepare(2, PasskeyOp::SignIn, Some(RP), &rnd()).is_ok());
        assert!(take_pending(3).is_none());
        assert!(take_pending(2).is_some());
        assert!(take_pending(2).is_none());
        assert!(prepare(4, PasskeyOp::Register, Some(RP), &rnd()).is_ok());
        reset();
        assert!(take_pending(4).is_none());
    }

    #[test]
    fn host_begin_never_succeeds() {
        for op in [PasskeyOp::Register, PasskeyOp::SignIn] {
            assert!(matches!(
                begin(1, op),
                Err(DeviceError::Unsupported("passkey"))
            ));
        }
    }
}
