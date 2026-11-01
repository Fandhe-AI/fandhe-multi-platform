//! 物・サービスの決済シート（Payment Request API + Google Pay、TEST 環境）の抽象
//! （Phase 11 の拡充ループ、親 #114、Issue #541）。
//!
//! 範囲は「Google Pay の TEST 環境で固定金額の決済シートを出し、結果を返す」まで。**実決済はしない**。
//! PSP 連携は範囲外。画面（`fandhe_core::screens::payment_request`、端末依存ゼロ）へは
//! `fandhe-egui-proxy` の effect ドライバ（`device/payment_request.rs`）が写像する。
//! Web（`fandhe-web-dom`、後続の (c)）は wasm で [`PaymentSheet`]（ブロッキング）を呼べないため
//! trait を実装せず、const と [`outcome_from_dom_error`] だけを共有する。
//! Android（Issue #1130 の骨格、実装は #1131）は Google Pay API（TEST 環境）のコールバック型のため
//! [`PaymentSheet`] を実装せず、`fandhe-egui-proxy` の pull 型 bridge から Android 用 const と
//! [`outcome_from_android_result`] を共有する。iOS・macOS は対応表で保留、Windows は非対応のため配線しない。
//!
//! # 課金を起こさない設計
//!
//! 1. 支払い方法と金額は本モジュールの const だけに固定し、画面・外部入力から組み立てない
//! 2. 本番環境を表す enum 値・設定項目を作らない
//! 3. 返ったトークン（`response.details`）は読まずに捨て、`complete("success")` だけを呼ぶ。
//!    画面・console・storage・ネットワークに出さない（契約表 8）
//! 4. const に `TEST` と `example` が含まれ `PRODUCTION` と `merchantId` キーが含まれないことをテストで固定する
//! 5. Android は環境を Java の `WalletOptions` で `ENVIRONMENT_TEST` に固定し、`PaymentData.toJson()` を読まない。
//!    要求 JSON は本モジュールの const だけを JNI で渡し、Java 側に複製しない（契約表 5・8）
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
//! Android の追加分（bridge が写像する）:
//!
//! | 状況 | 返す値 |
//! |---|---|
//! | GMS が使えない（`isGooglePlayServicesAvailable` が SUCCESS 以外。更新ダイアログは出さない） | `Err(Unsupported)` |
//! | `isReadyToPay` が false | 確認は `Ok(false)`、決済は `Err(Unsupported)` |
//! | `isReadyToPay` の Task 失敗 | `Err(Internal)`（「使えない」へ畳まない、契約表 6） |
//! | `RESULT_OK` かつ `PaymentData` あり | `Ok(TestAuthorized)` |
//! | `RESULT_OK` で `PaymentData` が null | `Ok(Failed)`（結果の欠落を成功にしない） |
//! | `RESULT_CANCELED` | `Ok(Cancelled)` |
//! | `AutoResolveHelper.RESULT_ERROR`・未知のコード | `Ok(Failed)` |
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
//! Android も期限は掛けない。決済シートは別 Activity で自アプリの `onPause` / `onStop` を起こすため、
//! それらでは取り消さない。Activity 再生成では保留中の要求を Rust と Java の両方で破棄し、旧要求の結果は
//! request_id の不一致で捨てる。プロセス死をまたぐ結果は破棄する。
//!
//! # スレッド（Android）
//!
//! Java の Task listener / ActivityResult callback は main thread で受け、JNI で request_id 付きの結果を返す。
//! Rust の UI スレッドは OS を待たない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 マウント内の要求と完了の順序、嘘の成功の禁止、課金できるトークンを作らないこと。
//! 守らない: 利用者の取り消しとアプリの中断の区別、複数タブ。
//!
//! Android も同じ範囲を守り、再生成時の旧要求の破棄とトークンを読まない・出さないことを加える。
//! 守らない: プロセス死をまたぐ結果、GMS の Task が返らない場合の処理中表示、複数プロセス。

use crate::DeviceError;

/// 対応する支払い方法の識別子（Google Pay）。
pub const SUPPORTED_METHOD: &str = "https://google.com/pay";

/// `new PaymentRequest(...)` の第 1 引数（methodData 配列）。Google 公開のテスト用サンプル値だけを持つ。
pub const METHOD_DATA_JSON: &str = r#"[{"supportedMethods":"https://google.com/pay","data":{"environment":"TEST","apiVersion":2,"apiVersionMinor":0,"merchantInfo":{"merchantName":"fandhe demo"},"allowedPaymentMethods":[{"type":"CARD","parameters":{"allowedAuthMethods":["PAN_ONLY","CRYPTOGRAM_3DS"],"allowedCardNetworks":["MASTERCARD","VISA"]},"tokenizationSpecification":{"type":"PAYMENT_GATEWAY","parameters":{"gateway":"example","gatewayMerchantId":"exampleGatewayMerchantId"}}}]}}]"#;

/// `new PaymentRequest(...)` の第 2 引数（details）。1 円の固定金額。
pub const DETAILS_JSON: &str =
    r#"{"total":{"label":"テスト商品","amount":{"currency":"JPY","value":"1"}}}"#;

/// Android 用 `IsReadyToPayRequest.fromJson` の入力。Web の [`METHOD_DATA_JSON`] と同じカード条件で、
/// 金額・加盟店・トークン化は持たない。
pub const ANDROID_IS_READY_TO_PAY_JSON: &str = r#"{"apiVersion":2,"apiVersionMinor":0,"allowedPaymentMethods":[{"type":"CARD","parameters":{"allowedAuthMethods":["PAN_ONLY","CRYPTOGRAM_3DS"],"allowedCardNetworks":["MASTERCARD","VISA"]}}]}"#;

/// Android 用 `PaymentDataRequest.fromJson` の入力。TEST 固定・1 円・`merchantId` なし。
pub const ANDROID_PAYMENT_DATA_REQUEST_JSON: &str = r#"{"apiVersion":2,"apiVersionMinor":0,"merchantInfo":{"merchantName":"fandhe demo"},"allowedPaymentMethods":[{"type":"CARD","parameters":{"allowedAuthMethods":["PAN_ONLY","CRYPTOGRAM_3DS"],"allowedCardNetworks":["MASTERCARD","VISA"]},"tokenizationSpecification":{"type":"PAYMENT_GATEWAY","parameters":{"gateway":"example","gatewayMerchantId":"exampleGatewayMerchantId"}}}],"transactionInfo":{"totalPriceStatus":"FINAL","totalPrice":"1","currencyCode":"JPY","countryCode":"JP"}}"#;

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

/// Android の `onActivityResult` 相当の結果を写す SSOT。`result_code` は `Activity.RESULT_OK` = -1・
/// `RESULT_CANCELED` = 0。`has_payment_data` は `PaymentData` が null でないか。
/// `TestAuthorized` になるのは「`RESULT_OK` かつデータあり」だけで、それ以外（欠落・未知のコードを含む）は成功にしない。
pub fn outcome_from_android_result(result_code: i32, has_payment_data: bool) -> PaymentOutcome {
    match (result_code, has_payment_data) {
        (-1, true) => PaymentOutcome::TestAuthorized,
        (0, _) => PaymentOutcome::Cancelled,
        _ => PaymentOutcome::Failed,
    }
}

/// 決済シートを出す。実装は OS ごとに持たず、現状は [`UnsupportedPaymentSheet`] のみ
/// （`fandhe-egui-proxy` の Android 以外のネイティブが使う。Android は pull 型 bridge）。
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
    fn android_consts_are_test_only_fixed_values() {
        let ready: Value = serde_json::from_str(ANDROID_IS_READY_TO_PAY_JSON).unwrap();
        let pay: Value = serde_json::from_str(ANDROID_PAYMENT_DATA_REQUEST_JSON).unwrap();
        let web: Value = serde_json::from_str(METHOD_DATA_JSON).unwrap();
        let web_methods = &web[0]["data"]["allowedPaymentMethods"];
        assert_eq!(pay["allowedPaymentMethods"], *web_methods);
        assert_eq!(
            ready["allowedPaymentMethods"][0]["parameters"],
            web_methods[0]["parameters"]
        );
        let p = &pay["allowedPaymentMethods"][0]["tokenizationSpecification"]["parameters"];
        assert_eq!(p["gateway"], "example");
        assert_eq!(pay["transactionInfo"]["currencyCode"], "JPY");
        assert_eq!(pay["transactionInfo"]["totalPrice"], "1");
        for j in [
            ANDROID_IS_READY_TO_PAY_JSON,
            ANDROID_PAYMENT_DATA_REQUEST_JSON,
        ] {
            assert!(!j.contains("PRODUCTION"));
        }
        let mut ks = vec![];
        keys(&ready, &mut ks);
        keys(&pay, &mut ks);
        assert!(!ks.iter().any(|k| k == "merchantId"));
    }

    #[test]
    fn no_android_result_is_authorized_without_data() {
        assert_eq!(
            outcome_from_android_result(-1, true),
            PaymentOutcome::TestAuthorized
        );
        assert_eq!(
            outcome_from_android_result(-1, false),
            PaymentOutcome::Failed
        );
        assert_eq!(
            outcome_from_android_result(0, true),
            PaymentOutcome::Cancelled
        );
        assert_eq!(
            outcome_from_android_result(0, false),
            PaymentOutcome::Cancelled
        );
        for c in [1, 2, i32::MIN, i32::MAX] {
            for d in [true, false] {
                assert_eq!(outcome_from_android_result(c, d), PaymentOutcome::Failed);
            }
        }
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
