//! 画像内のバーコード・QR 読み取りの抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/barcode-scan.json`、親 #116、Issue #557）。
//!
//! 扱う範囲は「写真選択（[`crate::photo_picker`]）で選んだ画像 1 枚のバーコード・QR を読み、
//! 中身の文字列の一覧を返す」ことだけ。カメラのライブ読み取り・位置枠・形式の絞り込み・形式名の
//! 返却は扱わない。対象は iOS / macOS（Vision `VNDetectBarcodesRequest`）と Web
//! （`BarcodeDetector`）。Android・Windows は OS 標準の API が無く対象外。
//! 一発呼び出し型の同期 API。呼び出し側（`fandhe-egui-proxy` の effect ドライバ）は
//! UI を止めないようワーカースレッドから呼ぶ。
//!
//! # 契約
//!
//! - **入力**: [`PickedPhoto`]（`validate_photo` 済み）。復号は OS 側に任せる。辺・画素数の
//!   上限検査は呼び出し側が OS を呼ぶ前に済ませる
//! - **出力**: 読み取った中身（[`normalize_payloads`] 済み）。**空の一覧は「コードが無い」と
//!   いう成功**であり、失敗ではない。文字列にできない値は含めない
//! - **失敗の区別**: 非対応の OS・ブラウザ → `Err(Unsupported("barcode_scan"))`、辺・画素数の
//!   上限超過 → `Err(InvalidInput)`、復号不能・OS 呼び出しの失敗 → `Err(Internal)`。
//!   失敗を空の成功へ畳まない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **権限は不要**: 画像は呼び出し側が渡すバイト列で、カメラ・写真ライブラリへ直接触れない。
//!   権限拒否の失敗系は無い
//! - **三値**（契約表 10）: 写真選択のキャンセルは「キャンセル」（失敗表示にしない）、
//!   コードが無いのは成功、それ以外は失敗。キャンセルは画面側（写真選択の段）で決まり、
//!   この trait は返さない
//! - **中身の扱い**: 読んだ中身は信頼できない入力。表示するだけで、URL でも開かず、ログにも
//!   出さない（security.md A03・A09）。trim もしない（契約表 8: 入力どおり）
//!
//! # 守る範囲と守らない範囲
//!
//! 守る範囲: 1 画面・1 プロセス内での要求と完了の順序（画面の世代照合）、嘘の成功の禁止、
//! ワーカーが持つバイト列の解放。守らない範囲: OS ごとの検出結果・順序の違い、処理時間
//! （タイムアウトは掛けない）、実行中の検出の取り消し（結果は世代照合で捨てる）、複数プロセス。

use crate::DeviceError;
use crate::photo_picker::PickedPhoto;

/// バーコード・QR 読み取りの契約（モジュール doc の「契約」節）。
pub trait BarcodeReader: Send + Sync {
    /// `photo` のコードを読み、[`normalize_payloads`] 済みの中身一覧を返す。0 件は成功。
    fn read(&self, photo: &PickedPhoto) -> Result<Vec<String>, DeviceError>;
}

/// 対象外 OS（Linux 等）・未実装 OS のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedBarcodeReader;

impl BarcodeReader for UnsupportedBarcodeReader {
    fn read(&self, _photo: &PickedPhoto) -> Result<Vec<String>, DeviceError> {
        Err(DeviceError::Unsupported("barcode_scan"))
    }
}

/// OS が返した生の中身を契約の形へ揃える: 空文字列だけを除く。前後の空白・順序・重複は保つ
/// （OCR の `normalize_lines` と違い trim しない）。
pub fn normalize_payloads<I, S>(raw: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    raw.into_iter()
        .map(|t| t.as_ref().to_owned())
        .filter(|t| !t.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photo_picker::ImageFormat;

    #[test]
    fn normalize_drops_only_empty_keeps_order_and_duplicates() {
        let out = normalize_payloads(["a", "", "b", "a"]);
        assert_eq!(out, ["a", "b", "a"]);
    }

    #[test]
    fn normalize_does_not_trim() {
        assert_eq!(normalize_payloads([" x ", "  "]), [" x ", "  "]);
    }

    #[test]
    fn unsupported_reader_is_not_a_fake_success() {
        let photo = PickedPhoto {
            bytes: vec![0],
            format: ImageFormat::Png,
        };
        let err = UnsupportedBarcodeReader.read(&photo).unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported("barcode_scan")));
    }
}
