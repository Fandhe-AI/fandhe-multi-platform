//! 画像内の文字認識（OCR）の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/image-recognition.json`、親 #1120）。
//!
//! 扱う範囲は「写真選択（[`crate::photo_picker`]）で選んだ画像 1 枚の文字を認識し、行の
//! 一覧を返す」ことだけ。位置枠・顔・矩形検出・カメラ入力は扱わない。対象は iOS / macOS
//! （Vision `VNRecognizeTextRequest`）、Android（ML Kit Text Recognition）、
//! Windows（`Windows.Media.Ocr.OcrEngine`）。Web は標準 API が無く対象外。
//! 一発呼び出し型の同期 API。呼び出し側（`fandhe-egui-proxy` の effect ドライバ）は
//! UI を止めないようワーカースレッドから呼ぶ。
//!
//! # 契約
//!
//! - **入力**: [`PickedPhoto`]（`validate_photo` 済みの JPEG / PNG 等のバイト列）。復号は
//!   OS 側に任せる。辺・画素数の上限検査は呼び出し側が OS を呼ぶ前に済ませる
//! - **出力**: 認識した行の文字列（[`normalize_lines`] 済み）。**空の一覧は「文字が
//!   無い」という成功**であり、失敗ではない
//! - **失敗の区別**: 非対応 OS → `Err(Unsupported("image_recognition"))`、認識言語が
//!   使えない（Apple の古い OS、Windows の OCR 言語パック未導入）→
//!   `Err(Unsupported(`[`LANGUAGE_UNAVAILABLE`]`))`、辺の上限超過 → `Err(InvalidInput)`、
//!   復号不能・OS 呼び出しの失敗 → `Err(Internal)`。失敗を空の成功へ畳まない
//!   （`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **権限は不要**: 画像は呼び出し側が渡すバイト列で、カメラ・写真ライブラリへ直接触れない。
//!   権限拒否の失敗系は無い
//! - **三値**（契約表 10）: 写真選択のキャンセルは「キャンセル」（失敗表示にしない）、
//!   文字が無いのは成功、それ以外は失敗。キャンセルは画面側（写真選択の段）で決まり、
//!   この trait は返さない
//!
//! # 守る範囲と守らない範囲
//!
//! 守る範囲: 1 画面・1 プロセス内での要求と完了の順序（画面の世代照合）、嘘の成功の
//! 禁止、ワーカーが持つバイト列の解放。守らない範囲: OS ごとの認識結果・行の分け方・
//! 順序の違い、処理時間（タイムアウトは掛けない）、実行中の OCR の取り消し（結果は
//! 世代照合で捨てる）、複数プロセス。認識した文字・画像はログへ出さない。

use crate::DeviceError;
use crate::photo_picker::PickedPhoto;

/// 認識言語が端末で使えないときの `Unsupported` の識別子（SSOT）。呼び出し側はこれと
/// 一致するかで「言語データ不足」と「非対応 OS」を見分ける。
pub const LANGUAGE_UNAVAILABLE: &str = "image_recognition language";

/// 文字認識の契約（モジュール doc の「契約」節）。
pub trait TextRecognizer: Send + Sync {
    /// `photo` の文字を認識し、[`normalize_lines`] 済みの行一覧を返す。0 行は成功。
    fn recognize(&self, photo: &PickedPhoto) -> Result<Vec<String>, DeviceError>;
}

/// 対象外 OS（Linux 等）・未実装 OS のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedTextRecognizer;

impl TextRecognizer for UnsupportedTextRecognizer {
    fn recognize(&self, _photo: &PickedPhoto) -> Result<Vec<String>, DeviceError> {
        Err(DeviceError::Unsupported("image_recognition"))
    }
}

/// OS が返した生の行を契約の形へ揃える: 前後空白を除き、空になる行を除く。順序と重複は保つ。
pub fn normalize_lines<I, S>(raw: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    raw.into_iter()
        .map(|t| t.as_ref().trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photo_picker::ImageFormat;

    #[test]
    fn normalize_drops_blank_lines_keeps_order_and_duplicates() {
        let out = normalize_lines(["  Hello ", "", "   ", "世界", "Hello"]);
        assert_eq!(out, ["Hello", "世界", "Hello"]);
    }

    #[test]
    fn normalize_keeps_punctuation_only_lines() {
        assert_eq!(normalize_lines(["!?", " - "]), ["!?", "-"]);
    }

    #[test]
    fn unsupported_recognizer_is_not_a_fake_success() {
        let photo = PickedPhoto {
            bytes: vec![0],
            format: ImageFormat::Png,
        };
        let err = UnsupportedTextRecognizer.recognize(&photo).unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported("image_recognition")));
    }
}
