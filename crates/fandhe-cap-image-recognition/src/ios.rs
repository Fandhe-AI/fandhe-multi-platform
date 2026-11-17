//! iOS 向け文字認識の effect ドライバ（Issue #1252）。`crate::driver` の iOS 経路（`image_recognize_effect`）から毎フレーム呼ばれる。
//! 写真選択（PHPicker）の結果を受け、`apple::AppleTextRecognizer`（Vision。`.m` は macOS と共用）で OCR して画面状態へ反映する。
//! 契約（結果の形・失敗の区別）の SSOT は [`crate::contract`]。
//!
//! # 経路
//!
//! 1. `drive` が要求を観測し、自前の [`SLOT`] で採番した id を持つ受け取り（OCR まで行う閉包）を
//!    `fandhe_cap_photo_picker::present_ios_photo_picker` へ登録して PHPicker を出す。
//! 2. 完了は写真選択の crate の受け口（`NSItemProvider` のバックグラウンドキュー）が、登録された受け取りを id で 1 件取り出して呼ぶ。
//!    受け取りは元のバイト列をここで検査・OCR する（写真選択の `bridge` は表示用に縮小した画素しか持たず OCR に使えない）。OCR は
//!    main ではなくこのコールバックスレッドで同期に走る（`.m` が `@autoreleasepool` 内で実行し、`dispatch_sync` は使わない）。
//!    結果を slot へ置いて戻った直後に、受け口が `request_repaint` で次フレームを起こす。
//! 3. `drive` が毎フレーム `request_slot::poll_and_finish` で結果を拾い、`finish_recognize` で反映する。
//!
//! 以前は要求 id の最上位ビットのタグ（`1 << 63`）で写真選択・バーコード読み取りと受け口を共有していたが、要求ごとに受け取りを登録する
//! 形にしたため、機能ごとの目印・振り分けの順序への依存は無い。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序（世代照合・スロット照合）、嘘の成功の禁止（キャンセル・成功・失敗の三値。0 行は成功）、
//! バイト列の解放。守らない: 実行中の OCR の取り消し（結果は世代照合で捨てる）、処理時間、`.m` が HEIC 等を長辺 4096 の JPEG へ
//! 縮めることによる精度差、複数プロセス。保留中の要求は復帰時に復元しない（写真選択と同じ）。

use fandhe_core::screens::image_recognize::{
    ImageRecognizePhase, ImageRecognizeState, RecognizeFailure, finish_recognize, mark_recognizing,
    take_request,
};
use fandhe_device::file_picker::PickedFile;
use fandhe_runtime::CompletionSender;
use fandhe_runtime::request_slot::{RequestSlot, poll_and_finish};

use fandhe_cap_photo_picker::{PhotoPickerError, present_ios_photo_picker};

use crate::apple::AppleTextRecognizer;
use crate::driver::{recognize_picked, to_failure};
use crate::error::ImageRecognitionError;

/// 画面へ反映する完了結果（`None` = キャンセル）。
type Outcome = Result<Option<Vec<String>>, RecognizeFailure>;

static SLOT: RequestSlot<Outcome> = RequestSlot::new();

/// PHPicker の結果（`NSItemProvider` のキューから呼ばれる）を OCR して slot へ置く。
/// id 不一致・未登録・重複は捨てて `false`。
fn deliver(id: u64, result: Result<Option<PickedFile>, PhotoPickerError>) -> bool {
    let outcome = recognize_picked(result, &AppleTextRecognizer);
    SLOT.deliver(id, outcome)
}

/// iOS 版 effect 本体。
pub(crate) fn drive(state: &ImageRecognizeState, sender: &CompletionSender<ImageRecognizeState>) {
    if take_request(state).is_some() {
        let _ = sender.send(Box::new(mark_recognizing));
        let id = SLOT.begin();
        // iOS では通常 true（提示失敗も非同期に通知される）。false は提示依頼自体の同期的な失敗なので失敗で閉じる。
        let presented = present_ios_photo_picker(Box::new(move |result| {
            deliver(id, result);
        }));
        if !presented {
            SLOT.deliver(
                id,
                Err(to_failure(ImageRecognitionError::Internal(
                    "image_recognition present",
                ))),
            );
        }
        return;
    }
    if state.phase != ImageRecognizePhase::Recognizing {
        return;
    }
    let generation = state.generation;
    poll_and_finish(
        &SLOT,
        sender,
        |_| {},
        move |s: &mut ImageRecognizeState, outcome| {
            // Idle へ戻っていれば反映せず結果だけ畳む。別世代は反映せず畳む。
            if s.phase == ImageRecognizePhase::Idle {
                return false;
            }
            if s.generation != generation {
                return true;
            }
            finish_recognize(s, generation, outcome);
            true
        },
    );
}
