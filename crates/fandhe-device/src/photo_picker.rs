//! 写真選択（既存の画像 1 枚の取り込み）の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/photo-picker.json`）。
//!
//! 扱う範囲は「画像を 1 枚選んで画面に表示する」ことだけ。複数選択・動画・EXIF・保存は
//! 扱わない。対象は iOS（`PHPickerViewController`）/ Android（`PickVisualMedia`、GMS 無しは
//! `ACTION_OPEN_DOCUMENT`）/ macOS（`PHPicker` または `NSOpenPanel`）/ Windows
//! （`IFileOpenDialog`）/ Web（`<input type=file accept="image/*">`）。
//!
//! 機能の契約（`PhotoPicker` trait）と OS 実装・effect ドライバは機能 crate `fandhe-cap-photo-picker` へ移した
//! （Issue #1355）。ここに残るのは、写真選択・文字認識（`fandhe-cap-image-recognition`）・バーコード読み取り
//! （`fandhe-cap-barcode-scan`）・書類スキャン・スキャナー機器・Web（`fandhe-web-dom`）が共有する、OS に触れない部品
//! （型・定数・[`validate_photo`]・寸法の検査・表示用の復号）だけで、`fandhe-device` から機能 crate へは依存しない
//! （`docs/operations/capability-crate-migration.md` §7）。
//!
//! request/response 型（要求 1 件を出して 1 件の結果を待つ）。OS がコールバック型のため
//! [`file_picker`](crate::file_picker) と同じく、橋渡し層（機能 crate・`fandhe-web-dom`）が
//! `crates/fandhe-runtime/src/request_slot.rs` の `RequestSlot` で要求を追跡し、OS コールバックから結果を書き戻す前提。
//! バイト列の読み取りは [`read_picked_from`](crate::file_picker::read_picked_from) に [`MAX_PHOTO_BYTES`] を
//! 渡して行い、読み取りの実装を増やさない。
//!
//! # 契約
//!
//! - **結果**: [`PickedPhoto`]（画像のバイト列 + 形式 [`ImageFormat`]）。形式は OS が名乗る
//!   MIME ではなく、バイト列の先頭（マジックナンバー）から [`validate_photo`] が決める。
//!   HEIC 等、表示側が復号できない形式は OS 側が JPEG / PNG へ変換して渡す。変換できない
//!   場合は成功にせず `Err(Internal)`
//! - **成功にしない場合（2 段で検査する）**:
//!   1. 受け取り時の検査（[`validate_photo`]、本クレート）: 0 バイト・形式不明 →
//!      `Err(Internal)`、[`MAX_PHOTO_BYTES`] 超過 → `Err(InvalidInput)`（縮小・切り詰めで
//!      成功にしない）。先頭のマジックナンバーしか見ないため、これを通っても復号できるとは
//!      限らない（ヘッダーだけ・途中で切れたデータは通りうる）
//!   2. 表示前の復号確認（橋渡し層。機能 crate / `fandhe-web-dom`）: 実際に
//!      画像として復号し、失敗したら `Err(Internal)` として画面へ返す。**利用者に成功として
//!      見せてよいのは 2 段目まで通ったものだけ**（`device-bridge-contracts.md`「6. 嘘の成功」）。
//!      復号は任意の feature `display-decode` の `decode_for_display` に 1 つにまとめた
//!      （既定では画像復号の依存を持たない。共通コアの依存を増やさないため）
//! - **キャンセルは `Err(Cancelled)`**（失敗として見せない。「10. 三値」）。ピッカーの
//!   起動失敗等は `Err(Internal)`、非対応 OS は `Err(Unsupported)`。権限は不要のため
//!   `PermissionDenied` は返さない
//! - **再要求は置き換え**: 保留中に新しい要求が来たら古い要求を置き換え、古い世代の
//!   結果・完了通知は `RequestSlot` の世代照合で捨てる（「2. 並行性・世代」）。OS 側に残る
//!   ピッカーは、実装が可能な範囲で閉じる
//! - **画面を離れる**: 保留中の要求は破棄する。破棄後に届いた結果は世代照合で捨て、
//!   最後の結果を返し続けない（「1. ライフサイクル境界」）。一時ファイル・ハンドルは
//!   成功・失敗・キャンセル・破棄のすべての経路で解放する（「9. 失敗時のリソース解放」）
//! - **完了通知は一度きり**。UI を要する提示（iOS の `PHPicker`、macOS の `NSOpenPanel`）は
//!   main queue / メインスレッドで行う
//!
//! # 守る範囲と既知の制約
//!
//! 守るのは 1 画面・1 プロセス内の要求と完了の順序、失敗・0 バイト・復号不能を成功と
//! 見せないこと、一時資源の解放。既知の制約（`AGENTS.md`「端末機能の並行性は守備範囲で
//! 判定する」）は次のとおり。
//!
//! - **Web のキャンセル検知はブラウザ差がある**: `<input type=file>` はキャンセル時に
//!   `change` が来ない。`cancel` イベント頼みで、未対応のブラウザでは保留のまま残りうる
//!   （次の要求による置き換えか画面離脱で破棄される）。
//! - **Android は Activity 再生成で結果を失いうる**: `registerForActivityResult` の結果は
//!   再生成をまたいで届かない場合がある。`RequestTracker` と同じ方針で、要求 id と
//!   照合できない結果は捨て、保留中の要求は復元せず破棄扱いにする（成功を作らない）
//! - 選ばれた画像の中身・ファイル名・パスはログに出さない。複数プロセス・複数タブでの
//!   同時操作と、OS 側で非同期に確定する選択との時間差は扱わない

use crate::file_picker::PickError;

/// 1 枚あたりのバイト数上限。読み取り側（`file_picker::read_picked_from`）と値をずらさないため
/// `file_picker::MAX_FILE_BYTES` を参照する（SSOT）。超過は `InvalidInput`。
pub const MAX_PHOTO_BYTES: usize = crate::file_picker::MAX_FILE_BYTES;

/// 表示側が復号できる画像形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// JPEG。
    Jpeg,
    /// PNG。
    Png,
    /// GIF。
    Gif,
    /// WebP。
    Webp,
}

impl ImageFormat {
    /// 対応する MIME タイプ。
    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

/// 選ばれた画像。`bytes` は空でなく `MAX_PHOTO_BYTES` 以下、先頭が `format` と一致する
/// ことを [`validate_photo`] が保証する（ファイル名・パスは持たない）。復号できることは
/// 保証しない（橋渡し層が表示前に復号して確かめる。モジュール doc の「契約」節の 2 段目）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedPhoto {
    /// 画像のバイト列。
    pub bytes: Vec<u8>,
    /// バイト列から判定した形式。
    pub format: ImageFormat,
}

/// バイト列の先頭から画像形式を判定する。対応外は `None`。
fn sniff_format(b: &[u8]) -> Option<ImageFormat> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(ImageFormat::Png)
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else {
        None
    }
}

/// OS から受け取ったバイト列を契約の形へ検証する（2 段検査の 1 段目）: 0 バイトと形式不明は
/// `Internal`、上限超過は `InvalidInput`。各 OS 実装が結果を返す直前に必ず通す。`Ok` は
/// 「復号できる」ことを意味しない（例: JPEG のヘッダー 3 バイトだけでも通る）。成功表示の前に
/// 橋渡し層が復号を確かめる。
pub fn validate_photo(bytes: Vec<u8>) -> Result<PickedPhoto, PickError> {
    if bytes.is_empty() {
        return Err(PickError::Internal("photo_picker empty"));
    }
    if bytes.len() > MAX_PHOTO_BYTES {
        return Err(PickError::InvalidInput("photo_picker size"));
    }
    let format = sniff_format(&bytes).ok_or(PickError::Internal("photo_picker format"))?;
    Ok(PickedPhoto { bytes, format })
}

/// 復号前に許す辺の長さ（画素）。表示用の復号（`decode_for_display`）の `image::Limits` もこの値を使う。
pub const MAX_PHOTO_SIDE: u32 = 16384;
/// 復号前に許す総画素数。RGBA 4 バイトで 256 MiB（[`MAX_DECODE_ALLOC`] と同じ）。
pub const MAX_PHOTO_PIXELS: u64 = 64 * 1024 * 1024;
/// 表示用の復号中の確保上限（バイト）。`MAX_PHOTO_PIXELS` 画素を RGBA 4 バイトで持った大きさ。
pub const MAX_DECODE_ALLOC: u64 = MAX_PHOTO_PIXELS * 4;
/// 表示用の複製の辺の上限（画素）。`RgbaPixels` の上限（2048）に収まり、egui の最大テクスチャ辺も避けられる。
pub const DISPLAY_MAX_SIDE: u32 = 1024;

fn be16(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?])))
}

fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some((be16(b, i)? << 16) | be16(b, i + 2)?)
}

fn le16(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?])))
}

fn le24(b: &[u8], i: usize) -> Option<u32> {
    Some(le16(b, i)? | (u32::from(*b.get(i + 2)?) << 16))
}

/// 画像ヘッダーだけから `(幅, 高さ)` を読む。復号しないため、巨大画像の展開前に上限を
/// 検査する橋渡し層（Web）が使う。読めないときは `None`（呼び出し側は失敗として扱う）。
pub fn image_dimensions(photo: &PickedPhoto) -> Option<(u32, u32)> {
    let b = &photo.bytes[..];
    match photo.format {
        ImageFormat::Png => Some((be32(b, 16)?, be32(b, 20)?)),
        ImageFormat::Gif => Some((le16(b, 6)?, le16(b, 8)?)),
        ImageFormat::Jpeg => {
            let mut i = 2;
            loop {
                if *b.get(i)? != 0xFF {
                    return None;
                }
                let m = *b.get(i + 1)?;
                match m {
                    0xFF => i += 1,
                    // SOF0〜SOF15（DHT / JPG / DAC を除く）
                    0xC0..=0xCF if !matches!(m, 0xC4 | 0xC8 | 0xCC) => {
                        return Some((be16(b, i + 7)?, be16(b, i + 5)?));
                    }
                    // TEM / RSTn / SOI / EOI は長さを持たない
                    0x01 | 0xD0..=0xD9 => i += 2,
                    _ => i += 2 + be16(b, i + 2)? as usize,
                }
            }
        }
        ImageFormat::Webp => match b.get(12..16)? {
            b"VP8 " => Some((le16(b, 26)? & 0x3FFF, le16(b, 28)? & 0x3FFF)),
            b"VP8L" => {
                let v = le24(b, 21)? | (u32::from(*b.get(24)?) << 24);
                Some(((v & 0x3FFF) + 1, ((v >> 14) & 0x3FFF) + 1))
            }
            b"VP8X" => Some((le24(b, 24)? + 1, le24(b, 27)? + 1)),
            _ => None,
        },
    }
}

/// 幅・高さが復号前の上限内か（0 は不正として弾く）。
pub fn dimensions_within_limits(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= MAX_PHOTO_SIDE
        && height <= MAX_PHOTO_SIDE
        && u64::from(width) * u64::from(height) <= MAX_PHOTO_PIXELS
}

/// OS（OCR・バーコード読み取り）へ画像を渡す前に、ヘッダーだけで辺・画素数を検査する（圧縮爆弾対策）。
/// 上限超過は `InvalidInput("photo_picker size")`、ヘッダーを読めない画像は `Internal("photo_picker header")`
/// （切り詰めて成功にしない）。文字認識とバーコード読み取りの両方が使う、OS に触れない検証。
pub fn precheck_dimensions(photo: &PickedPhoto) -> Result<(), PickError> {
    match image_dimensions(photo) {
        Some((w, h)) if dimensions_within_limits(w, h) => Ok(()),
        Some(_) => Err(PickError::InvalidInput("photo_picker size")),
        None => Err(PickError::Internal("photo_picker header")),
    }
}

/// 表示用に復号した画像。`image` は辺 [`DISPLAY_MAX_SIDE`] 以下へ縮めた画素、`width` / `height` は元の寸法。
#[cfg(feature = "display-decode")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayImage {
    /// 表示用の画素（縮小済み）。
    pub image: fandhe_core::RgbaPixels,
    /// 元画像の幅（画素）。
    pub width: u32,
    /// 元画像の高さ（画素）。
    pub height: u32,
}

/// 2 段検査の 2 段目: 実際に復号できるかを確かめ、表示用の画素を作る。失敗は `None`
/// （画像の中身・エラー文は持ち出さない。呼び出し側が自機能の失敗へ写す）。
///
/// 写真選択・書類スキャン・スキャナー機器が同じ復号を使う。辺 [`MAX_PHOTO_SIDE`]・確保 [`MAX_DECODE_ALLOC`] の上限を
/// `image::Limits` で掛け、超えたら `None`（成功にしない）。表示は [`DISPLAY_MAX_SIDE`] 以下へ縮めた複製で行い、報告する
/// 寸法は元のもの。PNG / JPEG のみ復号する（`image` の gif / webp feature は新しい依存を足すため採らない。GIF / WebP は `None`）。
#[cfg(feature = "display-decode")]
pub fn decode_for_display(bytes: &[u8], format: ImageFormat) -> Option<DisplayImage> {
    let format = match format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Gif | ImageFormat::Webp => return None,
    };
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    // `Limits` は non_exhaustive のため、既定値から書き換える。
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PHOTO_SIDE);
    limits.max_image_height = Some(MAX_PHOTO_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    let (width, height) = (decoded.width(), decoded.height());
    // thumbnail は小さい画像を拡大しうるため、大きいときだけ縮める。
    let shown = if width > DISPLAY_MAX_SIDE || height > DISPLAY_MAX_SIDE {
        decoded.thumbnail(DISPLAY_MAX_SIDE, DISPLAY_MAX_SIDE)
    } else {
        decoded
    };
    let rgba = shown.to_rgba8();
    // 表示辺 DISPLAY_MAX_SIDE 以下なので `RGBA_MAX_SIDE` に収まる。外れたら成功にせず失敗にする。
    let image = fandhe_core::RgbaPixels::new(rgba.width(), rgba.height(), rgba.into_raw())?;
    Some(DisplayImage {
        image,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_dimensions_from_headers() {
        let mut png = vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
        ];
        png.extend_from_slice(&40000u32.to_be_bytes());
        png.extend_from_slice(&30u32.to_be_bytes());
        let p = validate_photo(png).unwrap();
        assert_eq!(image_dimensions(&p), Some((40000, 30)));
        assert!(!dimensions_within_limits(40000, 30));

        let jpeg = vec![
            0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 11, 8, 0, 20, 0, 10,
        ];
        let p = validate_photo(jpeg).unwrap();
        assert_eq!(image_dimensions(&p), Some((10, 20)));

        let gif = b"GIF89a\x0a\x00\x14\x00".to_vec();
        assert_eq!(
            image_dimensions(&validate_photo(gif).unwrap()),
            Some((10, 20))
        );

        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp.extend_from_slice(&[9, 0, 0, 19, 0, 0]);
        assert_eq!(
            image_dimensions(&validate_photo(webp).unwrap()),
            Some((10, 20))
        );

        // 切れたヘッダーは None（呼び出し側は失敗にする）
        let cut = validate_photo(vec![0xFF, 0xD8, 0xFF]).unwrap();
        assert_eq!(image_dimensions(&cut), None);
        assert!(dimensions_within_limits(16384, 4096));
        assert!(!dimensions_within_limits(16384, 16384));
    }

    #[test]
    fn detects_each_supported_format() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0];
        let cases: [(&[u8], ImageFormat); 5] = [
            (&[0xFF, 0xD8, 0xFF, 0xE0], ImageFormat::Jpeg),
            (&png, ImageFormat::Png),
            (b"GIF89a....", ImageFormat::Gif),
            (b"GIF87a....", ImageFormat::Gif),
            (b"RIFF\0\0\0\0WEBPVP8 ", ImageFormat::Webp),
        ];
        for (bytes, want) in cases {
            assert_eq!(validate_photo(bytes.to_vec()).unwrap().format, want);
        }
    }

    #[test]
    fn rejects_empty_as_internal() {
        assert!(matches!(
            validate_photo(Vec::new()),
            Err(PickError::Internal(_))
        ));
    }

    #[test]
    fn rejects_unknown_format_as_internal() {
        assert!(matches!(
            validate_photo(b"not an image".to_vec()),
            Err(PickError::Internal(_))
        ));
        // RIFF だが WEBP ではない（WAV 等）。
        assert!(matches!(
            validate_photo(b"RIFF\0\0\0\0WAVEfmt ".to_vec()),
            Err(PickError::Internal(_))
        ));
    }

    #[test]
    fn rejects_over_limit_as_invalid_input() {
        let mut big = vec![0xFF, 0xD8, 0xFF];
        big.resize(MAX_PHOTO_BYTES + 1, 0);
        assert!(matches!(
            validate_photo(big),
            Err(PickError::InvalidInput(_))
        ));
    }

    #[test]
    fn accepts_exactly_limit() {
        let mut ok = vec![0xFF, 0xD8, 0xFF];
        ok.resize(MAX_PHOTO_BYTES, 0);
        assert!(validate_photo(ok).is_ok());
    }

    #[test]
    fn mime_matches_format() {
        assert_eq!(ImageFormat::Jpeg.mime(), "image/jpeg");
        assert_eq!(ImageFormat::Webp.mime(), "image/webp");
    }

    #[test]
    fn header_only_passes_stage_one_but_is_not_a_decode_guarantee() {
        // 1 段目はマジックナンバーしか見ない。復号の確認は橋渡し層（2 段目）の責務。
        let photo = validate_photo(vec![0xFF, 0xD8, 0xFF]).unwrap();
        assert_eq!(photo.format, ImageFormat::Jpeg);
    }

    fn png_with_size(w: u32, h: u32) -> PickedPhoto {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        validate_photo(b).expect("PNG と判定できる")
    }

    #[test]
    fn precheck_accepts_within_limits() {
        assert!(precheck_dimensions(&png_with_size(10, 10)).is_ok());
    }

    #[test]
    fn precheck_rejects_oversize_as_invalid_input() {
        let r = precheck_dimensions(&png_with_size(1, MAX_PHOTO_SIDE + 1));
        assert!(matches!(
            r,
            Err(PickError::InvalidInput("photo_picker size"))
        ));
    }

    #[test]
    fn precheck_rejects_unreadable_header_as_internal() {
        // IHDR を欠く PNG（先頭マジックだけ）はヘッダーを読めない。
        let p = validate_photo(vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]).unwrap();
        let r = precheck_dimensions(&p);
        assert!(matches!(r, Err(PickError::Internal("photo_picker header"))));
    }
}

#[cfg(all(test, feature = "display-decode"))]
mod decode_tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn valid_png_decodes_within_display_side() {
        let d = decode_for_display(&png(8, 4), ImageFormat::Png).unwrap();
        assert_eq!((d.width, d.height), (8, 4));
        assert_eq!((d.image.width(), d.image.height()), (8, 4));
    }

    #[test]
    fn large_image_is_shrunk_for_display_but_reports_original_size() {
        let d = decode_for_display(&png(2048, 16), ImageFormat::Png).unwrap();
        assert_eq!((d.width, d.height), (2048, 16));
        assert!(d.image.width() <= DISPLAY_MAX_SIDE);
    }

    #[test]
    fn truncated_png_fails() {
        let mut b = png(64, 64);
        b.truncate(b.len() / 2);
        assert!(decode_for_display(&b, ImageFormat::Png).is_none());
    }

    #[test]
    fn header_only_jpeg_passes_stage_one_but_fails_decode() {
        let p = validate_photo(vec![0xFF, 0xD8, 0xFF]).unwrap();
        assert!(decode_for_display(&p.bytes, p.format).is_none());
    }

    #[test]
    fn gif_and_webp_are_not_success() {
        assert!(decode_for_display(b"GIF89a....", ImageFormat::Gif).is_none());
        assert!(decode_for_display(b"RIFF\0\0\0\0WEBPVP8 ", ImageFormat::Webp).is_none());
    }

    #[test]
    fn side_over_limit_is_rejected() {
        // 1 辺 16385 の PNG（ほぼ空なので小さい）。上限超えは復号前に弾かれる。
        let b = png(MAX_PHOTO_SIDE + 1, 1);
        assert!(decode_for_display(&b, ImageFormat::Png).is_none());
    }
}
