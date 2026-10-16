//! 型サンプル「OS 搭載フォント」の Web 経路（Issue #1379、親 #1145）。
//!
//! `fandhe_core::screens::local_fonts_show`（端末依存ゼロ）を [`super::mount::mount_with_effect`] の
//! effect として観測し、`window.queryLocalFonts()` の結果を `finish_read` へ書き戻す。
//! `fandhe-device` の `LocalFonts` トレイトは経由しない（`fandhe-device/src/local_fonts.rs` の
//! module doc どおり、Web は本クレートの担当）。整形・重複除去・昇順・上限は
//! `fandhe_device::local_fonts::{families_from, require_named}` を macOS / Windows と共用する。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外。範囲はファミリー名の一覧のみ。
//!
//! # 読み取り方
//!
//! - `queryLocalFonts()` はボタン押下の同期区間（effect 内）で呼ぶ（transient activation が要る。
//!   `spawn_local` の中では呼ばない）。起動時には読まない
//! - 各 `FontData` から `family` だけを `Reflect` で読む。`fullName` / `postscriptName` / `style` は
//!   読まず、`blob()` も呼ばない（フィンガープリントに使える情報を取らない）
//! - 非 secure context・`queryLocalFonts` 無しは `Unsupported`。`NotAllowedError` は
//!   `PermissionDenied`（失敗とは別）、`SecurityError`（permissions policy 等）は `Unsupported`
//! - `FontData` は書体単位なので件数は `total`（要素数）で数え、同じファミリーの重複は除く。
//!   要素があるのに名前が 1 件も取れなければ失敗（嘘の 0 件にしない）。0 要素は確定の 0 件。
//!   ただし Chrome は許可をブロックしても reject せず空配列で resolve するため、0 要素のときだけ
//!   `permissions.query({name: "local-fonts"})` を見て `denied` なら `PermissionDenied`。
//!   query が無い・名前を受け付けない環境は状態不明として 0 件のまま（追加の await 後の古い世代は
//!   `finish_read` が捨てる。この確認にも期限は設けない）
//! - 例外の `message` と名前は画面にも `console` にも出さない（security.md A09）
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 0 件と失敗の区別、切り捨ての明示、許可拒否と失敗の区別、古い世代の完了の破棄、
//! 名前をログに出さないこと、1 マウント内の発送記録の解放。
//! 守らない: Chromium 以外（非対応表示）、許可ダイアログの応答待ちの期限（ダイアログを読む
//! 利用者を失敗にしないため設けない）、読み取り後のフォント追加・削除（再取得まで）、
//! 複数タブ・複数マウント、ブラウザ間・OS 間の一覧の一致。

use fandhe_core::screens::local_fonts_show::{LocalFontsReading, ReadFailure};
use fandhe_device::local_fonts::{families_from, require_named};

/// 失敗時の固定文言（egui-proxy 側と同じ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const READ_FAILED: &str = "フォント一覧の読み取り";

/// `FontData` の要素数 `total` と各 `family` から表示用の読み取り結果を作る純関数。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn reading_from_names(
    total: usize,
    names: Vec<String>,
) -> Result<LocalFontsReading, ReadFailure> {
    require_named(total, families_from(names))
        .map(|f| LocalFontsReading {
            families: f.families,
            truncated: f.truncated,
        })
        .map_err(|_| ReadFailure::Failed(READ_FAILED.to_string()))
}

/// 0 要素で resolve したときの結果。許可状態（`permissions.query` の `state`）が `denied` なら
/// `PermissionDenied`（Chrome は拒否でも空配列で resolve する）、それ以外・不明は確定の 0 件。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn zero_result(state: Option<&str>) -> Result<LocalFontsReading, ReadFailure> {
    match state {
        Some("denied") => Err(ReadFailure::PermissionDenied),
        _ => reading_from_names(0, vec![]),
    }
}

/// 例外の `name` から失敗種別を決める純関数。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_name(name: &str) -> ReadFailure {
    match name {
        "NotAllowedError" => ReadFailure::PermissionDenied,
        "SecurityError" => ReadFailure::Unsupported,
        _ => ReadFailure::Failed(READ_FAILED.to_string()),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::local_fonts_show::{
        finish_read, mark_reading, take_request, LocalFontsShowState, ReadFailure,
    };
    use js_sys::{Array, Function, Reflect};
    use wasm_bindgen::prelude::*;

    use super::{failure_from_name, reading_from_names, zero_result, READ_FAILED};
    use crate::mount::{query_permission, Applier};

    thread_local! {
        /// マウントごとの発送済み世代（同じ世代を二重に読まない）。
        static DISPATCHED: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に発送記録を解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        DISPATCHED.with(|d| d.borrow_mut().remove(&mount_id));
    }

    fn get(target: &JsValue, name: &str) -> JsValue {
        Reflect::get(target, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
    }

    fn failure_of(e: &JsValue) -> ReadFailure {
        failure_from_name(&get(e, "name").as_string().unwrap_or_default())
    }

    /// `queryLocalFonts()` を同期で呼んで Promise を得る。呼べない環境は `Unsupported`、
    /// 同期例外は名前で分類、Promise でなければ `Failed`。
    fn start_query() -> Result<js_sys::Promise, ReadFailure> {
        let window = web_sys::window().ok_or(ReadFailure::Unsupported)?;
        if !window.is_secure_context() {
            return Err(ReadFailure::Unsupported);
        }
        let w: JsValue = window.into();
        let f = get(&w, "queryLocalFonts");
        if !f.is_function() {
            return Err(ReadFailure::Unsupported);
        }
        match f.unchecked_into::<Function>().call0(&w) {
            Ok(p) => p
                .dyn_into::<js_sys::Promise>()
                .map_err(|_| ReadFailure::Failed(READ_FAILED.to_string())),
            Err(e) => Err(failure_of(&e)),
        }
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ読む。
    pub fn local_fonts_show_effect(
        state: &LocalFontsShowState,
        applier: &Applier<LocalFontsShowState>,
    ) {
        let Some(req) = take_request(state) else {
            return;
        };
        let generation = req.generation;
        let mount_id = applier.mount_id();
        let fresh =
            DISPATCHED.with(|d| d.borrow_mut().insert(mount_id, generation) != Some(generation));
        if !fresh {
            return;
        }
        applier.apply(mark_reading);
        let promise = match start_query() {
            Ok(p) => p,
            Err(e) => {
                applier.apply(move |s| finish_read(s, generation, Err(e)));
                return;
            }
        };
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(list) if Array::is_array(&list) => {
                    let arr = Array::from(&list);
                    let names = arr
                        .iter()
                        .filter_map(|d| get(&d, "family").as_string())
                        .collect();
                    if arr.length() == 0 {
                        let status = query_permission("local-fonts").await;
                        let state = status.and_then(|s| get(&s, "state").as_string());
                        zero_result(state.as_deref())
                    } else {
                        reading_from_names(arr.length() as usize, names)
                    }
                }
                Ok(_) => Err(ReadFailure::Failed(READ_FAILED.to_string())),
                Err(e) => Err(failure_of(&e)),
            };
            applier.apply(move |s| finish_read(s, generation, result));
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::local_fonts_show_effect;

/// マウント破棄時に発送記録を解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::local_fonts::MAX_FAMILIES;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn zero_fonts_is_empty_success() {
        let r = reading_from_names(0, vec![]).unwrap();
        assert!(r.families.is_empty() && !r.truncated);
    }

    #[test]
    fn zero_result_denied_is_permission_denied() {
        assert_eq!(
            zero_result(Some("denied")),
            Err(ReadFailure::PermissionDenied)
        );
    }

    #[test]
    fn zero_result_other_states_are_empty_success() {
        for st in [Some("granted"), Some("prompt"), None] {
            let r = zero_result(st).unwrap();
            assert!(r.families.is_empty() && !r.truncated);
        }
    }

    #[test]
    fn all_unnamed_is_failure_not_zero() {
        assert_eq!(
            reading_from_names(3, s(&["", "\u{0}"])),
            Err(ReadFailure::Failed(READ_FAILED.to_string()))
        );
    }

    #[test]
    fn dedups_cleans_and_sorts() {
        let r = reading_from_names(4, s(&["B", " A\u{0} ", "A", "B"])).unwrap();
        assert_eq!(r.families, ["A", "B"]);
    }

    #[test]
    fn truncates_over_max() {
        let names: Vec<String> = (0..=MAX_FAMILIES).map(|i| format!("f{i:05}")).collect();
        let r = reading_from_names(names.len(), names).unwrap();
        assert!(r.truncated);
        assert_eq!(r.families.len(), MAX_FAMILIES);
    }

    #[test]
    fn classifies_exception_names() {
        assert_eq!(
            failure_from_name("NotAllowedError"),
            ReadFailure::PermissionDenied
        );
        assert_eq!(failure_from_name("SecurityError"), ReadFailure::Unsupported);
        for n in ["", "TypeError"] {
            assert_eq!(
                failure_from_name(n),
                ReadFailure::Failed(READ_FAILED.to_string())
            );
        }
    }
}
