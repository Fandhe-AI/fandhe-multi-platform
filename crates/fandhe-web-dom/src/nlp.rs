//! Phase 11（親 #1093）型サンプル「単語分割」の Web 経路（Issue #1094）。
//!
//! `fandhe_core::screens::nlp_segment`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、`Intl.Segmenter`
//! （granularity `word`）で分割して結果を書き戻す。契約（結果の形・失敗の区別）の SSOT は
//! `fandhe_device::nlp`。Web は `fandhe-device` のトレイトを通らない（`share.rs` と同じ理由）
//! ため、共有できる `validate_input` / `normalize_words` だけを直接使う。
//!
//! # ブラウザ側の制約
//!
//! - `Intl.Segmenter` が関数として無い環境は、入力に関わらず `Unsupported`
//!   （web-sys に型は無く、`Reflect` で実在を境界で確かめる。security.md A03）
//! - 判定順: API の有無 → `validate_input` → 空なら API を呼ばず `Ok(vec![])` → 分割 →
//!   `isWordLike` が真のものだけ残して `normalize_words`
//! - 構築・`segment`・反復で例外が出たら `Failed`（空リストの成功へ畳まない）。例外の中身は
//!   `console` へ出さない（security.md A09）
//! - 分割は同期 API。同じ effect 呼び出しの中で完了まで済ませる

use fandhe_core::screens::nlp_segment::SegmentFailure;
use fandhe_device::nlp::{normalize_words, validate_input};
use fandhe_device::DeviceError;

/// 分割の呼び出しが例外で失敗したときの案内（例外の中身は載せない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const SEGMENT_FAILED: &str = "ブラウザの単語分割に失敗しました";

/// `(segment, isWordLike)` の列を契約の形へ揃える純関数（ブラウザ API を呼ばないため
/// native からもテストできる。`haptics.rs` の純関数と同じ理由で `wasm32` gate の外に置く）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn words_from_segments(
    segments: impl IntoIterator<Item = (String, bool)>,
) -> Vec<String> {
    normalize_words(
        segments
            .into_iter()
            .filter_map(|(text, word_like)| word_like.then_some(text)),
    )
}

/// 判定順（モジュール doc）を実装する純関数。`call` は `Intl.Segmenter` 呼び出しで、
/// 例外は `Err(())` で受ける。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn segment_result(
    has_api: bool,
    text: &str,
    call: impl FnOnce(&str) -> Result<Vec<(String, bool)>, ()>,
) -> Result<Vec<String>, SegmentFailure> {
    if !has_api {
        return Err(SegmentFailure::Unsupported);
    }
    validate_input(text).map_err(|e| match e {
        DeviceError::InvalidInput(_) => SegmentFailure::InvalidInput,
        other => SegmentFailure::Failed(other.to_string()),
    })?;
    if text.is_empty() {
        return Ok(Vec::new());
    }
    call(text)
        .map(words_from_segments)
        .map_err(|()| SegmentFailure::Failed(SEGMENT_FAILED.to_string()))
}

#[cfg(target_arch = "wasm32")]
mod web {
    use fandhe_core::screens::nlp_segment::{finish_segment, take_request, NlpSegmentState};
    use js_sys::{Array, Function, Object, Reflect};
    use wasm_bindgen::{JsCast, JsValue};

    use super::segment_result;
    use crate::mount::Applier;

    /// グローバルの `Intl.Segmenter` コンストラクタ（関数として実在するときのみ）。
    fn segmenter_ctor() -> Option<Function> {
        let global = js_sys::global();
        let intl = Reflect::get(&global, &JsValue::from_str("Intl")).ok()?;
        let ctor = Reflect::get(&intl, &JsValue::from_str("Segmenter")).ok()?;
        ctor.dyn_into::<Function>().ok()
    }

    /// `Intl.Segmenter(undefined, {granularity: "word"}).segment(text)` を反復して
    /// `(segment, isWordLike)` の列を得る。どこかで例外・型不一致なら `Err(())`。
    fn call_segmenter(ctor: &Function, text: &str) -> Result<Vec<(String, bool)>, ()> {
        let options = Object::new();
        Reflect::set(
            &options,
            &JsValue::from_str("granularity"),
            &JsValue::from_str("word"),
        )
        .map_err(|_| ())?;
        let args = Array::of2(&JsValue::UNDEFINED, &options);
        let segmenter = Reflect::construct(ctor, &args).map_err(|_| ())?;
        let segment_fn: Function = Reflect::get(&segmenter, &JsValue::from_str("segment"))
            .map_err(|_| ())?
            .dyn_into()
            .map_err(|_| ())?;
        let segments = segment_fn
            .call1(&segmenter, &JsValue::from_str(text))
            .map_err(|_| ())?;
        let iter = js_sys::try_iter(&segments).map_err(|_| ())?.ok_or(())?;
        let mut out = Vec::new();
        for item in iter {
            let item = item.map_err(|_| ())?;
            let segment = Reflect::get(&item, &JsValue::from_str("segment"))
                .map_err(|_| ())?
                .as_string()
                .ok_or(())?;
            let word_like = Reflect::get(&item, &JsValue::from_str("isWordLike"))
                .map_err(|_| ())?
                .as_bool()
                .unwrap_or(false);
            out.push((segment, word_like));
        }
        Ok(out)
    }

    /// `mount_with_effect` へ渡す effect。`nlp_segment` 画面の要求を処理する。
    pub fn nlp_segment_effect(state: &NlpSegmentState, applier: &Applier<NlpSegmentState>) {
        let Some(request) = take_request(state) else {
            return;
        };
        let generation = request.generation;
        let ctor = segmenter_ctor();
        let result = segment_result(ctor.is_some(), &request.text, |text| {
            // `has_api` が真のときだけ呼ばれるため `ctor` は `Some`。
            ctor.as_ref().map_or(Err(()), |c| call_segmenter(c, text))
        });
        applier.apply(move |s| finish_segment(s, generation, result));
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::nlp_segment_effect;

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::nlp::MAX_INPUT_CHARS;

    fn seg(items: &[(&str, bool)]) -> Vec<(String, bool)> {
        items.iter().map(|(t, w)| (t.to_string(), *w)).collect()
    }

    #[test]
    fn non_word_like_and_symbol_only_segments_are_dropped() {
        let words = words_from_segments(seg(&[
            ("Hello", true),
            (", ", false),
            ("world", true),
            ("!", true),
            ("世界", true),
        ]));
        assert_eq!(words, ["Hello", "world", "世界"]);
    }

    #[test]
    fn order_and_duplicates_are_kept() {
        let words = words_from_segments(seg(&[("a", true), (" ", false), ("a", true)]));
        assert_eq!(words, ["a", "a"]);
    }

    #[test]
    fn missing_api_is_unsupported_even_for_empty_input() {
        for text in ["", "hello"] {
            let r = segment_result(false, text, |_| panic!("API 無しでは呼ばない"));
            assert_eq!(r, Err(SegmentFailure::Unsupported));
        }
    }

    #[test]
    fn invalid_input_is_rejected_before_calling_api() {
        let long = "a".repeat(MAX_INPUT_CHARS + 1);
        for text in [long.as_str(), "a\0b"] {
            let r = segment_result(true, text, |_| panic!("検証前に呼ばない"));
            assert_eq!(r, Err(SegmentFailure::InvalidInput));
        }
    }

    #[test]
    fn empty_input_is_empty_success_without_calling_api() {
        let r = segment_result(true, "", |_| panic!("空入力では呼ばない"));
        assert_eq!(r, Ok(vec![]));
    }

    #[test]
    fn api_exception_is_failure_not_empty_success() {
        let r = segment_result(true, "hello", |_| Err(()));
        assert!(matches!(r, Err(SegmentFailure::Failed(_))));
    }

    #[test]
    fn api_result_is_normalized() {
        let r = segment_result(true, "a b", |_| {
            Ok(seg(&[("a", true), (" ", false), ("b", true)]))
        });
        assert_eq!(r, Ok(vec!["a".to_string(), "b".to_string()]));
    }
}
