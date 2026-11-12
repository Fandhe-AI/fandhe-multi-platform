//! macOS / iOS 向け機械学習推論実装（Issue #289 macOS、#291 iOS）。
//! `driver.rs` の macOS 経路と iOS 経路（`ios` 経由）から呼ばれ、`apple/fandhe_apple_ml_inference.m`
//! （Core ML。本 crate の `build.rs` がコンパイル）を `unsafe extern "C"` 1 関数で
//! 呼ぶ。`unsafe` はこの呼び出し 1 か所に閉じる（coding-rust.md の FFI 例外規定準拠）。
//! `build.rs` のコンパイル条件・`lib.rs` の mod cfg と同じ OS 集合に保つ。
//!
//! 契約の SSOT は `crate::contract`。モデルのバイト列は `coreml_spec` が実行時に組み、
//! `.m` へはポインタと長さだけを渡す。呼び出しはブロックするため、呼び出し元はワーカーで呼ぶ。

use std::time::Duration;

use crate::contract::{DemoModel, INVALID_MODEL, MlInference, TIMEOUT, coreml_spec};
use crate::error::MlInferenceError;

// SAFETY: `apple/fandhe_apple_ml_inference.m` が定義する C ABI（`build.rs` が macOS / iOS
// ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は `.m` 冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_ml_inference_run(
        spec: *const u8,
        len: usize,
        input: *const f64,
        n: usize,
        deadline_ms: u64,
        out_y: *mut f64,
    ) -> i32;
}

/// Core ML（CPU 固定）で推論する実装。
pub(crate) struct AppleMlInference;

impl MlInference for AppleMlInference {
    #[allow(unsafe_code)]
    fn infer(
        &self,
        model: DemoModel,
        input: [f32; 2],
        remaining: Duration,
    ) -> Result<f64, MlInferenceError> {
        let spec = coreml_spec(model);
        let input = input.map(f64::from);
        let mut y = 0.0_f64;
        let deadline_ms = u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX);
        // SAFETY: `spec`・`input`・`y` は呼び出しの間生存する領域で、`.m` は呼び出しの間だけ
        // 参照し保持しない（`spec` は `.m` 側でコピーするため、期限切れで戻った後に走る読み込みも安全）。`input` は 2 要素、`out_y` は 1 要素の書き込み先。
        let code = unsafe {
            fandhe_apple_ml_inference_run(
                spec.as_ptr(),
                spec.len(),
                input.as_ptr(),
                input.len(),
                deadline_ms,
                &mut y,
            )
        };
        match code {
            0 => Ok(y),
            1 => Err(MlInferenceError::Internal(INVALID_MODEL)),
            3 => Err(MlInferenceError::Internal(TIMEOUT)),
            4 => Err(MlInferenceError::Unsupported),
            _ => Err(MlInferenceError::Internal("ml_inference")),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::contract::{DEMO_INPUT, check_output, expected};

    const LONG: Duration = Duration::from_secs(30);

    #[test]
    fn valid_model_infers_24() {
        let y = AppleMlInference
            .infer(DemoModel::Valid, DEMO_INPUT, LONG)
            .expect("推論できる");
        assert_eq!(check_output(y, expected(DEMO_INPUT)).unwrap(), y);
    }

    #[test]
    fn corrupt_model_is_rejected_not_success() {
        let err = AppleMlInference
            .infer(DemoModel::Corrupt, DEMO_INPUT, LONG)
            .unwrap_err();
        assert!(
            matches!(err, MlInferenceError::Internal(INVALID_MODEL)),
            "{err:?}"
        );
    }
}
