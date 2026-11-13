//! iOS 向け文字サイズ追従実装（`UIFont.preferredFontForTextStyle:`、Issue #914）。
//! `apple/fandhe_apple_text_scale.m`（`build.rs` が iOS のときだけコンパイルする）の C ABI を呼ぶ。
//! `driver` の effect が毎フレーム pull する（push にしない判断は `contract` の doc 参照）。
//! `unsafe` はこの呼び出しに閉じ、`// SAFETY:` で契約を明記する。

use crate::contract::TextScale;
use crate::error::DynamicTypeError;

// SAFETY: `apple/fandhe_apple_text_scale.m` が定義する C ABI（`build.rs` が iOS ターゲットのときだけ
// コンパイル・リンクする）。契約は下記 `scale_factor` のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_text_scale_read(out_scale: *mut f32) -> i32;
}

/// iOS 実文字サイズ追従実装。
pub(crate) struct IosTextScale;

impl TextScale for IosTextScale {
    #[allow(unsafe_code)]
    fn scale_factor(&self) -> Result<f32, DynamicTypeError> {
        // `fandhe_apple_text_scale_read` は main スレッド専用（`.m` 冒頭コメント参照）。
        if !fandhe_platform_apple::is_main_thread() {
            return Err(DynamicTypeError::Internal);
        }
        let mut out_scale: f32 = 0.0;
        // SAFETY: out 引数はこの呼び出し中だけ有効なスタック上の `f32` へのポインタで、所有権移転・malloc は無い。
        // 戻り値が 0 以外のとき out 引数は書き換えられない契約のため、直前の初期値のまま扱う。
        let code = unsafe { fandhe_apple_text_scale_read(&mut out_scale) };
        match code {
            0 => Ok(out_scale),
            // 0 以外はすべて「読めない」（`.m` 側の異常系、`pointSize` が 0 以下等）。
            // NaN・範囲外の丸めは `read_scale` に一本化するため、ここでは値をそのまま返す。
            _ => Err(DynamicTypeError::Unsupported),
        }
    }
}
