//! デジタル身分証の提示要求（Digital Credentials API、検証側）の抽象
//! （Phase 11 の拡充ループ、親 #186、Issue #958）。
//!
//! 範囲は「固定の要求（18 歳以上か）を 1 種類出し、ウォレットの応答の有無と種別だけを返す」まで。
//! 応答の中身は読まない・検証しない・表示しない・保存しない・送らない。**本人確認・年齢確認の根拠にしてはならない**。
//! 画面（`fandhe_core::screens::digital_credentials`、端末依存ゼロ）へは `fandhe-egui-proxy` の
//! effect ドライバ（`device/digital_credentials.rs`）が写像する。
//!
//! 全ネイティブ OS は同期で `Unsupported`（iOS は Apple の許可待ち、Android は GMS 依存で保留、
//! macOS・Windows・Linux は対象外）。Web は `fandhe-web-dom`（後続の (c)）が const と純関数だけを共有する。
//! ブロッキング trait は置かない。iOS / Android に着手するときに `NotShared` を「取り消し」と
//! 「該当なし」に分け、blocking trait と `RequestSlot` を足す。
//!
//! # 結果の種類（[`CredentialOutcome`]）
//!
//! | 状況 | 返す値 |
//! |---|---|
//! | `DigitalCredential` が protocol 一致・data ありで解決 | `Ok(Responded)`（中身は未検証） |
//! | `NotAllowedError`（取り消し・該当なし・ウォレットなし） | `Ok(NotShared)` |
//! | `AbortError`（画面の破棄・`pagehide` の `abort()`） | `Ok(Cancelled)` |
//! | 事前判定で使えない（`typeof DigitalCredential`・protocol 非対応・secure context 外） | `Err(Unsupported("digital_credentials"))` |
//! | protocol 不一致・data なし・その他の例外 | `Err(Internal(..))` |
//!
//! # 三値の報告先（契約表 10）
//!
//! `NotShared` と `Cancelled` は失敗にしない。Web の `NotAllowedError` は取り消し・該当なし・ウォレットなしを
//! 区別できず、transient activation 不足でも発生するため、`get()` はクリックから続く同期処理内で呼ぶ。
//!
//! # 期限（契約表 3）
//!
//! アプリ側の期限は置かない。利用者が OS の画面を閉じるまで待つ。取り消しは画面の破棄と `pagehide` の
//! `abort()` だけで、理由なしで呼ぶ（既定の `AbortError` を返させる）。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 マウント内の要求と完了の順序（`(mount_id, generation)` の照合、二重に発送しない）、
//! 非対応・取り消し・該当なし・protocol 不一致・data 欠落を成功にしないこと、応答の中身と例外の `message` を
//! 画面・console・storage・ネットワークに出さないこと、画面の破棄と `pagehide` での解除。
//! 守らない: 取り消しと該当なしの区別、応答の正しさ（署名・発行者・nonce は照合しない）、
//! 端末間提示（QR + BLE）の所要時間、複数タブ、JS メモリ上に応答が一時的に残ること。

use crate::DeviceError;
use crate::passkey::base64url_encode;

/// 対応表・`DeviceError::Unsupported` で使う機能名。
pub const FEATURE: &str = "digital_credentials";
/// 要求の protocol（unsigned の OpenID4VP over DC API）。
pub const PROTOCOL: &str = "openid4vp-v1-unsigned";
/// 要求する身分証の doctype（mDL）。
pub const DOCTYPE: &str = "org.iso.18013.5.1.mDL";
/// 要求する claim の名前空間。
pub const NAMESPACE: &str = "org.iso.18013.5.1";
/// 要求する claim（18 歳以上か）。
pub const CLAIM: &str = "age_over_18";
/// nonce のバイト長。
pub const NONCE_LEN: usize = 32;

/// 提示要求の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialOutcome {
    /// ウォレットが応答を返した。protocol 一致と data 非 null だけを確かめた。中身は検証していない。
    Responded,
    /// 共有されなかった（取り消し、または該当する身分証なし。区別できない）。
    NotShared,
    /// 画面の破棄・`pagehide` による `abort()`。
    Cancelled,
}

/// `navigator.credentials.get()` の options の `digital` メンバーの値（JSON）を返す。
/// 入力で変わるのは nonce だけ。`client_id` は含めない（unsigned の DC API 要求では origin を使う）。
/// nonce を固定長配列で受けるのは、不正な nonce を型で作れなくするため。
pub fn request_json(nonce: &[u8; NONCE_LEN]) -> String {
    serde_json::json!({
        "requests": [{
            "protocol": PROTOCOL,
            "data": {
                "response_type": "vp_token",
                "response_mode": "dc_api",
                "nonce": base64url_encode(nonce),
                "dcql_query": {
                    "credentials": [{
                        "id": "mdl",
                        "format": "mso_mdoc",
                        "meta": { "doctype_value": DOCTYPE },
                        "claims": [{ "path": [NAMESPACE, CLAIM] }]
                    }]
                }
            }
        }]
    })
    .to_string()
}

/// DOM 例外名（Web の `DOMException.name`）を結果へ写す SSOT。どの名前も `Responded` にならない。
/// 非対応かどうかは例外名ではなく Web 側の事前判定で決める。
pub fn outcome_from_dom_error(name: &str) -> Result<CredentialOutcome, DeviceError> {
    match name {
        "NotAllowedError" => Ok(CredentialOutcome::NotShared),
        "AbortError" => Ok(CredentialOutcome::Cancelled),
        _ => Err(DeviceError::Internal("digital_credentials failed")),
    }
}

/// 解決した資格情報の protocol と data の有無から結果を決める。応答の中身は受け取らない。
/// `DigitalCredential` 以外で解決した場合、Web 側は `protocol` に `None` を渡す。
pub fn outcome_from_response(
    protocol: Option<&str>,
    has_data: bool,
) -> Result<CredentialOutcome, DeviceError> {
    match (protocol, has_data) {
        (Some(PROTOCOL), true) => Ok(CredentialOutcome::Responded),
        _ => Err(DeviceError::Internal("digital_credentials response")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn request_json_is_fixed_shape() {
        let n = [7u8; NONCE_LEN];
        let s = request_json(&n);
        let v: Value = serde_json::from_str(&s).unwrap();
        let d = &v["requests"][0]["data"];
        assert_eq!(v["requests"][0]["protocol"], PROTOCOL);
        assert_eq!(d["response_mode"], "dc_api");
        assert_eq!(d["response_type"], "vp_token");
        let c = &d["dcql_query"]["credentials"];
        assert_eq!(c.as_array().unwrap().len(), 1);
        assert_eq!(c[0]["meta"]["doctype_value"], DOCTYPE);
        assert_eq!(c[0]["claims"].as_array().unwrap().len(), 1);
        assert_eq!(c[0]["claims"][0]["path"][1], "age_over_18");
        assert_eq!(d["nonce"], base64url_encode(&n).as_str());
        assert_eq!(d["nonce"].as_str().unwrap().len(), 43);
        for bad in ["client_id", "dc_api.jwt", "family_name"] {
            assert!(!s.contains(bad));
        }
        let n2 = [8u8; NONCE_LEN];
        let s2 = request_json(&n2);
        assert_eq!(
            s.replace(&base64url_encode(&n), ""),
            s2.replace(&base64url_encode(&n2), "")
        );
    }

    #[test]
    fn no_dom_error_name_is_responded() {
        for n in [
            "NotAllowedError",
            "AbortError",
            "SecurityError",
            "TypeError",
            "OperationError",
            "",
            "Whatever",
        ] {
            assert!(!matches!(
                outcome_from_dom_error(n),
                Ok(CredentialOutcome::Responded)
            ));
        }
        assert!(matches!(
            outcome_from_dom_error("NotAllowedError"),
            Ok(CredentialOutcome::NotShared)
        ));
        assert!(matches!(
            outcome_from_dom_error("AbortError"),
            Ok(CredentialOutcome::Cancelled)
        ));
        assert!(matches!(
            outcome_from_dom_error("SecurityError"),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn response_requires_matching_protocol_and_data() {
        assert!(matches!(
            outcome_from_response(Some(PROTOCOL), true),
            Ok(CredentialOutcome::Responded)
        ));
        for (p, d) in [
            (Some("org-iso-mdoc"), true),
            (None, true),
            (Some(PROTOCOL), false),
            (None, false),
        ] {
            assert!(outcome_from_response(p, d).is_err());
        }
    }
}
