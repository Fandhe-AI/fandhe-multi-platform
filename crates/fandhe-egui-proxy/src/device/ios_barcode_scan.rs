//! iOS 向けバーコード・QR 読み取りの effect ドライバ（Issue #558）。`device/barcode_scan.rs` の
//! iOS 経路（`barcode_scan_effect`）から毎フレーム呼ばれる。写真選択（PHPicker）の結果を受け、
//! `apple_barcode_scan::AppleBarcodeReader`（Vision `VNDetectBarcodesRequest`。`.m` は macOS と共用）
//! で読んで画面状態へ反映する。契約（結果の形・失敗の区別）の SSOT は `fandhe_device::barcode_scan`。
//! 下敷きは文字認識の `ios_image_recognition`。
//!
//! # 経路
//!
//! 1. `drive` が要求を観測し、自前の [`SLOT`] で採番した id に `IOS_PICKER_TAG` を立てて既存の
//!    `ios::present_photo_picker` で PHPicker を出す（`.m` は変えない）。
//! 2. 完了は `ios::fandhe_rust_photo_picker_outcome`（`NSItemProvider` のバックグラウンドキュー）が
//!    受け、目印が立っていれば [`deliver`] へ振り分ける。読み取りはこのコールバックスレッドで
//!    同期に走る（`.m` が `@autoreleasepool` 内で実行し、`dispatch_sync` は使わない）。結果を
//!    slot へ置いて戻った直後に、呼び出し元の `ios.rs` が `request_repaint` で次フレームを起こす。
//! 3. `drive` が毎フレーム `request_slot::poll_and_finish` で結果を拾い、`finish_scan` で反映する。
//!
//! # 目印
//!
//! 文字認識の `1 << 63` と区別するため `1 << 62` を使い、判定は `owns_picker_id` で
//! 排他にする（写真選択の通常の id は小さい連番）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序（世代照合・スロット照合）、嘘の成功の禁止（キャンセル・
//! 成功・失敗の三値。0 件は成功）、バイト列の解放。守らない: 実行中の読み取りの取り消し（結果は
//! 世代照合で捨てる）、処理時間、OS ごとの検出結果の違い、複数プロセス。保留中の要求は復帰時に
//! 復元しない（写真選択と同じ）。読んだ中身はログに出さず、URL でも開かない。

use fandhe_core::screens::barcode_scan::{
    finish_scan, mark_scanning, take_request, BarcodeScanPhase, BarcodeScanState, ScanFailure,
};
use fandhe_device::file_picker::PickedFile;
use fandhe_device::DeviceError;

use super::apple_barcode_scan::AppleBarcodeReader;
use super::barcode_scan::{owns_picker_id, scan_picked, to_failure, IOS_PICKER_TAG};
use super::request_slot::{poll_and_finish, RequestSlot};
use crate::app::CompletionSender;

/// 画面へ反映する完了結果（`None` = キャンセル）。
type Outcome = Result<Option<Vec<String>>, ScanFailure>;

static SLOT: RequestSlot<Outcome> = RequestSlot::new();

/// `request_id` がバーコード読み取りの PHPicker 要求のものか。`ios::fandhe_rust_photo_picker_outcome` が使う。
pub(crate) fn owns(request_id: u64) -> bool {
    owns_picker_id(request_id)
}

/// PHPicker の結果（`NSItemProvider` のキューから呼ばれる）を読んで slot へ置く。
/// id 不一致・未登録・重複は捨てて `false`。
pub(crate) fn deliver(tagged_id: u64, result: Result<Option<PickedFile>, DeviceError>) -> bool {
    let outcome = scan_picked(result, &AppleBarcodeReader);
    SLOT.deliver(tagged_id & !IOS_PICKER_TAG, outcome)
}

/// iOS 版 effect 本体。
pub(crate) fn drive(state: &BarcodeScanState, sender: &CompletionSender<BarcodeScanState>) {
    if take_request(state).is_some() {
        let _ = sender.send(Box::new(mark_scanning));
        let id = SLOT.begin();
        // iOS では常に true（提示失敗も非同期に通知される）。false は契約違反なので失敗で閉じる。
        if !super::ios::present_photo_picker(id | IOS_PICKER_TAG) {
            SLOT.deliver(
                id,
                Err(to_failure(DeviceError::Internal("barcode_scan present"))),
            );
        }
        return;
    }
    if state.phase != BarcodeScanPhase::Scanning {
        return;
    }
    let generation = state.generation;
    poll_and_finish(
        &SLOT,
        sender,
        |_| {},
        move |s: &mut BarcodeScanState, outcome| {
            // Idle へ戻っていれば反映せず結果だけ畳む。別世代は反映せず畳む。
            if s.phase == BarcodeScanPhase::Idle {
                return false;
            }
            if s.generation != generation {
                return true;
            }
            finish_scan(s, generation, outcome);
            true
        },
    );
}
