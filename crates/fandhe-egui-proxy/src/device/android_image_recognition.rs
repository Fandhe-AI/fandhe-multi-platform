//! Android 向け文字認識の骨格（Issue #1250 で置いたスタブ。実装は #1253）。
//! `device/image_recognition.rs` の Android 経路から毎フレーム呼ばれる。#1253 が ML Kit の
//! Java ヘルパークラス・JNI・`RequestSlot` を使った実装に本ファイルの `drive` を置き換える
//! （dispatch・画面・文言は #1250 で最終形）。
//!
//! 現状は要求を観測したら `Unsupported` の失敗を 1 回返す（画面が処理中のまま止まらず、
//! 嘘の成功にもならない）。

use fandhe_core::screens::image_recognize::{
    finish_recognize, mark_recognizing, take_request, ImageRecognizeState, RecognizeFailure,
};

use crate::app::CompletionSender;

/// Android 版 effect 本体。#1253 まで要求を `Unsupported` で完了させるだけ。
pub(crate) fn drive(state: &ImageRecognizeState, sender: &CompletionSender<ImageRecognizeState>) {
    let Some(generation) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_recognizing));
    let _ = sender.send(Box::new(move |s: &mut ImageRecognizeState| {
        finish_recognize(s, generation, Err(RecognizeFailure::Unsupported));
    }));
}
