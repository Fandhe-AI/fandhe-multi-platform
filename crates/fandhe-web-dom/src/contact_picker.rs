//! 連絡先を 1 件選ぶ機能の Web 経路（Phase 11 の拡充ループ、親 #1117、Issue #1223）。
//!
//! `fandhe_core::screens::contact_pick`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、Contact Picker API の
//! `navigator.contacts.select(['name'], { multiple: false })` で利用者に 1 件を選ばせ、
//! 表示名だけを画面へ書き戻す。契約の SSOT は `fandhe_device::contact_picker` のモジュール doc。
//! Web は `fandhe-device` の `ContactPicker` トレイトを通らない（`bluetooth_le.rs` と同じ理由）
//! ため、本モジュールが DOM レンダラ側の対応実装にあたる。
//!
//! # ブラウザ側の制約
//!
//! - **user activation 必須**: `select()` はボタン `click` の同期チェーン内で呼ぶ。
//!   `await` はこの呼び出しより前に挟まない。
//! - **secure context・Android Chrome 系のみ**: `navigator.contacts` / `select` が無い、または
//!   secure context 外は `Unsupported`。JS 側の実態は境界で型を検証する（security.md A03）。
//! - **web-sys の unstable API を避ける**: `Reflect` / `Function` だけで呼ぶ（Cargo.toml 変更なし）。
//!
//! # 三値の報告先
//!
//! - 成功: 先頭 1 件の `name[0]`。[`validate_contact_name`] を通し、空白だけなら `Failed`。
//!   名前は加工しない。名前が取れなければ偽の成功にせず `Failed`。
//! - キャンセル: 空配列が resolve された場合（ピッカーを閉じた）。失敗として見せない。
//! - 失敗: reject・同期 throw（固定文言）。`PermissionDenied` は使わない（権限ダイアログが
//!   なく、拒否を表す例外名もないため）。
//!
//! # 守備範囲
//!
//! 守る: 1 タブ内の要求と完了の順序（`finish_pick` の世代照合）・未対応や失敗を成功に見せない・
//! キャンセルを失敗に見せない。守らない: Android Chrome 系以外・複数タブの同時操作・
//! bfcache 退避中のピッカー（ブラウザ側が閉じる前提）・ページからピッカーを閉じる手段が
//! ないこと（期限なし）・名前以外の属性。
//!
//! 二重実行防止は不要（`bluetooth_le.rs` と同じ判断）: `mark_picking` 以降は `take_request` が
//! `None` を返し、`request_pick` は `Idle` 以外では no-op。
//!
//! 名前・例外の `message` は `console` 出力・送信・永続化をしない（security.md A09、契約表 8）。

use fandhe_core::screens::contact_pick::ContactPickOutcome;
use fandhe_device::contact_picker::validate_contact_name;

/// `SecurityError`（user activation 無し・非 secure context）のときの案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const SECURITY_FAILED: &str =
    "連絡先の呼び出しが許可されませんでした（ボタン操作・https が必要です）";
/// 種別を特定できない失敗の汎用案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const REQUEST_FAILED: &str = "連絡先の呼び出しに失敗しました";
/// 名前が取れない・空白だけのときの案内（成功にしない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const NAME_UNAVAILABLE: &str = "連絡先の名前を取得できませんでした";

/// 例外の `name` を画面の結果へ写す純関数（native からもテストできるよう gate の外）。
/// どの名前でも `Cancelled` にはしない。`message` は受け取らない（security.md A09）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_error_name(name: Option<&str>) -> ContactPickOutcome {
    match name {
        Some("SecurityError") => ContactPickOutcome::Failed(SECURITY_FAILED.to_string()),
        _ => ContactPickOutcome::Failed(REQUEST_FAILED.to_string()),
    }
}

/// resolve された選択結果（件数と先頭の名前）を画面の結果へ写す純関数。
/// 0 件はキャンセル、名前なし・空白だけは `Failed`、名前は加工しない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_selection(len: usize, first_name: Option<String>) -> ContactPickOutcome {
    if len == 0 {
        return ContactPickOutcome::Cancelled;
    }
    match first_name.map(validate_contact_name) {
        Some(Ok(c)) => ContactPickOutcome::Picked(c.display_name),
        _ => ContactPickOutcome::Failed(NAME_UNAVAILABLE.to_string()),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use fandhe_core::screens::contact_pick::{
        finish_pick, mark_picking, take_request, ContactPickOutcome, ContactPickState,
    };
    use js_sys::{Array, Function, Object, Promise, Reflect};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::{outcome_from_error_name, outcome_from_selection, REQUEST_FAILED};
    use crate::mount::Applier;

    fn get(target: &JsValue, name: &str) -> Option<JsValue> {
        Reflect::get(target, &JsValue::from_str(name))
            .ok()
            .filter(|v| !v.is_undefined() && !v.is_null())
    }

    fn error_name(err: &JsValue) -> Option<String> {
        get(err, "name").and_then(|v| v.as_string())
    }

    fn failed() -> ContactPickOutcome {
        ContactPickOutcome::Failed(REQUEST_FAILED.to_string())
    }

    /// resolve 値（`[{ name: [string] }]`）の型を確かめて結果へ写す。
    fn outcome_from_value(value: &JsValue) -> ContactPickOutcome {
        if !Array::is_array(value) {
            return failed();
        }
        let list: Array = value.clone().unchecked_into();
        let len = list.length() as usize;
        let first_name = (len > 0)
            .then(|| get(&list.get(0), "name"))
            .flatten()
            .filter(Array::is_array)
            .and_then(|n| n.unchecked_into::<Array>().get(0).as_string());
        outcome_from_selection(len, first_name)
    }

    /// `mount_with_effect` へ渡す effect。`contact_pick` 画面の選択要求を処理する。
    ///
    /// 要求が無ければ何もしない（`mount` モジュール doc の「effect フック」節の契約）。
    pub fn contact_pick_effect(state: &ContactPickState, applier: &Applier<ContactPickState>) {
        let Some(generation) = take_request(state) else {
            return;
        };
        applier.apply(mark_picking);

        let finish = |applier: &Applier<ContactPickState>, outcome: ContactPickOutcome| {
            applier.apply(move |s| finish_pick(s, generation, outcome));
        };

        let Some(window) = web_sys::window() else {
            finish(applier, ContactPickOutcome::Unsupported);
            return;
        };
        let navigator: JsValue = window.navigator().into();
        let contacts = get(&navigator, "contacts");
        let select = contacts
            .as_ref()
            .and_then(|c| get(c, "select"))
            .and_then(|f| f.dyn_into::<Function>().ok());
        let (Some(contacts), Some(select), true) = (contacts, select, window.is_secure_context())
        else {
            finish(applier, ContactPickOutcome::Unsupported);
            return;
        };

        let props = Array::of1(&JsValue::from_str("name"));
        let options = Object::new();
        if Reflect::set(&options, &JsValue::from_str("multiple"), &JsValue::FALSE).is_err() {
            finish(applier, failed());
            return;
        }

        // click の同期チェーン内での呼び出し（user activation 要件）。ここより前に await しない。
        let promise = match select.call2(&contacts, &props, &options) {
            Ok(p) => match p.dyn_into::<Promise>() {
                Ok(p) => p,
                Err(_) => {
                    finish(applier, failed());
                    return;
                }
            },
            Err(err) => {
                finish(
                    applier,
                    outcome_from_error_name(error_name(&err).as_deref()),
                );
                return;
            }
        };

        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = match JsFuture::from(promise).await {
                Ok(v) => outcome_from_value(&v),
                Err(err) => outcome_from_error_name(error_name(&err).as_deref()),
            };
            applier.apply(move |s| finish_pick(s, generation, outcome));
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::contact_pick_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_selection_is_cancelled() {
        assert_eq!(
            outcome_from_selection(0, None),
            ContactPickOutcome::Cancelled
        );
    }

    #[test]
    fn missing_or_blank_name_is_failed() {
        for n in [None, Some(String::new()), Some("  ".to_string())] {
            assert!(matches!(
                outcome_from_selection(1, n),
                ContactPickOutcome::Failed(_)
            ));
        }
    }

    #[test]
    fn name_is_not_trimmed() {
        assert_eq!(
            outcome_from_selection(1, Some(" 山田 ".to_string())),
            ContactPickOutcome::Picked(" 山田 ".to_string())
        );
    }

    #[test]
    fn errors_are_failures_never_cancelled() {
        for n in [
            Some("SecurityError"),
            Some("InvalidStateError"),
            Some("X"),
            None,
        ] {
            assert!(matches!(
                outcome_from_error_name(n),
                ContactPickOutcome::Failed(_)
            ));
        }
    }
}
