//! Android 向けセーフエリア実装（Issue #446、親 #105）。契約は `fandhe_device::safe_area_insets`、
//! 呼び出し元は `device/safe_area_insets.rs` の Android 経路（dispatch は毎フレーム
//! [`implementation`] を呼び、画面側が 1 秒間隔で `read` する pull 型）。
//!
//! # 値の出どころ
//!
//! `MainActivity` の `ViewCompat.setOnApplyWindowInsetsListener`（`systemBars | displayCutout` の
//! 合計）→ JNI `nativeOnWindowInsetsChanged` → `android.rs` の static（物理 px、受信済みフラグつき）。
//! 本モジュールはそれを pull で読み、`ctx.native_pixels_per_point()` で dp に換算する。
//! JNI の署名・Java・dispatch は変えていない。
//!
//! # 結果
//!
//! | 状態 | 結果 |
//! |---|---|
//! | insets 未受信 / egui が ppp を未確定 | `Ok(None)`（画面は「読み取り中」。0 にしない） |
//! | ppp が非有限か 0 以下 | `Internal` |
//! | 負・巨大・非有限の値 | `Internal`（`insets_from_raw` に一本化。f32 へ丸める前に検査する） |
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 未受信を 0 にしないこと、値をログに出さないこと。毎回 static を読み直し、失敗後に前の値を返さない。
//! 守らない: 反映は最大 1 秒 + 1 フレーム遅れる、4 辺の組が 1 回混ざり得る、Activity 再生成の直後は
//! 前の値が残る、egui の zoom が 1 でないとき point とずれる、API 28 未満は切り欠きの概念がなく
//! `displayCutout` が 0（`systemBars` だけの値）。
//!
//! OS API: <https://developer.android.com/reference/androidx/core/view/WindowInsetsCompat>

#[cfg(target_os = "android")]
use std::sync::Arc;

#[cfg(target_os = "android")]
use fandhe_device::safe_area_insets::SafeAreaSource;
use fandhe_device::safe_area_insets::{insets_from_raw, Insets};
use fandhe_device::DeviceError;

/// 物理 px（`[top, right, bottom, left]`）を dp の [`Insets`] にする純関数。ホストのテストからも呼ぶ。
fn insets_from_px(
    px: Option<[i32; 4]>,
    native_ppp: Option<f32>,
) -> Result<Option<Insets>, DeviceError> {
    let (Some(px), Some(ppp)) = (px, native_ppp) else {
        return Ok(None);
    };
    if !ppp.is_finite() || ppp <= 0.0 {
        return Err(DeviceError::Internal("safe_area_insets read"));
    }
    let ppp = f64::from(ppp);
    insets_from_raw(0, px.map(|v| f64::from(v) / ppp)).map(Some)
}

/// insets の static を読む実装。ppp は生成時（毎フレーム）の値を持つ。
#[cfg(target_os = "android")]
struct AndroidSafeArea {
    native_ppp: Option<f32>,
}

#[cfg(target_os = "android")]
impl SafeAreaSource for AndroidSafeArea {
    fn read(&self) -> Result<Option<Insets>, DeviceError> {
        insets_from_px(crate::android::window_insets_px(), self.native_ppp)
    }
}

/// Android の実装を返す（換算用に `egui::Context` の ppp を受け取る）。
#[cfg(target_os = "android")]
pub(crate) fn implementation(ctx: &egui::Context) -> Arc<dyn SafeAreaSource> {
    Arc::new(AndroidSafeArea {
        native_ppp: ctx.native_pixels_per_point(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_internal<T>(r: Result<T, DeviceError>) -> bool {
        matches!(r, Err(DeviceError::Internal(_)))
    }

    #[test]
    fn 未受信は_none() {
        assert!(matches!(insets_from_px(None, Some(2.0)), Ok(None)));
    }

    #[test]
    fn ppp_未確定は_none() {
        assert!(matches!(insets_from_px(Some([1, 2, 3, 4]), None), Ok(None)));
    }

    #[test]
    fn ppp_不正は_internal() {
        for ppp in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(is_internal(insets_from_px(Some([1, 2, 3, 4]), Some(ppp))));
        }
    }

    #[test]
    fn 負と巨大は_internal() {
        assert!(is_internal(insets_from_px(Some([0, -1, 0, 0]), Some(2.0))));
        assert!(is_internal(insets_from_px(
            Some([0, 0, 0, 5000]),
            Some(2.0)
        )));
    }

    #[test]
    fn 辺ごとに換算する() {
        let got = insets_from_px(Some([96, 0, 48, 20]), Some(2.0))
            .unwrap()
            .unwrap();
        assert_eq!(
            got,
            Insets {
                top: 48.0,
                right: 0.0,
                bottom: 24.0,
                left: 10.0
            }
        );
    }

    #[test]
    fn 全辺_0_は成功() {
        let got = insets_from_px(Some([0; 4]), Some(2.0)).unwrap();
        assert_eq!(
            got,
            Some(Insets {
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0
            })
        );
    }
}
