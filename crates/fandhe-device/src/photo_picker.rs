//! 写真選択（既存の画像 1 枚の取り込み）の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/photo-picker.json`）。
//!
//! 扱う範囲は「画像を 1 枚選んで画面に表示する」ことだけ。複数選択・動画・EXIF・保存は
//! 扱わない。対象は iOS（`PHPickerViewController`）/ Android（`PickVisualMedia`、GMS 無しは
//! `ACTION_OPEN_DOCUMENT`）/ macOS（`PHPicker` または `NSOpenPanel`）/ Windows
//! （`IFileOpenDialog`）/ Web（`<input type=file accept="image/*">`）。
//!
//! request/response 型（要求 1 件を出して 1 件の結果を待つ）。OS がコールバック型のため
//! [`file_picker`](crate::file_picker) と同じく、橋渡し層（`fandhe-egui-proxy` の `device/`・
//! `fandhe-web-dom`）が `crates/fandhe-egui-proxy/src/device/request_slot.rs` の
//! `RequestSlot` で要求を追跡し、OS コールバックから結果を書き戻す前提。[`PhotoPicker`] の
//! 同期 API はデスクトップ向けで、モバイル / Web は本モジュールの型・定数・
//! [`validate_photo`] だけを共有する。バイト列の読み取りは
//! [`read_picked_from`](crate::file_picker::read_picked_from) に [`MAX_PHOTO_BYTES`] を
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
//!   2. 表示前の復号確認（橋渡し層。`fandhe-egui-proxy` / `fandhe-web-dom`）: 実際に
//!      画像として復号し、失敗したら `Err(Internal)` として画面へ返す。**利用者に成功として
//!      見せてよいのは 2 段目まで通ったものだけ**（`device-bridge-contracts.md`「6. 嘘の成功」）。
//!      本クレートは画像復号の依存を持たない（共通コアの依存を増やさないため）
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

use crate::DeviceError;
use crate::file_picker::PickedFile;

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

/// 写真選択の契約（モジュール doc の「契約」節）。デスクトップ向けの同期 API。
pub trait PhotoPicker: Send + Sync {
    /// 画像選択 UI を 1 回開き、選ばれた 1 枚を [`validate_photo`] 済みで返す。
    /// キャンセルは `Err(Cancelled)`。
    fn pick(&self) -> Result<PickedPhoto, DeviceError>;
}

/// 対象外 OS（Linux 等）・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedPhotoPicker;

impl PhotoPicker for UnsupportedPhotoPicker {
    fn pick(&self) -> Result<PickedPhoto, DeviceError> {
        Err(DeviceError::Unsupported("photo_picker"))
    }
}

/// デスクトップのダイアログが返す 3 値（選択 / キャンセル / 失敗）を [`PickedPhoto`] の契約へ写す
/// 純関数。キャンセルは `Cancelled`、選択は 1 段目 [`validate_photo`] を通す。OS を問わず
/// ホストでテストできるよう `cfg` を付けない。
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn photo_from_pick(
    picked: Result<Option<PickedFile>, DeviceError>,
) -> Result<PickedPhoto, DeviceError> {
    match picked? {
        None => Err(DeviceError::Cancelled("photo_picker")),
        Some(f) => validate_photo(f.bytes),
    }
}

/// Windows の `OpenFileDialog.Filter` に渡す固定フィルタ。デスクトップの復号が PNG / JPEG
/// のみ（依存を増やさないため。GIF / WebP は対象外）なので、ダイアログ側でも絞る。
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_PHOTO_FILTER: &str = "Images (*.png;*.jpg;*.jpeg)|*.png;*.jpg;*.jpeg";

/// macOS / Windows 向け実装。ファイル選択（`RfdFilePicker`）と同じ OS 分けを踏襲する:
/// macOS は `rfd`（`NSOpenPanel`）、Windows は PowerShell 子プロセス（終了コードで
/// 3 値を区別。rfd は失敗をキャンセルへ化けさせるため使わない）。UI スレッドから同期で呼ぶ
/// 契約（`NSOpenPanel` は main スレッド必須）。
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub struct RfdPhotoPicker {
    busy: std::sync::atomic::AtomicBool,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl RfdPhotoPicker {
    /// 構築は失敗しない。
    pub fn new() -> Self {
        Self {
            busy: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Default for RfdPhotoPicker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl PhotoPicker for RfdPhotoPicker {
    fn pick(&self) -> Result<PickedPhoto, DeviceError> {
        use std::sync::atomic::Ordering;
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(DeviceError::Internal("photo_picker busy"));
        }
        let _guard = crate::file_picker::BusyGuard(&self.busy);

        #[cfg(target_os = "macos")]
        let picked = match rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg"])
            .pick_file()
        {
            None => Ok(None),
            Some(path) => crate::file_picker::read_picked(&path, MAX_PHOTO_BYTES).map(Some),
        };
        #[cfg(target_os = "windows")]
        let picked = crate::file_picker::run_windows_pick(
            crate::file_picker::windows_pick_command(Some(WINDOWS_PHOTO_FILTER)),
            MAX_PHOTO_BYTES,
        );
        photo_from_pick(picked)
    }
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
pub fn validate_photo(bytes: Vec<u8>) -> Result<PickedPhoto, DeviceError> {
    if bytes.is_empty() {
        return Err(DeviceError::Internal("photo_picker empty"));
    }
    if bytes.len() > MAX_PHOTO_BYTES {
        return Err(DeviceError::InvalidInput("photo_picker size"));
    }
    let format = sniff_format(&bytes).ok_or(DeviceError::Internal("photo_picker format"))?;
    Ok(PickedPhoto { bytes, format })
}

/// 復号前に許す辺の長さ（画素）。デスクトップの `image::Limits`（`fandhe-egui-proxy`
/// `device/photo_pick.rs::MAX_SIDE`）と揃える。
pub const MAX_PHOTO_SIDE: u32 = 16384;
/// 復号前に許す総画素数。RGBA 4 バイトで 256 MiB（デスクトップの `max_alloc` と同じ）。
pub const MAX_PHOTO_PIXELS: u64 = 64 * 1024 * 1024;

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
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn rejects_unknown_format_as_internal() {
        assert!(matches!(
            validate_photo(b"not an image".to_vec()),
            Err(DeviceError::Internal(_))
        ));
        // RIFF だが WEBP ではない（WAV 等）。
        assert!(matches!(
            validate_photo(b"RIFF\0\0\0\0WAVEfmt ".to_vec()),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn rejects_over_limit_as_invalid_input() {
        let mut big = vec![0xFF, 0xD8, 0xFF];
        big.resize(MAX_PHOTO_BYTES + 1, 0);
        assert!(matches!(
            validate_photo(big),
            Err(DeviceError::InvalidInput(_))
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
    fn unsupported_picker_is_not_a_fake_success() {
        let err = UnsupportedPhotoPicker.pick().unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported(_)));
    }

    #[test]
    fn header_only_passes_stage_one_but_is_not_a_decode_guarantee() {
        // 1 段目はマジックナンバーしか見ない。復号の確認は橋渡し層（2 段目）の責務。
        let photo = validate_photo(vec![0xFF, 0xD8, 0xFF]).unwrap();
        assert_eq!(photo.format, ImageFormat::Jpeg);
    }

    fn file(bytes: &[u8]) -> PickedFile {
        PickedFile {
            name: "a".to_string(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn cancel_is_cancelled_not_success() {
        assert!(matches!(
            photo_from_pick(Ok(None)),
            Err(DeviceError::Cancelled(_))
        ));
    }

    #[test]
    fn pick_failure_passes_through() {
        assert!(matches!(
            photo_from_pick(Err(DeviceError::Internal("x"))),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn picked_file_goes_through_stage_one() {
        assert!(matches!(
            photo_from_pick(Ok(Some(file(b"")))),
            Err(DeviceError::Internal(_))
        ));
        assert!(matches!(
            photo_from_pick(Ok(Some(file(b"not an image")))),
            Err(DeviceError::Internal(_))
        ));
        let mut big = vec![0xFF, 0xD8, 0xFF];
        big.resize(MAX_PHOTO_BYTES + 1, 0);
        assert!(matches!(
            photo_from_pick(Ok(Some(file(&big)))),
            Err(DeviceError::InvalidInput(_))
        ));
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0];
        assert_eq!(
            photo_from_pick(Ok(Some(file(&png)))).unwrap().format,
            ImageFormat::Png
        );
    }

    #[cfg(unix)]
    fn sh(script: &str) -> crate::file_picker::PickCommand {
        crate::file_picker::PickCommand {
            program: std::path::PathBuf::from("/bin/sh"),
            args: vec!["-c".to_string(), script.to_string()],
        }
    }

    #[test]
    #[cfg(unix)]
    fn windows_style_cancel_failure_and_non_image_are_not_success() {
        use crate::file_picker::run_windows_pick;
        let run = |script: &str| photo_from_pick(run_windows_pick(sh(script), MAX_PHOTO_BYTES));
        // キャンセル専用コード 2。
        assert!(matches!(run("exit 2"), Err(DeviceError::Cancelled(_))));
        // それ以外の非ゼロは失敗。
        assert!(matches!(run("exit 1"), Err(DeviceError::Internal(_))));
        // 画像でないファイルは 1 段目で落ちる。
        let path = std::env::temp_dir().join(format!("fandhe-photo-test-{}", std::process::id()));
        std::fs::write(&path, b"plain text").unwrap();
        let r = run(&format!("printf '%s' '{}'", path.display()));
        let _ = std::fs::remove_file(&path);
        assert!(matches!(r, Err(DeviceError::Internal(_))), "{r:?}");
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn rfd_photo_picker_busy_is_internal() {
        let picker = RfdPhotoPicker::new();
        picker
            .busy
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(matches!(picker.pick(), Err(DeviceError::Internal(_))));
    }
}
