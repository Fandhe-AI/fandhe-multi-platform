//! iOS 向けバーコード・QR 読み取りの骨格（Issue #557 で置いたスタブ。実装は #558）。
//! `device/barcode_scan.rs` の iOS 経路から毎フレーム呼ばれ、`ios.rs` の
//! `fandhe_rust_photo_picker_outcome` が PHPicker の結果の振り分けに [`owns`]・[`deliver`] を呼ぶ
//! （振り分け行は #557 で最終形）。
//!
//! #558 がこのファイルを置き換える内容: 目印 `1 << 62`（`ios_image_recognition` の `1 << 63` と
//! 区別する）、`RequestSlot` と `poll_and_finish`、PHPicker の起動、`crate::ios::request_repaint`、
//! `apple_barcode_scan::AppleBarcodeReader` での読み取り。あわせて `device.rs` の
//! `apple_barcode_scan` と `barcode_scan::to_failure` の `allow(dead_code)` を外す。
//!
//! 現状は要求を観測したら `Unsupported` の失敗を 1 回返す（画面が処理中のまま止まらず、
//! 嘘の成功にもならない）。

use fandhe_core::screens::barcode_scan::{
    finish_scan, mark_scanning, take_request, BarcodeScanState, ScanFailure,
};
use fandhe_device::file_picker::PickedFile;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// iOS 版 effect 本体。#558 まで要求を `Unsupported` で完了させるだけ。
pub(crate) fn drive(state: &BarcodeScanState, sender: &CompletionSender<BarcodeScanState>) {
    let Some(generation) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_scanning));
    let _ = sender.send(Box::new(move |s: &mut BarcodeScanState| {
        finish_scan(s, generation, Err(ScanFailure::Unsupported));
    }));
}

/// `request_id` がバーコード読み取りの PHPicker 要求のものか。スタブは要求を出さないので常に `false`。
pub(crate) fn owns(_request_id: u64) -> bool {
    false
}

/// PHPicker の結果の受け口。スタブは何も受けず `false`（#558 が読み取りと slot への配送を実装する）。
pub(crate) fn deliver(_tagged_id: u64, _result: Result<Option<PickedFile>, DeviceError>) -> bool {
    false
}
