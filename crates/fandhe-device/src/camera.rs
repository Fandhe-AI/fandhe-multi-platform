//! カメラ撮影の抽象（Phase 8 / Issue #746）。実カメラアクセスはプラットフォーム固有
//! （AVFoundation / CameraX 等）のため、本クレートの共通実装は desktop 向けダミーのみを持つ
//! （ファイル選択ダイアログは依存追加になるため不採用、実装計画参照）。

use crate::{DeviceError, Image};

/// 撮影の契約。実装はスレッド間で共有されうる（`Storage`/`Http` と同じ理由、#745 参照）。
pub trait Camera: Send + Sync {
    /// 1 枚撮影して `Image` を返す。サイズが `MAX_IMAGE_BYTES` を超える、または
    /// `width * height` がオーバーフローする場合は `DeviceError::InvalidImageSize`
    /// を返す（前提条件なし・失敗は回復可能なエラーとして返す契約）。
    fn capture(&self) -> Result<Image, DeviceError>;
}

/// 画像 1 枚あたりの RGBA バイト数上限（メモリ枯渇防止・オーバーフロー対策、security.md A05）。
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

/// 固定の格子パターン RGBA を返すダミー実装（desktop 向け、実カメラ非搭載環境の動作確認用）。
pub struct DummyCamera {
    /// 撮影する画像の幅（ピクセル）。
    pub width: u32,
    /// 撮影する画像の高さ（ピクセル）。
    pub height: u32,
}

impl Camera for DummyCamera {
    fn capture(&self) -> Result<Image, DeviceError> {
        // 幅・高さのいずれかが 0 だと積が 0 になり後続の上限チェックを素通りしてしまう
        // （0 x 大きな高さ等でピクセルループが u32::MAX 回近く回りかねない）。
        // `InvalidImageSize` はゼロ寸法も対象と rustdoc に明記しているため、
        // 乗算前に弾いて実装とドキュメントを一致させる（Bugbot / codex-review 指摘対応）。
        if self.width == 0 || self.height == 0 {
            return Err(DeviceError::InvalidImageSize(self.width, self.height));
        }
        // width * height * 4 は u32 のままだとオーバーフローしうる
        // （例: 65536 x 65536 は release で確保量が暴走し、debug では panic する）。
        // checked_mul + 上限検証で過大要求を回復可能なエラーに変換する（P1 指摘対応）。
        let pixel_count = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or(DeviceError::InvalidImageSize(self.width, self.height))?;
        let byte_len = pixel_count
            .checked_mul(4)
            .filter(|&len| len <= MAX_IMAGE_BYTES)
            .ok_or(DeviceError::InvalidImageSize(self.width, self.height))?;
        let mut rgba = Vec::with_capacity(byte_len);
        for y in 0..self.height {
            for x in 0..self.width {
                // 8px 格子の市松模様（実カメラでないことが見て分かるダミー画像）。
                let on = ((x / 8) + (y / 8)) % 2 == 0;
                let v = if on { 220 } else { 40 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Ok(Image {
            width: self.width,
            height: self.height,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_length_matches_dimensions() {
        let camera = DummyCamera {
            width: 4,
            height: 3,
        };
        let image = camera.capture().expect("capture は成功する");
        assert_eq!(image.rgba.len(), 4 * 3 * 4);
    }

    #[test]
    fn oversized_dimensions_return_error_instead_of_overflow() {
        // 65536 x 65536 は u32 乗算がオーバーフローする組み合わせ
        // （P1 指摘の回帰テスト。修正前は release で暴走確保、debug で panic していた）。
        let camera = DummyCamera {
            width: 65536,
            height: 65536,
        };
        assert!(matches!(
            camera.capture(),
            Err(DeviceError::InvalidImageSize(65536, 65536))
        ));
    }

    #[test]
    fn zero_width_or_height_returns_error_instead_of_empty_image() {
        // 幅または高さが 0 だと積も 0 になり checked_mul 系のチェックを素通りするため、
        // 乗算前の明示チェックが効いているかを検証する（Bugbot / codex-review 指摘の回帰テスト）。
        let zero_width = DummyCamera {
            width: 0,
            height: 1,
        };
        assert!(matches!(
            zero_width.capture(),
            Err(DeviceError::InvalidImageSize(0, 1))
        ));

        let zero_height = DummyCamera {
            width: 1,
            height: 0,
        };
        assert!(matches!(
            zero_height.capture(),
            Err(DeviceError::InvalidImageSize(1, 0))
        ));
    }
}
