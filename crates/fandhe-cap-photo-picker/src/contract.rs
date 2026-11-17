//! 写真選択（既存の画像 1 枚の取り込み）の契約（Phase 11 の拡充ループ、対応表 `docs/device/capabilities/photo-picker.json`）。
//!
//! 扱う範囲は「画像を 1 枚選んで画面に表示する」ことだけ。複数選択・動画・EXIF・保存は扱わない。[`PhotoPicker`] トレイトの同期契約は
//! デスクトップ（macOS / Windows）専用で、iOS・Android は OS のピッカーがコールバック型のため trait を通らず、`driver` の pull 型
//! `bridge` で結果を受ける（`driver` のモジュール doc）。Web は `fandhe-web-dom` が受ける。
//!
//! 結果の型・検証・上限（[`PickedPhoto`]・[`validate_photo`]・[`MAX_PHOTO_BYTES`]）と 3 値（選択・キャンセル・失敗）・2 段検査・
//! 守る範囲と既知の制約の SSOT は `fandhe_device::photo_picker` のモジュール doc（文字認識・バーコード読み取り・Web も同じ部品を使う
//! ため `fandhe-device` に残した。`docs/operations/capability-crate-migration.md` §7）。この crate はそれを再公開して使う。
//!
//! # 契約（この trait の実装・呼び出し元が守る不変条件）
//!
//! - 1 回の [`PhotoPicker::pick`] = 1 回のダイアログ。ユーザーが閉じるまで確定しない。固定タイムアウトを掛けない
//! - 結果は選択 `Ok(PickedPhoto)`（[`validate_photo`] 済み。復号できることは保証しない）/ キャンセル `Err(Cancelled)`（失敗として見せない）/
//!   失敗 `Err`。非対応 OS は `Err(Unsupported)`、起動失敗・0 バイト・形式不明は `Err(Internal)`、上限超過は `Err(InvalidInput)`
//! - 進行中に再度呼ばれたら `Err(Internal("photo_picker busy"))`（二重にダイアログを開かない）
//! - macOS の `NSOpenPanel` はメインスレッド必須。呼び出し元は UI スレッドから同期で呼ぶ

use crate::error::PhotoPickerError;

pub use fandhe_device::file_picker::PickedFile;
pub use fandhe_device::photo_picker::{
    ImageFormat, MAX_PHOTO_BYTES, MAX_PHOTO_PIXELS, MAX_PHOTO_SIDE, PickedPhoto,
    dimensions_within_limits, image_dimensions, validate_photo,
};

/// 写真選択の契約（モジュール doc の「契約」節）。デスクトップ向けの同期 API。
pub trait PhotoPicker: Send + Sync {
    /// 画像選択 UI を 1 回開き、選ばれた 1 枚を [`validate_photo`] 済みで返す。キャンセルは `Err(Cancelled)`。
    fn pick(&self) -> Result<PickedPhoto, PhotoPickerError>;
}

/// 対象外 OS（Linux 等）・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedPhotoPicker;

impl PhotoPicker for UnsupportedPhotoPicker {
    fn pick(&self) -> Result<PickedPhoto, PhotoPickerError> {
        Err(PhotoPickerError::Unsupported("photo_picker"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_picker_is_not_a_fake_success() {
        let err = UnsupportedPhotoPicker.pick().unwrap_err();
        assert!(matches!(err, PhotoPickerError::Unsupported(_)));
    }
}
