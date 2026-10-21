//! 物・サービスの決済シート（Payment Request API + Google Pay、TEST 環境）の抽象
//! （Phase 11 の拡充ループ、親 #114、Issue #541）。
//!
//! 範囲は「Google Pay の TEST 環境で固定金額の決済シートを出し、結果を返す」まで。**実決済はしない**。
//! PSP 連携は範囲外。画面（`fandhe_core::screens::payment_request`、端末依存ゼロ）へは
//! `fandhe-egui-proxy` の effect ドライバ（`device/payment_request.rs`）が写像する。
//! Web（`fandhe-web-dom`、後続の (c)）は wasm で [`PaymentSheet`]（ブロッキング）を呼べないため
//! trait を実装せず、const と [`outcome_from_dom_error`] だけを共有する。
//! iOS・macOS・Android は対応表で保留、Windows は非対応のため配線しない。
//!
//! # 課金を起こさない設計
//!
//! 1. 支払い方法と金額は本モジュールの const だけに固定し、画面・外部入力から組み立てない
//! 2. 本番環境を表す enum 値・設定項目を作らない
//! 3. 返ったトークン（`response.details`）は読まずに捨て、`complete("success")` だけを呼ぶ。
//!    画面・console・storage・ネットワークに出さない（契約表 8）
//! 4. const に `TEST` と `example` が含まれ `PRODUCTION` と `merchantId` キーが含まれないことをテストで固定する
//!
//! # 結果の種類（[`PaymentOutcome`]）
//!
//! | 状況 | 返す値 |
//! |---|---|
//! | テスト環境で承認された | `Ok(TestAuthorized)`（決済の成立ではない） |
//! | 利用者の取り消し・`abort()`（`AbortError`） | `Ok(Cancelled)` |
//! | 支払い方法なし・secure context 外・iframe・ユーザー操作外 | `Err(Unsupported("payment_request"))` |
//! | `NotAllowedError`・`InvalidStateError`・入力不正・未知のエラー | `Ok(Failed)` |
//! | `can_make_payments` が使える / 使えない | `Ok(true)` / `Ok(false)` |
//!
//! # 三値の報告先（契約表 10）
//!
//! 利用者の取り消しと `abort()` は API がどちらも `AbortError` で返し区別できない。両方を `Cancelled`
//! とし失敗にしない。未知のエラー名は成功へ畳まず `Failed` にする。
//!
//! # 期限・ライフサイクル（契約表 1・3）
//!
//! アプリ側の期限は掛けない（利用者操作の UI のため）。Web はマウント破棄と `pagehide` で `abort()` し、
//! 古い世代の完了は画面の世代照合で捨てる。`show()` はユーザー操作の有効期間内に呼ぶ必要がある。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 マウント内の要求と完了の順序、嘘の成功の禁止、課金できるトークンを作らないこと。
//! 守らない: 利用者の取り消しとアプリの中断の区別、複数タブ。

use crate::DeviceError;

/// 対応する支払い方法の識別子（Google Pay）。
pub const SUPPORTED_METHOD: &str = "https://google.com/pay";

/// `new PaymentRequest(...)` の第 1 引数（methodData 配列）。Google 公開のテスト用サンプル値だけを持つ。
pub const METHOD_DATA_JSON: &str = r#"[{"supportedMethods":"https://google.com/pay","data":{"environment":"TEST","apiVersion":2,"apiVersionMinor":0,"merchantInfo":{"merchantName":"fandhe demo"},"allowedPaymentMethods":[{"type":"CARD","parameters":{"allowedAuthMethods":["PAN_ONLY","CRYPTOGRAM_3DS"],"allowedCardNetworks":["MASTERCARD","VISA"]},"tokenizationSpecification":{"type":"PAYMENT_GATEWAY","parameters":{"gateway":"example","gatewayMerchantId":"exampleGatewayMerchantId"}}}]}}]"#;

/// `new PaymentRequest(...)` の第 2 引数（details）。1 円の固定金額。
pub const DETAILS_JSON: &str =
    r#"{"total":{"label":"テスト商品","amount":{"currency":"JPY","value":"1"}}}"#;

/// 決済シートの結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentOutcome {
    /// テスト環境で承認されただけで、決済の成立は意味しない。トークンは破棄していて課金は無い。
    TestAuthorized,
    /// 利用者の取り消し、またはアプリの `abort()`（区別できない）。
    Cancelled,
    /// 失敗（権限・状態・入力不正・未知のエラー）。
    Failed,
}

/// DOM 例外名（Web の `DOMException.name`）を結果へ写す SSOT。どの名前も `TestAuthorized` にならない。
pub fn outcome_from_dom_error(name: &str) -> Result<PaymentOutcome, DeviceError> {
    match name {
        "AbortError" => Ok(PaymentOutcome::Cancelled),
        "NotSupportedError" | "SecurityError" => Err(DeviceError::Unsupported("payment_request")),
        _ => Ok(PaymentOutcome::Failed),
    }
}

/// 決済シートを出す。実装は OS ごとに持たず、現状は [`UnsupportedPaymentSheet`] のみ
/// （`fandhe-egui-proxy` の全ネイティブが使う）。
pub trait PaymentSheet: Send + Sync {
    /// 支払いに使えるかを調べる。`Ok(false)` は使えない、非対応環境は `Err(Unsupported)`。
    fn can_make_payments(&self) -> Result<bool, DeviceError>;
    /// 決済シートを出して結果を返す。使えない環境では `Err(Unsupported)`。
    fn request(&self) -> Result<PaymentOutcome, DeviceError>;
}

/// 非対応 OS 用。常に `Unsupported` を返し、成功を作らない。
pub struct UnsupportedPaymentSheet;

impl PaymentSheet for UnsupportedPaymentSheet {
    fn can_make_payments(&self) -> Result<bool, DeviceError> {
        Err(DeviceError::Unsupported("payment_request"))
    }
    fn request(&self) -> Result<PaymentOutcome, DeviceError> {
        Err(DeviceError::Unsupported("payment_request"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn keys(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    out.push(k.clone());
                    keys(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| keys(x, out)),
            _ => {}
        }
    }

    #[test]
    fn consts_are_test_only_fixed_values() {
        let m: Value = serde_json::from_str(METHOD_DATA_JSON).unwrap();
        let d: Value = serde_json::from_str(DETAILS_JSON).unwrap();
        assert_eq!(m[0]["supportedMethods"], SUPPORTED_METHOD);
        assert_eq!(m[0]["data"]["environment"], "TEST");
        let p =
            &m[0]["data"]["allowedPaymentMethods"][0]["tokenizationSpecification"]["parameters"];
        assert_eq!(p["gateway"], "example");
        assert_eq!(p["gatewayMerchantId"], "exampleGatewayMerchantId");
        assert_eq!(d["total"]["amount"]["currency"], "JPY");
        assert_eq!(d["total"]["amount"]["value"], "1");
        assert_eq!(d["total"]["label"], "テスト商品");
        assert!(!METHOD_DATA_JSON.contains("PRODUCTION") && !DETAILS_JSON.contains("PRODUCTION"));
        let mut ks = vec![];
        keys(&m, &mut ks);
        keys(&d, &mut ks);
        assert!(!ks.iter().any(|k| k == "merchantId"));
    }

    #[test]
    fn no_dom_error_name_is_authorized() {
        for n in [
            "AbortError",
            "NotSupportedError",
            "SecurityError",
            "NotAllowedError",
            "InvalidStateError",
            "TypeError",
            "RangeError",
            "",
            "Whatever",
        ] {
            assert!(!matches!(
                outcome_from_dom_error(n),
                Ok(PaymentOutcome::TestAuthorized)
            ));
        }
        assert!(matches!(
            outcome_from_dom_error("AbortError"),
            Ok(PaymentOutcome::Cancelled)
        ));
        for n in ["NotSupportedError", "SecurityError"] {
            assert!(matches!(
                outcome_from_dom_error(n),
                Err(DeviceError::Unsupported("payment_request"))
            ));
        }
        for n in ["NotAllowedError", "TypeError", "", "x"] {
            assert!(matches!(
                outcome_from_dom_error(n),
                Ok(PaymentOutcome::Failed)
            ));
        }
    }

    #[test]
    fn unsupported_is_never_success() {
        assert!(matches!(
            UnsupportedPaymentSheet.can_make_payments(),
            Err(DeviceError::Unsupported("payment_request"))
        ));
        assert!(matches!(
            UnsupportedPaymentSheet.request(),
            Err(DeviceError::Unsupported("payment_request"))
        ));
    }
}
