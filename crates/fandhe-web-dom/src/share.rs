//! Phase 11（端末機能の拡充ループ、親 #896）型サンプル「共有」の Web 経路（Issue #900）。
//!
//! `fandhe_core::screens::share_text`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、`navigator.share`
//! （Web Share API）で共有シートを開いて結果を書き戻す。
//!
//! Web は `fandhe-device` の `Share` トレイトを通らない（`clipboard.rs`/`tts.rs` と同じ
//! 理由。`fandhe-egui-proxy` の feature `device` は wasm32 に依存を持ち込まない設計、
//! `crates/fandhe-egui-proxy/src/device.rs` 参照）。そのため本モジュールが DOM レンダラ側
//! の対応実装にあたる。ただし入力検証だけは [`fandhe_device::share::ShareRequest::new`]
//! を共有する（`crates/fandhe-egui-proxy/src/device/share.rs` と同じ検証の SSOT 方針）。
//!
//! # ブラウザ側の制約（対応表 `docs/device/capabilities/share-sheet.json` の web 欄と対）
//!
//! - **user activation が必須**: `navigator.share()` はボタンの `click` ハンドラから
//!   始まる同期チェーンの中で呼ぶ必要がある。`mount.rs` の
//!   `attach_click` → `trigger_rerender` → `render_once` → `effect` は同一タスク内で
//!   同期実行される（`tts.rs` の `speak()` 呼び出しと同じ構造）ため、本モジュールの
//!   `navigator.share_with_data()` 呼び出しも要件を満たす。`await` はこの呼び出しより
//!   前に挟まない。
//! - **secure context 必須・非対応ブラウザは失敗**: `navigator.share` 自体が存在しない
//!   環境で黙って成功・無反応に化けさせない（型に現れない JS 側の実態を境界で検証する、
//!   `clipboard.rs::clipboard()` と同じ方針、security.md A03）。
//! - **`AbortError`（利用者キャンセル）は失敗ではない**: `tts_speak`/`file_pick` と同じ
//!   再発防止方針で、キャンセルを「失敗しました」に丸めない。
//! - **`NoTarget` は Web では生成しない**: Web Share API には「共有先が無い」を表す
//!   専用の信号が無い。`canShare()` が `false` を返すのはデータ形式が共有不可能という
//!   意味で、共有先の有無とは別の概念のため失敗（`Err`）へ倒す
//!   （実装計画「設計」節の判断）。
//!
//! # 二重実行防止は不要（`clipboard.rs::IN_FLIGHT` との違い）
//!
//! `share_text::mark_sharing` を送った時点で `phase` が `Sharing` へ進み、次の
//! 再描画からは `take_request` が `None` を返す。`clipboard_text` にはこの
//! `_dispatched` 相当のフラグが無いため `IN_FLIGHT` が要るが、`share_text` は
//! `tts_speak` と同型でマウント単位の飛行中集合を Web 側に別途持つ必要がない。
//!
//! 共有テキスト・URL・例外の `message` は `console` 出力・送信を一切行わない
//! （security.md A09。`clipboard.rs::to_message` と同じ方針。エラーは例外の `name`
//! のみを見て固定文言へ畳む）。

use fandhe_core::screens::share_text::ShareOutcome;

/// 非対応ブラウザ・secure context 外・`navigator.share` 取得失敗時の案内。
/// native ビルドでは `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る
/// （`location.rs::failure_kind_from_code` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const UNAVAILABLE: &str =
    "このブラウザ・接続では共有シートを使えません（https か localhost が必要）";
/// `canShare()` が `false` を返した（データ形式が共有不可能）ときの案内。
const UNSHAREABLE: &str = "この内容はブラウザが共有できません";
/// `NotAllowedError`（user activation を伴わない呼び出し）のときの案内。
const NOT_ALLOWED: &str =
    "共有の呼び出しが許可されませんでした（ボタン操作から呼ぶ必要があります）";
/// 種別を特定できない失敗・取得不能なときの汎用案内。
const SHARE_FAILED: &str = "共有に失敗しました";

/// 例外オブジェクトの `name`（`DOMException`/`TypeError` の種別）を画面結果へ写す
/// 純関数（ブラウザ API を呼ばないため native からもテストできる。`location.rs` の
/// `failure_kind_from_code` と同じ理由で `wasm32` gate の外に置く）。
///
/// `message` は受け取らない・転送しない（モジュール doc「二重実行防止は不要」節の上、
/// security.md A09 の方針）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_error_name(name: Option<&str>) -> Result<ShareOutcome, String> {
    match name {
        Some("AbortError") => Ok(ShareOutcome::Cancelled),
        Some("NotAllowedError") => Err(NOT_ALLOWED.to_string()),
        Some("DataError") | Some("TypeError") => Err(UNSHAREABLE.to_string()),
        _ => Err(SHARE_FAILED.to_string()),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use fandhe_core::screens::share_text::{
        finish_share, mark_sharing, take_request, ShareTextState,
    };
    use fandhe_device::share::ShareRequest;
    use js_sys::Reflect;
    use wasm_bindgen::JsValue;
    use wasm_bindgen_futures::JsFuture;

    use super::{outcome_from_error_name, UNAVAILABLE, UNSHAREABLE};
    use crate::mount::Applier;

    /// `target[name]` が呼び出し可能な関数として実在するかを確認する。
    /// `Navigator::share`/`canShare` は web-sys の型上は常に呼び出せるように見えるが、
    /// secure context 外・非対応ブラウザでは実際には `undefined` になる
    /// （型に現れない JS 側の実態を境界で検証する、`clipboard.rs::clipboard()` と
    /// 同じ理由、security.md A03）。
    fn has_function(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|value| value.is_function())
            .unwrap_or(false)
    }

    /// `mount_with_effect` へ渡す effect。`share_text` 画面の共有要求を処理する。
    ///
    /// 要求が無ければ何もしない（`mount` モジュール doc の「effect フック」節の契約）。
    pub fn share_text_effect(state: &ShareTextState, applier: &Applier<ShareTextState>) {
        let Some(request) = take_request(state) else {
            return;
        };

        // 次の再描画から `take_request` が `None` を返すようにする
        // （モジュール doc「二重実行防止は不要」節参照）。
        applier.apply(mark_sharing);
        let generation = request.generation;

        // 入力検証は `fandhe_device::share::ShareRequest` の SSOT に一本化する
        // （モジュール doc 参照）。`file:// ` 等・NUL 混入はここで弾かれる。
        let parsed = match ShareRequest::new(&request.text, request.url.as_deref()) {
            Ok(parsed) => parsed,
            Err(e) => {
                let message = e.to_string();
                applier.apply(move |s| finish_share(s, generation, Err(message)));
                return;
            }
        };

        let Some(window) = web_sys::window() else {
            applier.apply(move |s| finish_share(s, generation, Err(UNAVAILABLE.to_string())));
            return;
        };
        let navigator = window.navigator();
        let navigator_value: JsValue = navigator.clone().into();

        if !has_function(&navigator_value, "share") {
            applier.apply(move |s| finish_share(s, generation, Err(UNAVAILABLE.to_string())));
            return;
        }

        let data = web_sys::ShareData::new();
        data.set_text(parsed.text());
        if let Some(url) = parsed.url() {
            data.set_url(url);
        }

        // `canShare()` があれば事前判定に使う（無い実装もあるため存在確認してから呼ぶ）。
        if has_function(&navigator_value, "canShare") && !navigator.can_share_with_data(&data) {
            applier.apply(move |s| finish_share(s, generation, Err(UNSHAREABLE.to_string())));
            return;
        }

        // click の同期チェーン内での呼び出し（user activation 要件、モジュール doc
        // 「ブラウザ側の制約」節参照）。ここより前に `await` を挟まない。
        let promise = navigator.share_with_data(&data);
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match JsFuture::from(promise).await {
                Ok(_) => Ok(fandhe_core::screens::share_text::ShareOutcome::Shared),
                Err(err) => {
                    // `message` は転送しない（モジュール doc 参照）。`name` のみで判定する。
                    let name = Reflect::get(&err, &JsValue::from_str("name"))
                        .ok()
                        .and_then(|v| v.as_string());
                    outcome_from_error_name(name.as_deref())
                }
            };
            applier.apply(move |s| finish_share(s, generation, result));
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::share_text_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abort_error_is_cancelled_not_shared() {
        let outcome = outcome_from_error_name(Some("AbortError"));
        assert_eq!(outcome, Ok(ShareOutcome::Cancelled));
        assert_ne!(outcome, Ok(ShareOutcome::Shared));
    }

    #[test]
    fn not_allowed_error_is_failure() {
        let outcome = outcome_from_error_name(Some("NotAllowedError"));
        assert_eq!(outcome, Err(NOT_ALLOWED.to_string()));
    }

    #[test]
    fn type_error_is_failure() {
        let outcome = outcome_from_error_name(Some("TypeError"));
        assert_eq!(outcome, Err(UNSHAREABLE.to_string()));
    }

    #[test]
    fn data_error_is_failure() {
        let outcome = outcome_from_error_name(Some("DataError"));
        assert_eq!(outcome, Err(UNSHAREABLE.to_string()));
    }

    #[test]
    fn unknown_or_missing_name_is_generic_failure() {
        assert_eq!(outcome_from_error_name(None), Err(SHARE_FAILED.to_string()));
        assert_eq!(
            outcome_from_error_name(Some("SomethingElse")),
            Err(SHARE_FAILED.to_string())
        );
    }
}
