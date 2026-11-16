//! 画像を 1 枚、写真ライブラリへ保存する機能（photo-library）の契約のうち、OS に触れず複数の機能が使う部品だけを置く
//! （Issue #1332。残りの契約・OS 実装は機能 crate `fandhe-cap-photo-library`）。
//!
//! 検査済みの PNG [`PngImage`] は、写真ライブラリへの保存（photo-library）と壁紙の設定（wallpaper-set）の両方が
//! OS へ渡す前の入力検査として使う（`consumer-app-structure.md` §8）。PNG でない・空・上限超過は
//! `InvalidInput("photo_library png")` で弾く。画像のバイト列はログにも画面にも出さない（security.md A09）。

use crate::DeviceError;

/// 受け付ける PNG の最大バイト数（OS へ渡す前の資源枯渇対策、security.md A04）。
pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;

pub const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// 検査済みの PNG。[`PngImage::new`] 以外では作れない（検査の SSOT、
/// `calendar::EventDraft::parse` と同じ方式）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PngImage(Vec<u8>);

impl PngImage {
    /// 空・PNG シグネチャ不一致・[`MAX_PNG_BYTES`] 超過を `InvalidInput("photo_library png")`
    /// として弾く（復号の成否までは見ない。OS 側が最終判断する）。
    pub fn new(bytes: Vec<u8>) -> Result<Self, DeviceError> {
        if bytes.is_empty() || bytes.len() > MAX_PNG_BYTES || !bytes.starts_with(&PNG_SIGNATURE) {
            return Err(DeviceError::InvalidInput("photo_library png"));
        }
        Ok(Self(bytes))
    }

    /// PNG のバイト列。
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_image_rejects_empty_non_png_and_oversize() {
        let mut big = PNG_SIGNATURE.to_vec();
        big.resize(MAX_PNG_BYTES + 1, 0);
        for bad in [vec![], b"not png".to_vec(), big] {
            assert!(matches!(
                PngImage::new(bad),
                Err(DeviceError::InvalidInput("photo_library png"))
            ));
        }
        assert!(PngImage::new(PNG_SIGNATURE.to_vec()).is_ok());
    }
}
