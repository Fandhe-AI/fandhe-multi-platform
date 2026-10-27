//! Android 向け壁紙設定の骨格（Issue #970、親 #188）。`Unsupported` を返すスタブで、#971
//! （b-android）が実体に置き換える。`device/wallpaper.rs` の Android 経路（pull 型 `bridge`）から
//! 呼ばれる。dispatch・画面・文言・対応表の他セルは (a) で確定済みのため、#971 は本ファイル以外の
//! 共通ファイルを触らない。
//!
//! # #971（b-android）への申し送り
//!
//! 触ってよい範囲: このファイル、`WallpaperHelper.java`、`MainActivity` の委譲、
//! `AndroidManifest.xml` の `SET_WALLPAPER`、対応表の Android セル。
//!
//! - JNI の署名は Java・no-op スタブ・Rust の 3 か所で一致させる
//! - 処理順: `isWallpaperSupported` → `isSetWallpaperAllowed`（false は `PermissionDenied`）→
//!   `setStream(..., null, false, which)`。`allowBackup` は false
//! - 結果: 戻り値 0・`IOException`・`SecurityException` は `Internal`。成功は `Ok(Home | Lock)`
//! - `RequestTracker` と executor は単一スレッド。`InputStream` は try-with-resources で閉じる
//! - 結果コード表とそのテストは本ファイルに置く

use fandhe_device::photo_library::PngImage;
use fandhe_device::wallpaper::WallpaperTarget;
use fandhe_device::DeviceError;

/// OS へ設定を依頼する。`Ok` は受理済みで結果は後から `bridge::deliver` で届く。
/// `Err` は依頼自体の失敗（呼び出し元がその場で `deliver` する）。現状は常に `Unsupported`。
pub(crate) fn begin_set(
    request_id: u64,
    image: &PngImage,
    target: WallpaperTarget,
) -> Result<(), DeviceError> {
    let _ = (request_id, image, target);
    Err(DeviceError::Unsupported("wallpaper"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_is_unsupported() {
        let mut b = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        b.push(0);
        let png = PngImage::new(b).expect("valid signature");
        for t in [WallpaperTarget::Home, WallpaperTarget::Lock] {
            assert!(matches!(
                begin_set(1, &png, t),
                Err(DeviceError::Unsupported("wallpaper"))
            ));
        }
    }
}
