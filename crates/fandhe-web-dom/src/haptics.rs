//! Phase 11（親 #1090）型サンプル「触覚フィードバック」の Web 経路（Issue #1091）。
//!
//! `fandhe_core::screens::haptics_tap`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、`navigator.vibrate` で
//! 軽い振動を 1 回要求して結果を書き戻す。契約（`Ok` の意味・`Unsupported` の条件）の
//! SSOT は `fandhe_device::haptics`。Web は `fandhe-device` のトレイトを通らない
//! （`share.rs` と同じ理由）。
//!
//! # ブラウザ側の制約
//!
//! - `navigator.vibrate` が無い環境（Safari・Firefox デスクトップ等）は `Unsupported`。
//!   web-sys の型上は常に存在するため、`Reflect` で関数として実在するかを境界で確かめる
//!   （security.md A03）
//! - `vibrate` が `false` を返した（ユーザー操作の欠如・iframe 制限など）ときは失敗
//!   （成功にしない）
//! - `Ok` は「要求を渡した」まで。振動子の有無・サイレント設定は検知できない
//! - `vibrate` は同期 API。同じ effect 呼び出しの中で完了まで済ませ、非同期の飛行中管理は
//!   不要（`mark_sending` → `finish_tap` を 1 回の `apply` にまとめる）
//!
//! 例外・内部情報は `console` へ出さない（security.md A09）。

use fandhe_core::screens::haptics_tap::TapFailure;

/// `vibrate` が `false` を返したときの案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const REJECTED: &str = "ブラウザが振動の要求を受け付けませんでした";

/// `navigator.vibrate` の有無と戻り値を画面結果へ写す純関数（ブラウザ API を呼ばないため
/// native からもテストできる。`share.rs` の純関数と同じ理由で `wasm32` gate の外に置く）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn classify(has_vibrate: bool, accepted: bool) -> Result<(), TapFailure> {
    if !has_vibrate {
        return Err(TapFailure::Unsupported);
    }
    if !accepted {
        return Err(TapFailure::Failed(REJECTED.to_string()));
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
mod web {
    use fandhe_core::screens::haptics_tap::{
        finish_tap, mark_sending, take_request, HapticsTapState,
    };
    use js_sys::Reflect;
    use wasm_bindgen::JsValue;

    use super::classify;
    use crate::mount::Applier;

    /// 「軽い 1 回」として振動させる長さ（ミリ秒）。Android の標準的な軽い触覚（10〜20ms）
    /// に合わせる。
    const VIBRATE_MS: u32 = 15;

    /// `mount_with_effect` へ渡す effect。`haptics_tap` 画面の要求を処理する。
    pub fn haptics_tap_effect(state: &HapticsTapState, applier: &Applier<HapticsTapState>) {
        let Some(request) = take_request(state) else {
            return;
        };
        let generation = request.generation;

        let result = match web_sys::window() {
            None => classify(false, false),
            Some(window) => {
                let navigator = window.navigator();
                let navigator_value: JsValue = navigator.clone().into();
                let has_vibrate = Reflect::get(&navigator_value, &JsValue::from_str("vibrate"))
                    .map(|v| v.is_function())
                    .unwrap_or(false);
                // 無い環境では呼ばない（存在しない関数の呼び出しで例外になるのを避ける）。
                let accepted = has_vibrate && navigator.vibrate_with_duration(VIBRATE_MS);
                classify(has_vibrate, accepted)
            }
        };

        applier.apply(move |s| {
            mark_sending(s);
            finish_tap(s, generation, result);
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::haptics_tap_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_vibrate_is_unsupported() {
        assert_eq!(classify(false, false), Err(TapFailure::Unsupported));
        assert_eq!(classify(false, true), Err(TapFailure::Unsupported));
    }

    #[test]
    fn rejected_vibrate_is_failure_not_success() {
        assert!(matches!(classify(true, false), Err(TapFailure::Failed(_))));
    }

    #[test]
    fn accepted_vibrate_is_ok() {
        assert_eq!(classify(true, true), Ok(()));
    }
}
