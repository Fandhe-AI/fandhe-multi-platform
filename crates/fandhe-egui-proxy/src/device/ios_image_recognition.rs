//! iOS 向け文字認識の effect ドライバ（Issue #1252）。`device/image_recognition.rs` の iOS 経路
//! （`image_recognize_effect`）から毎フレーム呼ばれる。写真選択（PHPicker）の結果を受け、
//! `apple_image_recognition::AppleTextRecognizer`（Vision。`.m` は macOS と共用）で OCR して
//! 画面状態へ反映する。契約（結果の形・失敗の区別）の SSOT は `fandhe_device::image_recognition`。
//!
//! # 経路
//!
//! 1. `drive` が要求を観測し、自前の [`SLOT`] で採番した id に [`PICKER_TAG`] を立てて既存の
//!    `ios::present_photo_picker` で PHPicker を出す（`.m` は変えない。id は u64 のまま往復する）。
//! 2. 完了は `ios::fandhe_rust_photo_picker_outcome`（`NSItemProvider` のバックグラウンドキュー）が
//!    受け、タグ付きなら [`deliver`] へ振り分ける（タグ無しは従来の `photo_pick::bridge`）。
//!    `photo_pick::bridge` は表示用に縮小した画素しか持たず OCR に使えないため、元のバイト列を
//!    ここで検査・OCR する。OCR は main ではなくこのコールバックスレッドで同期に走る
//!    （`.m` が `@autoreleasepool` 内で実行し、`dispatch_sync` は使わない）。
//! 3. `drive` が毎フレーム `request_slot::poll_and_finish` で結果を拾い、`finish_recognize` で反映する。
//!
//! # タグ
//!
//! `RequestSlot` の採番はスロットごとに 1 から始まり、写真選択と id が衝突しうる。
//! 最上位ビットを OCR 要求の目印にして同じ受け口を共有する（採番が 2^63 に届くことはない）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序（世代照合・スロット照合）、嘘の成功の禁止（キャンセル・
//! 成功・失敗の三値。0 行は成功）、バイト列の解放。守らない: 実行中の OCR の取り消し（結果は
//! 世代照合で捨てる）、処理時間、`.m` が HEIC 等を長辺 4096 の JPEG へ縮めることによる精度差、
//! 複数プロセス。保留中の要求は復帰時に復元しない（写真選択と同じ）。

use fandhe_core::screens::image_recognize::{
    finish_recognize, mark_recognizing, take_request, ImageRecognizePhase, ImageRecognizeState,
    RecognizeFailure,
};
use fandhe_device::file_picker::PickedFile;
use fandhe_device::DeviceError;

use super::apple_image_recognition::AppleTextRecognizer;
use super::image_recognition::{recognize_picked, to_failure};
use super::request_slot::{poll_and_finish, RequestSlot};
use crate::app::CompletionSender;

/// 画面へ反映する完了結果（`None` = キャンセル）。
type Outcome = Result<Option<Vec<String>>, RecognizeFailure>;

static SLOT: RequestSlot<Outcome> = RequestSlot::new();

/// OCR 要求の PHPicker 呼び出しに立てる目印（写真選択の id と区別する）。
const PICKER_TAG: u64 = 1 << 63;

/// `request_id` が OCR 要求のものか。`ios::fandhe_rust_photo_picker_outcome` が振り分けに使う。
pub(crate) fn owns(request_id: u64) -> bool {
    request_id & PICKER_TAG != 0
}

/// PHPicker の結果（`NSItemProvider` のキューから呼ばれる）を OCR して slot へ置く。
/// id 不一致・未登録・重複は捨てて `false`。
pub(crate) fn deliver(tagged_id: u64, result: Result<Option<PickedFile>, DeviceError>) -> bool {
    let outcome = recognize_picked(result, &AppleTextRecognizer);
    SLOT.deliver(tagged_id & !PICKER_TAG, outcome)
}

/// iOS 版 effect 本体。
pub(crate) fn drive(state: &ImageRecognizeState, sender: &CompletionSender<ImageRecognizeState>) {
    if take_request(state).is_some() {
        let _ = sender.send(Box::new(mark_recognizing));
        let id = SLOT.begin();
        // iOS では常に true（提示失敗も非同期に通知される）。false は契約違反なので失敗で閉じる。
        if !super::ios::present_photo_picker(id | PICKER_TAG) {
            SLOT.deliver(
                id,
                Err(to_failure(DeviceError::Internal(
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
