//! 型サンプル「文字サイズ追従」の Web 経路（Issue #915）。
//!
//! `fandhe_core::screens::text_scale_preview`（端末依存ゼロ）の
//! [`fandhe_core::screens::text_scale_preview::OsScale::Unread`] を
//! [`super::mount::mount_with_effect`] の effect として観測し、
//! `getComputedStyle(document.documentElement).fontSize`（ブラウザの既定フォントサイズ、
//! `medium` = 100% のとき 16px）を読んで [`fandhe_core::screens::text_scale_preview::finish_read`]
//! へ書き戻す。対象は本クレート（DOM 出力）のみで、`fandhe-egui-proxy` の wasm
//! canvas 経路（egui 自前描画）は対象外（対応表 `docs/device/capabilities/dynamic-type.json`
//! の `web.note` 参照）。
//!
//! # 倍率を Rust 側から再適用しない理由
//!
//! Windows/iOS/Android の経路（`device/text_scale.rs`）は読んだ倍率を毎フレーム
//! `set_zoom_factor` で実描画へ押し込む必要があるが、Web の DOM 出力は
//! 見出し・本文が UA 既定（`h1 { font-size: 2em }` 等、`em`/`rem` 相当）を継承する
//! ため、ブラウザ自身が既定フォントサイズの変更をテキストへ反映済みである
//! （`html.rs`/`dom_tree.rs` に `font-size` の px 固定が無いことをテスト
//! `dom_output_has_no_pixel_font_size` で固定する）。ここで root へ倍率を
//! 再度掛けると二重拡大になるため、読んだ値は状態の表示用にのみ使う。
//!
//! # 再読込は `ResizeObserver`
//!
//! 既定フォントサイズの変更に対応する `resize`/media query は無いため、
//! `document.documentElement` を `ResizeObserver` で監視する（フォントサイズ変更で
//! テキストが再流し込みされ、対象要素の高さが変わることを利用する）。値が実際に
//! 変わったときだけ [`fandhe_core::screens::text_scale_preview::finish_read`] を
//! 再度呼ぶ（`Read → Read` の更新。
//! `text_scale_preview.rs` の doc 「変更通知を持つプラットフォームは値が変わった
//! ときに再度呼んでよい」契約に対応）。非対応ブラウザでは初回読み取りのみで動く。
//!
//! 例外の詳細（`message`）は `console` へ出さない（security.md A09、`share.rs` と同じ方針）。

/// CSS 既定フォントサイズ（`medium`、ブラウザ設定 100% のときの root `font-size`）。
// native ビルドでは `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る
// （`location.rs::failure_kind_from_code` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const DEFAULT_ROOT_FONT_PX: f32 = 16.0;

/// 既定フォントサイズの値を解釈できなかったときの案内。
/// `window`/`document`/`getComputedStyle` 自体が取得できない場合は
/// `OsScaleError::Unavailable`（メッセージ無しの「使えない」）へ倒すため、
/// 文言はこの「解釈できた値が不正」の場合にのみ使う。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const UNPARSEABLE: &str = "既定フォントサイズを解釈できませんでした";

/// `getComputedStyle(document.documentElement).fontSize`（例 `"20px"`）を倍率へ写す
/// 純関数（ブラウザ API を呼ばないため native からもテストできる、`location.rs::
/// failure_kind_from_code` と同じ理由で `wasm32` gate の外に置く）。
///
/// `px` 以外の単位・非有限・0 以下の値は `None`（呼び出し側が `Failed` にする。
/// `1.0` に化けさせない、security.md A04）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn scale_from_font_size(value: &str) -> Option<f32> {
    let px: f32 = value.trim().strip_suffix("px")?.trim().parse().ok()?;
    (px.is_finite() && px > 0.0).then_some(px / DEFAULT_ROOT_FONT_PX)
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::Cell;
    use std::rc::Rc;

    use fandhe_core::screens::text_scale_preview::{
        finish_read, OsScale, OsScaleError, TextScalePreviewState,
    };
    use fandhe_device::text_scale::clamp_scale;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    use super::{scale_from_font_size, UNPARSEABLE};
    use crate::mount::Applier;

    /// root 要素の既定フォントサイズを読み、[`OsScaleError`] へ写した結果を返す。
    fn read_ratio() -> Result<f32, OsScaleError> {
        let window = web_sys::window().ok_or(OsScaleError::Unavailable)?;
        let document = window.document().ok_or(OsScaleError::Unavailable)?;
        let root = document
            .document_element()
            .ok_or(OsScaleError::Unavailable)?;
        let style = window
            .get_computed_style(&root)
            .map_err(|_| OsScaleError::Unavailable)?
            .ok_or(OsScaleError::Unavailable)?;
        let value = style
            .get_property_value("font-size")
            .map_err(|_| OsScaleError::Failed(UNPARSEABLE.to_string()))?;
        scale_from_font_size(&value)
            .map(clamp_scale)
            .ok_or_else(|| OsScaleError::Failed(UNPARSEABLE.to_string()))
    }

    /// `mount_with_effect` へ渡す effect。`os_scale() == Unread` のときのみ読み取り、
    /// あわせて `ResizeObserver` を 1 度だけ登録する（要求が無ければ何もしない契約、
    /// `mount.rs` の「effect フック」節）。
    pub fn text_scale_effect(
        state: &TextScalePreviewState,
        applier: &Applier<TextScalePreviewState>,
    ) {
        if *state.os_scale() != OsScale::Unread {
            return;
        }

        let result = read_ratio();
        let initial = result.as_ref().ok().copied();
        applier.apply(move |s| finish_read(s, result));

        // `document.documentElement` はページ生存中不変なので、監視対象を
        // 取り直さず `Closure` を `forget` して生存させる（`location.rs` の
        // watch と同じ扱い。1 マウントにつき 1 件の固定天井）。
        // ponytail: マウント単位の明示アンマウント API が無いためリークは
        // ページ寿命に固定される。天井を超える場合は `mount.rs` にアンマウント
        // API を追加する設計に上げる。
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let Some(root) = document.document_element() else {
            return;
        };

        let last = Rc::new(Cell::new(initial));
        let applier = applier.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            // `finish_read` は `Read → Read`（値が実際に変わった場合のみ）しか
            // 許容しない契約（`text_scale_preview.rs` docs）。ここでの呼び出しは
            // 常に初回読み取り後（`os` は既に Read/Unavailable/Failed）なので、
            // 直近が成功していた場合にエラーを渡すと Read から Failed/Unavailable
            // へ退行し、直前の正常値をエラーで上書きしてしまう。成功した値の変化
            // のみを反映し、失敗は無視して直前の値を保つ。
            let Ok(value) = read_ratio() else {
                return;
            };
            if last.get() == Some(value) {
                return;
            }
            last.set(Some(value));
            applier.apply(move |s| finish_read(s, Ok(value)));
        });

        if let Ok(observer) = web_sys::ResizeObserver::new(closure.as_ref().unchecked_ref()) {
            observer.observe(&root);
            // `observer` 自身も生存させないと監視が即座に止まる（`Closure` と同じ理由）。
            std::mem::forget(observer);
        }
        closure.forget();
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::text_scale_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_pixel_value() {
        assert_eq!(scale_from_font_size("16px"), Some(1.0));
        assert_eq!(scale_from_font_size("20px"), Some(1.25));
        assert_eq!(scale_from_font_size("12px"), Some(0.75));
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(scale_from_font_size(" 24px "), Some(1.5));
    }

    #[test]
    fn rejects_non_pixel_or_invalid_values() {
        assert_eq!(scale_from_font_size("abc"), None);
        assert_eq!(scale_from_font_size(""), None);
        assert_eq!(scale_from_font_size("0px"), None);
        assert_eq!(scale_from_font_size("-16px"), None);
        assert_eq!(scale_from_font_size("NaNpx"), None);
        assert_eq!(scale_from_font_size("1.5rem"), None);
    }

    /// モジュール doc「倍率を Rust 側から再適用しない理由」の前提を固定する回帰テスト。
    /// DOM 出力（`to_html`）のテキストサイズが `px` 固定でないことを確認する
    /// （UA 既定 = `em`/`rem` 相当でブラウザの既定フォントサイズ変更に追従する）。
    #[test]
    fn dom_output_has_no_pixel_font_size() {
        use fandhe_core::screens::text_scale_preview::{
            text_scale_preview_screen, TextScalePreviewState,
        };

        let state = TextScalePreviewState::default();
        let screen = text_scale_preview_screen();
        let dom = crate::build_dom_tree(&screen, &state, 0);
        let html = crate::to_html(&dom);

        assert!(
            !html.contains("font-size"),
            "テキストサイズが固定指定されている: {html}"
        );
        assert!(
            !html.contains("px"),
            "テキスト要素に px 固定のスタイルが含まれている: {html}"
        );
    }
}
