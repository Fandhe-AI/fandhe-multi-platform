//! パスキー（WebAuthn）の登録・サインイン要求の抽象（Phase 11 の拡充ループ、親 #92、Issue #342）。
//!
//! 範囲は「OS がパスキーの登録・サインイン UI を出し、署名付きの応答が返る」まで。
//! **サーバー検証なしのデモ**で、署名と rpIdHash は検証しない（検証には暗号の新規依存が要る。既知の天井）。
//! そのため結果名に「サインイン成立」を意味する語は使わない。応答が今回の要求のものかは、
//! `clientDataJSON` の `type`・`challenge` と authenticatorData のフラグで確かめる
//! （[`check_client_data`]・[`verify_response`]）。画面（`fandhe_core::screens::passkey_signin`、
//! 端末依存ゼロ）へは `fandhe-egui-proxy` の effect ドライバ（`device/passkey.rs`）が写像する。
//! Windows（#343）・Web（#344）が [`Passkey`] を実装し、写像の純関数（[`outcome_from_dom_error`]・
//! base64url・検証）を共有する。iOS・macOS・Android は対応表で保留（`deferred`）のため配線しない。
//!
//! # 結果の種類（[`PasskeyOutcome`]）
//!
//! - `Registered`: 登録の応答が返り、整合確認を通った
//! - `AssertionReturned`: サインインの署名付き応答が返り、整合確認を通った（**サインインの成立ではない**）
//! - `NotAllowed`: キャンセル・該当パスキーなし・期限切れ・同意拒否。OS はこれらを区別して返さない。
//!   成功にも失敗にも見せない
//!
//! | 状況 | 返す値 |
//! |---|---|
//! | 認証器なし・secure context 外・OS が古い | `Unsupported("passkey")` |
//! | challenge 不一致・`type` 違い・JSON 不正・サイズ超過 | `InvalidInput("passkey client data")` |
//! | UP / UV / AT の欠落・authenticatorData が短い | `InvalidInput("passkey flags")` |
//! | 資格情報 ID が空 | `InvalidInput("passkey credential")` |
//! | 未知のエラー・`InvalidStateError` | `Internal("passkey failed")` |
//!
//! # 三値の報告先（契約表 10）
//!
//! 利用者の取り消し等（`NotAllowedError`・`AbortError`）は `NotAllowed` で報告し、失敗にしない。
//! 未知のエラー名は成功へ畳まず失敗に倒す。
//!
//! # 期限・ライフサイクル（契約表 1・3）
//!
//! Rust 側で独自の期限は掛けない（利用者操作の UI で、OS 側の期限に従う）。Web は `AbortController`、
//! Windows は `WebAuthNCancelCurrentOperation` で取り消し、取り消し後に届いた応答は世代の照合で捨てる。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 画面・1 プロセス内で同時に 1 要求、古い世代の完了を捨てること、嘘の成功の禁止
//! （失敗・非対応・不整合を成功に畳まない）、Windows のネイティブ資源の解放。
//!
//! 守らない: 複数タブ・複数プロセスからの同時登録、OS やパスワードマネージャーの一覧に反映される
//! までの時間差、アプリ終了と OS 呼び出しの重なり、署名・rpIdHash の検証。
//! challenge・user.id・資格情報 ID・署名・`clientDataJSON` はログ・`Debug` に出さず、永続化しない（契約表 8）。

use crate::DeviceError;

/// challenge の長さ（バイト）。OS 層が CSPRNG で要求ごとに作る。
pub const CHALLENGE_LEN: usize = 32;
/// デモ用 user.id の長さ（バイト）。
pub const USER_ID_LEN: usize = 16;
/// デモ用のユーザー名（個人情報は使わない）。
pub const DEMO_USER_NAME: &str = "fandhe-demo";
/// Relying Party の表示名。
pub const RP_NAME: &str = "fandhe demo";
/// Windows の RP ID。他者の実在ドメインを名乗らない（Web はオリジンのドメインを使う）。
pub const NATIVE_RP_ID: &str = "localhost";
/// `clientDataJSON` のサイズ上限。超えたら解析せず拒否する。
pub const CLIENT_DATA_MAX_BYTES: usize = 4096;

const FLAGS_OFFSET: usize = 32;
const AUTH_DATA_MIN: usize = 37;
const FLAG_UP: u8 = 0x01;
const FLAG_UV: u8 = 0x04;
const FLAG_AT: u8 = 0x40;

/// 要求の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasskeyOp {
    /// パスキーの登録（`create`）。
    Register,
    /// サインイン要求（`get`）。
    SignIn,
}

impl PasskeyOp {
    fn client_data_type(self) -> &'static str {
        match self {
            Self::Register => "webauthn.create",
            Self::SignIn => "webauthn.get",
        }
    }
}

/// 要求ごとの challenge。`Debug` は値を伏せる。
#[derive(Clone, PartialEq, Eq)]
pub struct Challenge([u8; CHALLENGE_LEN]);

impl Challenge {
    /// OS 層が CSPRNG で作った値から作る（本クレートは乱数を作らない）。
    pub fn from_bytes(bytes: [u8; CHALLENGE_LEN]) -> Self {
        Self(bytes)
    }

    /// OS へ渡す生のバイト。
    pub fn as_bytes(&self) -> &[u8; CHALLENGE_LEN] {
        &self.0
    }
}

impl std::fmt::Debug for Challenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Challenge(<redacted>)")
    }
}

/// 資格情報 ID。`Debug` は値を伏せる。
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialId(Vec<u8>);

impl CredentialId {
    /// OS が返したバイト列から作る。
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// 生のバイト（画面側の比較用）。
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for CredentialId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialId(<redacted>)")
    }
}

/// 整合確認を通った応答の要約。署名そのものは持たず、長さだけ持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasskeySummary {
    /// 応答の RP ID。
    pub rp_id: String,
    /// 資格情報 ID。
    pub credential_id: CredentialId,
    /// ユーザーの存在（UP）。
    pub user_present: bool,
    /// ユーザーの検証（UV）。
    pub user_verified: bool,
    /// 署名のバイト数（登録では `None`）。
    pub signature_len: Option<usize>,
}

/// パスキー要求の結果。`AssertionReturned` はサインインの成立を意味しない（モジュール doc）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasskeyOutcome {
    /// 登録の応答が返り、整合確認を通った。
    Registered(PasskeySummary),
    /// サインインの署名付き応答が返り、整合確認を通った。
    AssertionReturned(PasskeySummary),
    /// キャンセル・該当なし・期限切れ・同意拒否（区別できない）。
    NotAllowed,
}

/// DOM 例外名（Web の `DOMException.name`、Windows は `WebAuthNGetW3CExceptionDOMError` の結果）を
/// 結果へ写す SSOT。どの名前も成功にならない。
pub fn outcome_from_dom_error(name: &str) -> Result<PasskeyOutcome, DeviceError> {
    match name {
        "NotAllowedError" | "AbortError" => Ok(PasskeyOutcome::NotAllowed),
        "NotSupportedError" | "SecurityError" | "ConstraintError" => {
            Err(DeviceError::Unsupported("passkey"))
        }
        _ => Err(DeviceError::Internal("passkey failed")),
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// base64url（パディングなし）へ符号化する。
pub fn base64url_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |a, (i, b)| a | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// base64url（パディングなし）を復号する。`=`・不正文字・不正な長さ・正規形でない入力は `None`。
pub fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.as_bytes().chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            let v = B64.iter().position(|x| x == c)? as u32;
            n |= v << (18 - 6 * i);
        }
        for i in 0..chunk.len() - 1 {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    // 余りビットが 0 でない入力は正規形でないので拒否する。
    (base64url_encode(&out) == s).then_some(out)
}

/// Windows が自分で渡す `clientDataJSON` を作る（`json!` で組み立て、文字列連結はしない）。
pub fn client_data_json(op: PasskeyOp, challenge: &Challenge, origin: &str) -> String {
    serde_json::json!({
        "type": op.client_data_type(),
        "challenge": base64url_encode(challenge.as_bytes()),
        "origin": origin,
        "crossOrigin": false,
    })
    .to_string()
}

/// `clientDataJSON` が今回の要求のものか確かめる。サイズ上限 → UTF-8 → JSON の順に検査し、
/// `type` と `challenge`（復号値）の一致を見る。
pub fn check_client_data(
    json: &[u8],
    op: PasskeyOp,
    challenge: &Challenge,
) -> Result<(), DeviceError> {
    const BAD: DeviceError = DeviceError::InvalidInput("passkey client data");
    if json.len() > CLIENT_DATA_MAX_BYTES {
        return Err(BAD);
    }
    let text = std::str::from_utf8(json).map_err(|_| BAD)?;
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| BAD)?;
    let ty = v.get("type").and_then(|x| x.as_str()).ok_or(BAD)?;
    let ch = v.get("challenge").and_then(|x| x.as_str()).ok_or(BAD)?;
    let decoded = base64url_decode(ch).ok_or(BAD)?;
    if ty == op.client_data_type() && decoded.as_slice() == challenge.as_bytes() {
        Ok(())
    } else {
        Err(BAD)
    }
}

/// authenticatorData のフラグ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flags {
    /// ユーザーの存在。
    pub up: bool,
    /// ユーザーの検証。
    pub uv: bool,
    /// 証明済み資格情報データを含む。
    pub at: bool,
}

/// authenticatorData の flags（byte 32）を読む。37 バイト未満は拒否する。
pub fn parse_flags(authenticator_data: &[u8]) -> Result<Flags, DeviceError> {
    if authenticator_data.len() < AUTH_DATA_MIN {
        return Err(DeviceError::InvalidInput("passkey flags"));
    }
    let f = authenticator_data[FLAGS_OFFSET];
    Ok(Flags {
        up: f & FLAG_UP != 0,
        uv: f & FLAG_UV != 0,
        at: f & FLAG_AT != 0,
    })
}

/// 応答の整合確認。登録は UP・UV・AT、サインインは UP・UV を要求する。通れば要約を返す。
/// rpIdHash と署名は検証しない（モジュール doc）。
pub fn verify_response(
    op: PasskeyOp,
    challenge: &Challenge,
    rp_id: &str,
    credential_id: Vec<u8>,
    client_data_json: &[u8],
    authenticator_data: &[u8],
    signature: Option<&[u8]>,
) -> Result<PasskeySummary, DeviceError> {
    check_client_data(client_data_json, op, challenge)?;
    let flags = parse_flags(authenticator_data)?;
    let flags_ok = flags.up && flags.uv && (op == PasskeyOp::SignIn || flags.at);
    if !flags_ok {
        return Err(DeviceError::InvalidInput("passkey flags"));
    }
    if credential_id.is_empty() {
        return Err(DeviceError::InvalidInput("passkey credential"));
    }
    Ok(PasskeySummary {
        rp_id: rp_id.to_string(),
        credential_id: CredentialId::from_bytes(credential_id),
        user_present: flags.up,
        user_verified: flags.uv,
        signature_len: signature.map(<[u8]>::len),
    })
}

/// パスキーの登録・サインイン UI を出す。OS ごとの実装は `fandhe-egui-proxy`（Windows）/
/// `fandhe-web-dom`（Web）が持つ。
pub trait Passkey: Send + Sync {
    /// ブロッキングで OS の UI を出し、結果を返す。ワーカースレッドから呼ぶ前提。
    fn run(&self, op: PasskeyOp) -> Result<PasskeyOutcome, DeviceError>;
}

/// 非対応 OS 用。常に `Unsupported` を返し、成功を作らない。
pub struct UnsupportedPasskey;

impl Passkey for UnsupportedPasskey {
    fn run(&self, _op: PasskeyOp) -> Result<PasskeyOutcome, DeviceError> {
        Err(DeviceError::Unsupported("passkey"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(b: u8) -> Challenge {
        Challenge::from_bytes([b; CHALLENGE_LEN])
    }

    fn auth(flags: u8) -> Vec<u8> {
        let mut v = vec![0u8; 37];
        v[32] = flags;
        v
    }

    #[test]
    fn base64url_rfc4648_vectors_round_trip() {
        for (raw, enc) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64url_encode(raw.as_bytes()), enc);
            assert_eq!(base64url_decode(enc).unwrap(), raw.as_bytes());
        }
        assert_eq!(base64url_encode(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn base64url_rejects_padding_bad_chars_and_noncanonical() {
        for s in ["Zg==", "Zm9v+", "Zm9v/", "Z", "Zm 9", "Zh"] {
            assert!(base64url_decode(s).is_none(), "{s}");
        }
    }

    #[test]
    fn no_dom_error_name_is_success() {
        for n in [
            "NotAllowedError",
            "AbortError",
            "NotSupportedError",
            "SecurityError",
            "ConstraintError",
            "InvalidStateError",
            "Whatever",
            "",
        ] {
            assert!(!matches!(
                outcome_from_dom_error(n),
                Ok(PasskeyOutcome::Registered(_) | PasskeyOutcome::AssertionReturned(_))
            ));
        }
        assert!(matches!(
            outcome_from_dom_error("NotAllowedError"),
            Ok(PasskeyOutcome::NotAllowed)
        ));
        assert!(matches!(
            outcome_from_dom_error("AbortError"),
            Ok(PasskeyOutcome::NotAllowed)
        ));
        assert!(matches!(
            outcome_from_dom_error("SecurityError"),
            Err(DeviceError::Unsupported("passkey"))
        ));
        assert!(matches!(
            outcome_from_dom_error("InvalidStateError"),
            Err(DeviceError::Internal("passkey failed"))
        ));
        assert!(matches!(
            outcome_from_dom_error("x"),
            Err(DeviceError::Internal("passkey failed"))
        ));
    }

    #[test]
    fn built_client_data_passes_and_mismatches_fail() {
        let c = ch(7);
        let ok = client_data_json(PasskeyOp::Register, &c, "https://localhost");
        assert!(check_client_data(ok.as_bytes(), PasskeyOp::Register, &c).is_ok());
        // create と get の取り違え、前回の要求の challenge
        assert!(check_client_data(ok.as_bytes(), PasskeyOp::SignIn, &c).is_err());
        assert!(check_client_data(ok.as_bytes(), PasskeyOp::Register, &ch(8)).is_err());
    }

    #[test]
    fn malformed_client_data_is_rejected() {
        let c = ch(7);
        let padded = format!(
            r#"{{"type":"webauthn.get","challenge":"{}="}}"#,
            base64url_encode(c.as_bytes())
        );
        let big = vec![b' '; CLIENT_DATA_MAX_BYTES + 1];
        let cases: Vec<&[u8]> = vec![
            b"not json",
            b"{}",
            br#"{"type":"webauthn.get"}"#,
            br#"{"challenge":"x"}"#,
            &[0xff, 0xfe],
            padded.as_bytes(),
            &big,
        ];
        for j in cases {
            assert!(matches!(
                check_client_data(j, PasskeyOp::SignIn, &c),
                Err(DeviceError::InvalidInput("passkey client data"))
            ));
        }
    }

    #[test]
    fn flags_are_enforced() {
        assert!(parse_flags(&[0; 36]).is_err());
        let c = ch(1);
        let run = |op, flags| {
            let cd = client_data_json(op, &c, "o");
            verify_response(
                op,
                &c,
                "localhost",
                vec![1],
                cd.as_bytes(),
                &auth(flags),
                Some(&[0; 64]),
            )
        };
        assert!(run(PasskeyOp::Register, 0x45).is_ok());
        assert!(run(PasskeyOp::Register, 0x05).is_err()); // AT 欠落
        assert!(run(PasskeyOp::Register, 0x41).is_err()); // UV 欠落
        assert!(run(PasskeyOp::Register, 0x44).is_err()); // UP 欠落
        assert!(run(PasskeyOp::SignIn, 0x05).is_ok());
        assert!(run(PasskeyOp::SignIn, 0x01).is_err());
        let cd = client_data_json(PasskeyOp::Register, &c, "o");
        let empty = verify_response(
            PasskeyOp::Register,
            &c,
            "l",
            vec![],
            cd.as_bytes(),
            &auth(0x45),
            None,
        );
        assert!(matches!(
            empty,
            Err(DeviceError::InvalidInput("passkey credential"))
        ));
        assert_eq!(
            run(PasskeyOp::SignIn, 0x05).unwrap().signature_len,
            Some(64)
        );
    }

    #[test]
    fn stale_challenge_response_is_rejected() {
        let old = ch(1);
        let cd = client_data_json(PasskeyOp::SignIn, &old, "o");
        let r = verify_response(
            PasskeyOp::SignIn,
            &ch(2),
            "l",
            vec![1],
            cd.as_bytes(),
            &auth(0x05),
            None,
        );
        assert!(matches!(
            r,
            Err(DeviceError::InvalidInput("passkey client data"))
        ));
    }

    #[test]
    fn unsupported_is_never_success() {
        for op in [PasskeyOp::Register, PasskeyOp::SignIn] {
            assert!(matches!(
                UnsupportedPasskey.run(op),
                Err(DeviceError::Unsupported("passkey"))
            ));
        }
    }

    #[test]
    fn debug_hides_secret_bytes() {
        let c = format!("{:?}", Challenge::from_bytes([0xAB; CHALLENGE_LEN]));
        let i = format!("{:?}", CredentialId::from_bytes(vec![0xCD; 8]));
        for s in [c, i] {
            assert!(s.contains("redacted"));
            assert!(!s.contains("171") && !s.contains("205"));
        }
    }
}
