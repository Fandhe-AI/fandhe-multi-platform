//! 文字サイズ設定の追従の抽象（Phase 11 の拡充ループ、親 #911、Issue #912）。
//!
//! `Motion`/`Location` と同じく OS 設定を都度読む同期 API（push ではなく pull）。
//! 戻り値は `1.0` を OS 既定として、それより大きい/小さい値が拡大/縮小方向を表す倍率
//! （値域は [`SCALE_MIN`]`..=`[`SCALE_MAX`]）。**「読めない」を `1.0` で代用しない**契約が
//! 本モジュールの核（`biometric_auth.rs` の 4 値区別と同じ再発防止方針）。読めない
//! プラットフォーム・機種は必ず `Err(DeviceError::Unsupported("text_scale"))` を返し、
//! 呼び出し側（`fandhe-egui-proxy::device::text_scale`）はこれを「使えない」として
//! 「失敗」と区別して画面へ伝える。
//!
//! macOS は本 Issue の一次情報調査（Issue #912 実装計画 §2）の結果、第三者アプリ向けの
//! 公開 API が無いため [`UnsupportedTextScale`] を返す。Linux も恒久的に未対応。
//! Windows（`UISettings.TextScaleFactor`、Issue #913）・iOS/Android
//! （`UIContentSizeCategory`/`Configuration.fontScale`、Issue #914）・Web（`rem`、
//! Issue #915）は本トレイトの実装を追加する形で配線する（`fandhe-egui-proxy` 側に
//! `device/windows_text_scale.rs` 等を新設し、`apple/fandhe_apple_text_scale.m` は
//! iOS 向けに #914 が新規作成する想定。`build.rs` の `compile_apple_*` 群と同型で
//! 足せる、Issue #912 実装計画 §2 参照）。

use crate::DeviceError;

/// 倍率の下限（egui の `pixels_per_point` が過小にならないための上限側ガード）。
pub const SCALE_MIN: f32 = 0.5;
/// 倍率の上限（同様に過大にならないためのガード）。
pub const SCALE_MAX: f32 = 3.0;
/// OS 既定（追従設定なし）を表す倍率。
pub const DEFAULT_SCALE: f32 = 1.0;

/// `NaN` は [`DEFAULT_SCALE`] へ、それ以外（`±inf` を含む範囲外の値）は
/// [`SCALE_MIN`]`..=`[`SCALE_MAX`] へ丸める（`motion::clamp_finite` と同型。
/// `f32::clamp` は NaN をそのまま通すため先に NaN だけを弾く。`±inf` は
/// `f32::clamp` がそのまま境界値へ丸めるため素通しでよい、security.md A04）。
pub fn clamp_scale(value: f32) -> f32 {
    if value.is_nan() {
        DEFAULT_SCALE
    } else {
        value.clamp(SCALE_MIN, SCALE_MAX)
    }
}

/// 文字サイズ追従の契約。
pub trait TextScale: Send + Sync {
    /// OS の文字サイズ設定に基づく倍率を読む。読めない（本モジュール doc の契約）
    /// 場合は `Err(DeviceError::Unsupported("text_scale"))` を返す。
    fn scale_factor(&self) -> Result<f32, DeviceError>;
}

/// [`TextScale::scale_factor`] の戻り値を [`clamp_scale`] してから返す唯一の入口。
/// `Err` はそのまま透過する（呼び出し側は `Unsupported` を「使えない」として扱う）。
pub fn read_scale(ts: &dyn TextScale) -> Result<f32, DeviceError> {
    ts.scale_factor().map(clamp_scale)
}

/// 対応不可プラットフォーム用（Linux 恒久・macOS は本 Issue の判断、Windows/iOS/Android
/// は #913/#914 までの暫定）。常に `Unsupported("text_scale")` を返す。
pub struct UnsupportedTextScale;

impl TextScale for UnsupportedTextScale {
    fn scale_factor(&self) -> Result<f32, DeviceError> {
        Err(DeviceError::Unsupported("text_scale"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_returns_unsupported_not_default() {
        let err = read_scale(&UnsupportedTextScale).unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported("text_scale")));
    }

    struct MockTextScale(f32);

    impl TextScale for MockTextScale {
        fn scale_factor(&self) -> Result<f32, DeviceError> {
            Ok(self.0)
        }
    }

    #[test]
    fn read_scale_clamps_nan_and_out_of_range() {
        assert_eq!(read_scale(&MockTextScale(f32::NAN)).unwrap(), 1.0);
        assert_eq!(read_scale(&MockTextScale(0.1)).unwrap(), 0.5);
        assert_eq!(read_scale(&MockTextScale(10.0)).unwrap(), 3.0);
        assert_eq!(read_scale(&MockTextScale(f32::INFINITY)).unwrap(), 3.0);
        assert_eq!(read_scale(&MockTextScale(f32::NEG_INFINITY)).unwrap(), 0.5);
        assert_eq!(read_scale(&MockTextScale(-1.0)).unwrap(), 0.5);
    }

    #[test]
    fn clamp_scale_keeps_in_range_value() {
        assert_eq!(clamp_scale(1.3), 1.3);
    }
}
