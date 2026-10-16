//! 型サンプル「印刷」の Web 経路（Issue #329、親 #94）。
//!
//! `fandhe_core::screens::print_page`（端末依存ゼロ）を [`super::mount::mount_with_effect`] の
//! effect として観測し、`body` 直下の印刷用ブロックを組んで `window.print()` を呼び、結果を
//! `finish_print` へ書き戻す。`fandhe-device` の `Print` トレイトは経由せず、検証は
//! `PrintRequest::new`（macOS・Windows と共用）に一本化する。`fandhe-device` module doc の
//! 「非対応（Web ほか）→ `Unsupported`」は `fandhe-egui-proxy` の wasm canvas 経路の話で、
//! 本モジュールとは別経路（そちらは `Unsupported` のまま）。
//!
//! # 印刷用ブロック
//!
//! `#fandhe-print-block` を `document.body` 直下（`#fandhe-dom-root` の外）に置く。DOM レンダラの
//! 再描画で消えない。中身は `<style>`（固定定数 `PRINT_CSS`）・`<h1>`・各行の `<p>` で、
//! すべて `create_element` + `set_text_content` で組む（`innerHTML` は使わない、OWASP A03）。
//! 画面上は `display: none`、印刷時はこのブロックだけを出す。`<style>` をブロック内に入れるので、
//! ブロックが無いとき（ほかの画面で Ctrl+P）は印刷結果に影響せず、ホストページの CSS にも依存しない。
//! ブロックはページ内で常に 1 つ（作成時に既存の同 ID 要素を外す。複数マウントでも二重に出ない）。
//! 寿命は次の要求か画面の破棄まで（`print()` が例外なら即解放）（モバイルは `print()` がすぐ戻り非同期で印刷するため、
//! 戻った直後に消すと白紙になる）。
//!
//! # 結果の写像
//!
//! - 入力不正: `print()` を呼ばず `Invalid`（文言は `fandhe-egui-proxy/src/device/print.rs` の
//!   `to_outcome` と同期する）
//! - `window` が無い: `Unsupported`
//! - `document` / `body` が無い・ブロック作成失敗・`print()` が例外: 固定文言の `Failed`
//!   （例外の `message` と印刷内容は画面にも `console` にも出さない、security.md A09）
//! - `print()` が例外なく戻った: `Unconfirmed`（印刷したか取り消したかをブラウザは返さない）
//!
//! `Submitted` / `Cancelled` は返さない（嘘の成功を作らない）。呼び出しは同期で、クリックから
//! effect まで同期に繋がる中で `print()` が走る。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の要求と完了の順序（発送済み世代の記録と `finish_print` の世代照合）、
//! 入力検証、`Unconfirmed` を印刷済みと見せないこと、マウント破棄・失敗時のブロック解放。
//! 守らない: sandbox（`allow-modals` なし）やブラウザ設定で `print()` が黙って無視される場合、
//! 紙に出たか、ブラウザごとの余白・書体・ヘッダーの差、複数タブ、wasm canvas 経路。

use fandhe_core::screens::print_page::{CoreOutcome, PrintFailure, PrintRequestView};
use fandhe_device::print::PrintRequest;
use fandhe_device::DeviceError;

/// 印刷用ブロックの `id`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const PRINT_BLOCK_ID: &str = "fandhe-print-block";

/// 印刷用 CSS（固定。外部入力を連結しない）。画面では隠し、印刷ではブロックだけを出す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const PRINT_CSS: &str = "@media screen { #fandhe-print-block { display: none; } } \
@media print { body > :not(#fandhe-print-block) { display: none !important; } \
#fandhe-print-block { display: block; } }";

/// 印刷を開始できなかったときの固定文言（例外の内容は含めない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const PRINT_FAILED: &str = "OS の印刷を開始できませんでした";

/// 画面の要求を検証済みの `PrintRequest` にする。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn build_request(view: &PrintRequestView) -> Result<PrintRequest, DeviceError> {
    PrintRequest::new(&view.title, &view.body)
}

/// 入力不正を画面用の失敗にする。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn invalid_failure(e: DeviceError) -> PrintFailure {
    PrintFailure::Invalid(
        match e {
            DeviceError::InvalidInput("print title") => "見出し",
            DeviceError::InvalidInput("print body") => "本文",
            _ => "入力",
        }
        .to_string(),
    )
}

/// `print()` の結果の写像。戻れば常に `Unconfirmed`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn outcome_from_print(r: Result<(), ()>) -> Result<CoreOutcome, PrintFailure> {
    match r {
        Ok(()) => Ok(CoreOutcome::Unconfirmed),
        Err(()) => Err(PrintFailure::Failed(PRINT_FAILED.to_string())),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::print_page::{
        finish_print, mark_printing, take_request, CoreOutcome, PrintFailure, PrintPageState,
    };
    use fandhe_device::print::PrintRequest;
    use web_sys::{Document, Element};

    use super::{
        build_request, invalid_failure, outcome_from_print, PRINT_BLOCK_ID, PRINT_CSS, PRINT_FAILED,
    };
    use crate::mount::Applier;

    thread_local! {
        /// マウントごとの（発送済み世代, 印刷用ブロック）。同じ世代を二重に印刷させない。
        static DISPATCHED: RefCell<HashMap<u64, (u64, Option<Element>)>> =
            RefCell::new(HashMap::new());
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に記録とブロックを解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        if let Some((_, Some(block))) = DISPATCHED.with(|d| d.borrow_mut().remove(&mount_id)) {
            block.remove();
        }
    }

    fn text_child(doc: &Document, parent: &Element, tag: &str, text: &str) -> Result<(), ()> {
        let el = doc.create_element(tag).map_err(|_| ())?;
        el.set_text_content(Some(text));
        parent.append_child(&el).map(|_| ()).map_err(|_| ())
    }

    /// 印刷用ブロックを組んで body へ付ける。失敗時は組みかけを外す。
    fn build_block(doc: &Document, req: &PrintRequest) -> Result<Element, ()> {
        let body = doc.body().ok_or(())?;
        // 印刷対象はページ内で 1 つ。別マウントが保持するブロックも外す（二重印刷の防止）。
        while let Some(old) = doc.get_element_by_id(PRINT_BLOCK_ID) {
            old.remove();
        }
        let block = doc.create_element("div").map_err(|_| ())?;
        block.set_id(PRINT_BLOCK_ID);
        let built = (|| {
            text_child(doc, &block, "style", PRINT_CSS)?;
            text_child(doc, &block, "h1", req.title())?;
            for line in req.lines() {
                text_child(doc, &block, "p", line)?;
            }
            body.append_child(&block).map(|_| ()).map_err(|_| ())
        })();
        if built.is_err() {
            block.remove();
            return Err(());
        }
        Ok(block)
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ印刷を依頼する。
    pub fn print_page_effect(state: &PrintPageState, applier: &Applier<PrintPageState>) {
        let Some(view) = take_request(state) else {
            return;
        };
        let generation = view.generation;
        let mount_id = applier.mount_id();
        let fresh = DISPATCHED.with(|d| {
            let mut d = d.borrow_mut();
            if d.get(&mount_id).map(|e| e.0) == Some(generation) {
                return false;
            }
            // 古いブロックは新しい要求で置き換える（世代の記録は先に更新する）。
            if let Some((_, Some(old))) = d.insert(mount_id, (generation, None)) {
                old.remove();
            }
            true
        });
        if !fresh {
            return;
        }
        applier.apply(mark_printing);
        let result = match build_request(&view) {
            Err(e) => Err(invalid_failure(e)),
            Ok(req) => match web_sys::window() {
                None => Ok(CoreOutcome::Unsupported),
                Some(w) => match w.document().map(|doc| build_block(&doc, &req)) {
                    None | Some(Err(())) => Err(PrintFailure::Failed(PRINT_FAILED.to_string())),
                    Some(Ok(block)) => {
                        // print() 中の beforeprint/afterprint から同じ root が再マウントされると
                        // release_mount が走る。呼ぶ前にブロックを記録し、破棄時に解放できるようにする。
                        DISPATCHED.with(|d| {
                            if let Some(e) = d.borrow_mut().get_mut(&mount_id) {
                                e.1 = Some(block.clone());
                            }
                        });
                        let printed = w.print().map_err(|_| ());
                        // 失敗したブロックを残すと通常の Ctrl+P に混ざる。発送済み世代だけ残す。
                        // 復帰後にマウントが破棄済み（記録なし）か世代が違えば、このブロックは孤児なので外す。
                        let keep = printed.is_ok()
                            && DISPATCHED.with(|d| {
                                d.borrow().get(&mount_id).map(|e| e.0) == Some(generation)
                            });
                        if !keep {
                            block.remove();
                            DISPATCHED.with(|d| {
                                if let Some(e) = d.borrow_mut().get_mut(&mount_id) {
                                    if e.0 == generation {
                                        e.1 = None;
                                    }
                                }
                            });
                        }
                        outcome_from_print(printed)
                    }
                },
            },
        };
        applier.apply(move |s| finish_print(s, generation, result));
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::print_page_effect;

/// マウント破棄時に記録とブロックを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::print_page::{print, take_request, PrintPageState};

    fn view() -> PrintRequestView {
        let mut s = PrintPageState::default();
        print(&mut s);
        take_request(&s).unwrap()
    }

    #[test]
    fn fixed_values_build() {
        assert!(build_request(&view()).is_ok());
    }

    #[test]
    fn invalid_input_maps_to_labels() {
        let mut v = view();
        v.title = String::new();
        assert_eq!(
            invalid_failure(build_request(&v).unwrap_err()),
            PrintFailure::Invalid("見出し".into())
        );
        let mut v = view();
        v.body = "a\rb".to_string();
        assert_eq!(
            invalid_failure(build_request(&v).unwrap_err()),
            PrintFailure::Invalid("本文".into())
        );
        assert_eq!(
            invalid_failure(DeviceError::InvalidInput("x")),
            PrintFailure::Invalid("入力".into())
        );
    }

    #[test]
    fn print_maps_without_faking_success() {
        assert_eq!(outcome_from_print(Ok(())), Ok(CoreOutcome::Unconfirmed));
        assert_eq!(
            outcome_from_print(Err(())),
            Err(PrintFailure::Failed(PRINT_FAILED.to_string()))
        );
    }

    #[test]
    fn css_targets_block() {
        assert!(PRINT_CSS.contains("@media print"));
        assert!(PRINT_CSS.contains(PRINT_BLOCK_ID));
    }
}
