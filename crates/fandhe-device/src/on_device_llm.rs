//! 端末内 LLM の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/on-device-llm.json`、親 #93、Issue #345）。
//!
//! 扱う範囲は「固定の短いプロンプトを 1 回渡し、応答の全文を受け取って表示する。使えない端末
//! では理由つきの失敗を出す」ことだけ。ストリーミング・会話履歴・利用者が入力するプロンプト・
//! 構造化出力・モデルのダウンロード開始・取り消し・温度の調整は扱わない。対象は macOS / iOS
//! （FoundationModels）と Web（Prompt API）。Android は保留、Windows は申請待ちで
//! `Unsupported` 固定。一発呼び出し型の同期 API で、呼び出し側（`fandhe-egui-proxy` の effect
//! ドライバ）は UI を止めないようワーカースレッドから呼ぶ。Web は同じ契約を `fandhe-web-dom`
//! の非同期 effect で実装する。
//!
//! # 契約
//!
//! - **成功**: 空でない文字列だけ（[`check_response`]）。モデルが使えない状態（非対応・無効・
//!   未取得）では応答を作らず失敗にする（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **失敗の区別**（写像表）:
//!
//! | 状況 | Apple | Web | 返す値 |
//! | --- | --- | --- | --- |
//! | OS が古い・API が無い | `#available` / `canImport` 不成立 | `LanguageModel` が無い | `Unsupported` |
//! | 非対応機種 | `deviceNotEligible` | `unavailable` | `Unsupported` |
//! | 利用者が無効にしている | `appleIntelligenceNotEnabled` | 区別できない | `Disabled` |
//! | モデル未取得・取得中 | `modelNotReady`・`assetsUnavailable` | `downloadable`・`downloading` | `Internal(`[`MODEL_NOT_READY`]`)` |
//! | 応答拒否 | `guardrailViolation`・`refusal` | — | `Internal(`[`REFUSED`]`)` |
//! | 混雑・回数制限 | `rateLimited`・`concurrentRequests` | — | `Internal(`[`BUSY`]`)` |
//! | 期限切れ | Rust の期限 | Rust の期限 | `Internal(`[`TIMEOUT`]`)` |
//! | 空・空白だけの応答 | — | — | `Internal(`[`EMPTY`]`)` |
//! | `create()` の拒否 | — | `NotAllowedError` | `PermissionDenied` |
//! | 入力不正 | — | — | `InvalidInput`（[`validate_prompt`]） |
//! | それ以外 | 他の `GenerationError` | 他の例外 | `Internal("on_device_llm")` |
//!
//! - **三値**（契約表 10）: 利用者の取り消し操作が無いため成功 / 失敗の 2 値。応答拒否と
//!   モデル未取得は失敗として表示する
//! - **期限**（契約表 3）: 期限は Rust のドライバだけが持つ（[`RESPOND_DEADLINE`]、初回の
//!   モデル読み込みを含む）。起点は要求を観測した時の `Instant`（Web は `performance.now()`）。
//!   期限後に届いた成功は捨てる
//! - **並行性**（契約表 2）: 世代は画面が要求ごとに進め、完了は「処理中かつ同じ世代」の
//!   ときだけ適用する。処理中の再押下は無視する
//! - **ライフサイクル**（契約表 1）: iOS の背景遷移で `rateLimited` になっても失敗として出す
//!   だけ。Web は `pagehide` で `abort()` する
//! - **資源の解放**（契約表 9）: Web はすべての経路で `session.destroy()` する。Apple の
//!   セッションは呼び出しごとに作る
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序、嘘の成功の禁止、資源の解放。守らない範囲:
//! OS ごとの所要時間の違い、応答内容の非決定性、期限後も OS 側の生成が走り切ること、複数タブ・
//! 複数プロセス。プロンプト・応答・OS のエラー文はログへ出さない。

use std::time::Duration;

use crate::DeviceError;

/// 要求を受け付けてから結果を確定するまでの期限（Rust のドライバだけが持つ。初回のモデル読み込みを含む）。
pub const RESPOND_DEADLINE: Duration = Duration::from_secs(60);
/// プロンプトの最大バイト数。
pub const MAX_PROMPT_BYTES: usize = 1024;
/// Apple が `maximumResponseTokens` に使う応答の最大トークン数。
pub const MAX_RESPONSE_TOKENS: i32 = 256;
/// Apple の出力バッファのバイト数。
pub const RESPONSE_BUF_BYTES: usize = 16 * 1024;

/// モデルが端末にまだ無い（未取得・取得中）ときの `Internal` の識別子（SSOT）。
pub const MODEL_NOT_READY: &str = "on_device_llm model_not_ready";
/// モデルが応答を断ったときの `Internal` の識別子（SSOT）。
pub const REFUSED: &str = "on_device_llm refused";
/// 混雑・回数制限のときの `Internal` の識別子（SSOT）。
pub const BUSY: &str = "on_device_llm busy";
/// 期限切れのときの `Internal` の識別子（SSOT）。
pub const TIMEOUT: &str = "on_device_llm timeout";
/// 応答が空のときの `Internal` の識別子（SSOT）。
pub const EMPTY: &str = "on_device_llm empty";

/// 端末内 LLM の契約（モジュール doc の「契約」節）。
pub trait OnDeviceLlm: Send + Sync {
    /// `prompt` を 1 回渡して応答の全文を返す。`remaining` は期限までの残り時間。
    fn respond(&self, prompt: &str, remaining: Duration) -> Result<String, DeviceError>;
}

/// 対象外 OS・未実装 OS のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedOnDeviceLlm;

impl OnDeviceLlm for UnsupportedOnDeviceLlm {
    fn respond(&self, _: &str, _: Duration) -> Result<String, DeviceError> {
        Err(DeviceError::Unsupported("on_device_llm"))
    }
}

/// プロンプトを OS へ渡す前に検査する。空・空白だけ・NUL を含む・`MAX_PROMPT_BYTES` 超過は `InvalidInput`。
pub fn validate_prompt(prompt: &str) -> Result<(), DeviceError> {
    if prompt.trim().is_empty() || prompt.contains('\0') || prompt.len() > MAX_PROMPT_BYTES {
        Err(DeviceError::InvalidInput("on_device_llm prompt"))
    } else {
        Ok(())
    }
}

/// 応答を検査する。trim 後に空なら `Internal(EMPTY)`。それ以外は入力どおり返す（trim しない）。
pub fn check_response(text: String) -> Result<String, DeviceError> {
    if text.trim().is_empty() {
        Err(DeviceError::Internal(EMPTY))
    } else {
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_prompt_rejects_bad_input() {
        assert!(validate_prompt("こんにちは").is_ok());
        assert!(validate_prompt("a".repeat(MAX_PROMPT_BYTES).as_str()).is_ok());
        for bad in [
            "".to_string(),
            " \n\t".to_string(),
            "a\0b".to_string(),
            "a".repeat(MAX_PROMPT_BYTES + 1),
        ] {
            assert!(
                matches!(validate_prompt(&bad), Err(DeviceError::InvalidInput(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn check_response_rejects_empty_and_keeps_text() {
        for bad in ["", "  \n"] {
            assert!(matches!(
                check_response(bad.to_string()),
                Err(DeviceError::Internal(EMPTY))
            ));
        }
        assert_eq!(check_response(" 春 ".to_string()).unwrap(), " 春 ");
    }

    #[test]
    fn unsupported_is_not_a_fake_success() {
        let e = UnsupportedOnDeviceLlm
            .respond("x", RESPOND_DEADLINE)
            .unwrap_err();
        assert!(matches!(e, DeviceError::Unsupported("on_device_llm")));
    }
}
