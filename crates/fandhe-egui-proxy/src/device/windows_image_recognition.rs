//! Windows 向け文字認識（Issue #1251）。WinRT `Windows.Media.Ocr.OcrEngine` を呼ぶ。
//!
//! 呼び出し元: `device/image_recognition.rs` の Windows 経路。写真選択とヘッダー検査
//! （辺・画素数）のあと、ワーカースレッド上で `recognize` を同期に呼ぶ。世代照合・
//! キャンセル・失敗の画面向け写像は呼び出し側が担い、本ファイルは「ブロックしてよい
//! 同期関数」だけを持つ。契約の SSOT は `fandhe_device::image_recognition`。
//!
//! 失敗の写像（全呼び出し箇所で一貫）:
//! - `TryCreateFromUserProfileLanguages` が null（OCR 言語パック未導入）→
//!   `Unsupported(LANGUAGE_UNAVAILABLE)`。windows-core は null を `HRESULT(0)` の
//!   エラーにするため、`code().is_ok()` で他の失敗と区別する
//! - それ以外（ストリーム・復号・認識の失敗）→ `Internal`（固定ラベルのみ。HRESULT・
//!   画像・認識文字はログにも画面にも出さない）
//!
//! `RoInitialize` は呼ばず（MTA フォールバック）、`unsafe` は使わない。`.get()` に期限は
//! 設けない（契約の「守らない範囲」）。認識言語はユーザープロファイル言語に従い、
//! 日本語 OCR 言語が無い端末では日本語を読めない（OS ごとの認識結果の違いとして守らない）。
//!
//! 既知の天井: unpackaged exe から `OcrEngine` が動くかは未確認で実機確認待ち（#1245）。
//! 型検査は `x86_64-pc-windows-gnu` のみ。

use fandhe_device::image_recognition::{normalize_lines, TextRecognizer, LANGUAGE_UNAVAILABLE};
use fandhe_device::photo_picker::PickedPhoto;
use fandhe_device::DeviceError;
use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
    ExifOrientationMode, SoftwareBitmap,
};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

/// Windows の文字認識（`OcrEngine`）。
pub(crate) struct WindowsTextRecognizer;

fn internal(label: &'static str) -> DeviceError {
    DeviceError::Internal(label)
}

/// バイト列を復号し、OCR の辺上限を超えるときは縦横比を保って縮小した bitmap を返す。
fn decode(bytes: &[u8], max_dim: u32) -> Result<SoftwareBitmap, DeviceError> {
    let stream_err = |_| internal("image_recognition stream");
    let stream = InMemoryRandomAccessStream::new().map_err(stream_err)?;
    let writer = DataWriter::CreateDataWriter(&stream).map_err(stream_err)?;
    writer.WriteBytes(bytes).map_err(stream_err)?;
    writer
        .StoreAsync()
        .map_err(stream_err)?
        .get()
        .map_err(stream_err)?;
    // drop 時に stream を閉じさせない。
    writer.DetachStream().map_err(stream_err)?;
    stream.Seek(0).map_err(stream_err)?;

    let dec_err = |_| internal("image_recognition decode");
    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(dec_err)?
        .get()
        .map_err(dec_err)?;
    let (w, h) = (
        decoder.PixelWidth().map_err(dec_err)?,
        decoder.PixelHeight().map_err(dec_err)?,
    );
    let transform = BitmapTransform::new().map_err(dec_err)?;
    let longest = w.max(h);
    // ponytail: 切り詰めず全体を縮小する。長辺は 90 度回転でも変わらないので EXIF 回転前の
    // 寸法で計算してよい。上流の検査（16384）は OCR の上限より大きいため、拒否せず縮小で受ける。
    if max_dim > 0 && longest > max_dim {
        let scale =
            |v: u32| ((u64::from(v) * u64::from(max_dim) / u64::from(longest)) as u32).max(1);
        transform.SetScaledWidth(scale(w)).map_err(dec_err)?;
        transform.SetScaledHeight(scale(h)).map_err(dec_err)?;
    }
    decoder
        .GetSoftwareBitmapTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &transform,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )
        .map_err(dec_err)?
        .get()
        .map_err(dec_err)
}

impl TextRecognizer for WindowsTextRecognizer {
    fn recognize(&self, photo: &PickedPhoto) -> Result<Vec<String>, DeviceError> {
        // 言語が無いときに無駄な復号をしないよう、エンジン生成を先に行う。
        let engine = match OcrEngine::TryCreateFromUserProfileLanguages() {
            Ok(e) => e,
            Err(e) if e.code().is_ok() => {
                return Err(DeviceError::Unsupported(LANGUAGE_UNAVAILABLE))
            }
            Err(_) => return Err(internal("image_recognition init")),
        };
        let max_dim = OcrEngine::MaxImageDimension().unwrap_or(0);
        let bitmap = decode(&photo.bytes, max_dim)?;

        let rec_err = |_| internal("image_recognition recognize");
        let result = engine
            .RecognizeAsync(&bitmap)
            .map_err(rec_err)?
            .get()
            .map_err(rec_err)?;
        // 取得に失敗した行を空文字へ畳まない（嘘の成功の防止）。
        let raw = result
            .Lines()
            .map_err(rec_err)?
            .into_iter()
            .map(|l| l.Text().map(|t| t.to_string_lossy()).map_err(rec_err))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(normalize_lines(raw))
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use fandhe_device::photo_picker::validate_photo;

    #[test]
    fn blank_image_is_empty_success() {
        let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([255, 255, 255, 255]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("PNG を書ける");
        let photo = validate_photo(out).unwrap();
        match WindowsTextRecognizer.recognize(&photo) {
            Ok(lines) => assert!(lines.is_empty()),
            // OCR 言語が無い環境
            Err(DeviceError::Unsupported(_)) => {}
            Err(e) => panic!("{e:?}"),
        }
    }

    #[test]
    fn corrupt_image_is_never_success() {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(&[0xFF; 32]);
        let photo = validate_photo(bytes).unwrap();
        let err = WindowsTextRecognizer.recognize(&photo).unwrap_err();
        assert!(
            matches!(err, DeviceError::Internal(_) | DeviceError::Unsupported(_)),
            "{err:?}"
        );
    }
}
