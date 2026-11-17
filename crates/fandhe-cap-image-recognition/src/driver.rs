//! Phase 11（親 #1120）型サンプル「画像の文字認識」（Issue #1250）の effect ドライバ（機能 crate `fandhe-cap-image-recognition` の中核、
//! Issue #1355）。`fandhe_core::screens::image_recognize`（端末依存ゼロ）の要求を観測し、写真選択（機能 crate `fandhe-cap-photo-picker`）で
//! 選ばせた画像 1 枚を [`TextRecognizer`] で OCR して `CompletionSender` 経由で結果を返す（`fandhe-gallery` の `screens/registry.rs` の
//! `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`fandhe_runtime::EffectFn` 参照）。
//!
//! Web は対象外（対応表で `no`）。契約（結果の形・失敗の区別・守る範囲）の SSOT は [`crate::contract`]。
//!
//! # OS 別の分担
//!
//! - macOS: `apple::AppleTextRecognizer`（#1250）
//! - Windows: `windows`（#1251。WinRT `OcrEngine`）
//! - iOS: `ios::drive`（#1252。PHPicker + Vision（`.m` は macOS と共用））。写真選択がコールバック型のため同期の `PhotoPicker` を通らず、
//!   `fandhe_cap_photo_picker::present_ios_photo_picker` へ要求ごとの受け取りを登録する
//! - Android: `android::drive`（#1253 まで要求を `Unsupported` で完了させる骨格。写真選択の crate には依存しない）
//! - それ以外（Linux）: `UnsupportedTextRecognizer` 固定（ダミーの成功実装へは倒さない）
//!
//! # スレッド（デスクトップ）
//!
//! `NSOpenPanel` は main スレッド必須のため、`photo_pick` と同じく呼ばれたスレッド（egui の update ループ = main）で `pick()` を同期に呼ぶ。
//! OCR は数百 ms〜秒単位で UI を止めるため、選択後はワーカースレッドで実行して完了を送る（`http_fetch` と同型）。
//! 二重起動は、`mark_recognizing` の適用前に effect が再度走っても、`pick()` が main を塞いで完了が先に積まれるため起きず、
//! 画面側の `Idle` 以外 no-op でも防がれる。ワーカーが持つバイト列は完了送信後に drop される。
//!
//! # 入力の検査
//!
//! 写真は `validate_photo`（8 MiB 上限・形式）済み。加えて OCR に渡す前にヘッダーだけで辺・画素数を
//! `fandhe_device::photo_picker::precheck_dimensions`（バーコード読み取りと共有）で検査し、超えたら OS を呼ばず `InvalidInput`
//! （圧縮爆弾対策。切り詰めて成功にしない）。ヘッダーを読めない画像も失敗にする。

#[cfg(test)]
use std::sync::Arc;

#[cfg(any(test, not(any(target_os = "ios", target_os = "android"))))]
use fandhe_cap_photo_picker::PhotoPicker;
#[cfg(any(test, target_os = "ios"))]
use fandhe_cap_photo_picker::PhotoPickerError;
use fandhe_core::screens::image_recognize::{ImageRecognizeState, RecognizeFailure};
#[cfg(any(test, not(any(target_os = "ios", target_os = "android"))))]
use fandhe_core::screens::image_recognize::{finish_recognize, mark_recognizing, take_request};
#[cfg(any(test, not(target_os = "android")))]
use fandhe_device::photo_picker::{PickedPhoto, precheck_dimensions};
#[cfg(any(test, target_os = "ios"))]
use fandhe_device::{file_picker::PickedFile, photo_picker::validate_photo};
use fandhe_runtime::CompletionSender;

use crate::contract::LANGUAGE_UNAVAILABLE;
#[cfg(any(test, not(target_os = "android")))]
use crate::contract::TextRecognizer;
use crate::error::ImageRecognitionError;

/// 現在のプラットフォームに応じた [`TextRecognizer`] 実装を返す（プロセスで 1 個）。状態型に依存しない入口で、
/// フレームワークの外のアプリも自分の状態型の effect から使える。macOS は Vision、Windows は `OcrEngine`、
/// iOS・Android・Linux は `Unsupported` 固定（iOS は PHPicker が非同期に結果を返すため、同期の入口では使えない。
/// 画面は [`image_recognize_effect`] の経路を使う。Android は #1253 まで骨格）。
// ponytail: iOS の同期入口は要求が出たら足す（`ios::drive` の登録・受け取りを同期に直すにはスレッド待ちが要る）。
pub fn platform_image_recognition() -> std::sync::Arc<dyn crate::contract::TextRecognizer> {
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(crate::apple::AppleTextRecognizer)
    }
    #[cfg(target_os = "windows")]
    {
        std::sync::Arc::new(crate::windows::WindowsTextRecognizer)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::sync::Arc::new(crate::contract::UnsupportedTextRecognizer)
    }
}

/// `ImageRecognitionError` を画面の失敗種別へ写す。内部識別子・パス・認識文字は利用者向け文言へ
/// 出さず、上記以外は固定文言にする（security.md A09）。
#[cfg_attr(target_os = "android", allow(dead_code))]
pub(crate) fn to_failure(e: ImageRecognitionError) -> RecognizeFailure {
    match e {
        ImageRecognitionError::Unsupported(LANGUAGE_UNAVAILABLE) => {
            RecognizeFailure::LanguageUnavailable
        }
        ImageRecognitionError::Unsupported(_) => RecognizeFailure::Unsupported,
        ImageRecognitionError::InvalidInput(_) => RecognizeFailure::InvalidInput,
        _ => RecognizeFailure::Failed("文字を読み取れませんでした".to_string()),
    }
}

/// OS を呼ぶ前にヘッダーだけで辺・画素数を検査する（デスクトップ・iOS 共通。圧縮爆弾対策）。
/// 上限超過は `InvalidInput`、ヘッダーを読めない画像は `Internal`（切り詰めて成功にしない）。検査の本体は
/// `fandhe_device::photo_picker::precheck_dimensions`（バーコード読み取りと共有）。
#[cfg(any(test, not(target_os = "android")))]
pub(crate) fn precheck(photo: &PickedPhoto) -> Result<(), ImageRecognitionError> {
    precheck_dimensions(photo).map_err(ImageRecognitionError::from)
}

/// iOS の PHPicker 結果（`ios::drive` が登録した受け取りが OS コールバックスレッドで渡す）を
/// 検査して OCR する純粋関数。キャンセルは `Ok(None)`（失敗にしない）、0 行は成功、
/// それ以外の失敗は `to_failure` で固定文言へ写す。ホストのテストで検証できるよう分離。
#[cfg(any(test, target_os = "ios"))]
pub(crate) fn recognize_picked(
    result: Result<Option<PickedFile>, PhotoPickerError>,
    recognizer: &dyn TextRecognizer,
) -> Result<Option<Vec<String>>, RecognizeFailure> {
    let file = match result {
        Ok(Some(file)) => file,
        Ok(None) | Err(PhotoPickerError::Cancelled(_)) => return Ok(None),
        Err(e) => return Err(to_failure(e.into())),
    };
    let photo = validate_photo(file.bytes)
        .map_err(ImageRecognitionError::from)
        .map_err(to_failure)?;
    precheck(&photo).map_err(to_failure)?;
    recognizer.recognize(&photo).map(Some).map_err(to_failure)
}

/// `registry.rs` の `effect` 区画から `image_recognize` 画面用に配線される effect ドライバ。
pub fn image_recognize_effect(
    state: &ImageRecognizeState,
    sender: &CompletionSender<ImageRecognizeState>,
) {
    #[cfg(target_os = "ios")]
    ios_effect::drive(state, sender);
    #[cfg(target_os = "android")]
    android_effect::drive(state, sender);
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    drive_desktop(
        state,
        sender,
        &*fandhe_cap_photo_picker::platform_photo_picker(),
        platform_image_recognition(),
    );
}

/// デスクトップ版の要求 1 件の処理（テストから任意の実装を差し込めるよう切り出す）。
#[cfg(any(test, not(any(target_os = "ios", target_os = "android"))))]
fn drive_desktop(
    state: &ImageRecognizeState,
    sender: &CompletionSender<ImageRecognizeState>,
    picker: &dyn PhotoPicker,
    recognizer: std::sync::Arc<dyn TextRecognizer>,
) {
    let Some(generation) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_recognizing));

    let finish = |sender: &CompletionSender<ImageRecognizeState>,
                  result: Result<Option<Vec<String>>, RecognizeFailure>| {
        // 受信側のドロップ（アプリ終了中）による送信失敗は無視してよい。
        let _ = sender.send(Box::new(move |s: &mut ImageRecognizeState| {
            finish_recognize(s, generation, result);
        }));
    };

    let photo = match picker.pick() {
        Ok(photo) => photo,
        // キャンセルは失敗ではない（契約表 10）。
        Err(fandhe_cap_photo_picker::PhotoPickerError::Cancelled(_)) => {
            return finish(sender, Ok(None));
        }
        Err(e) => return finish(sender, Err(to_failure(e.into()))),
    };
    if let Err(e) = precheck(&photo) {
        return finish(sender, Err(to_failure(e)));
    }

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("image-recognition".into())
        .spawn(move || {
            let result = recognizer.recognize(&photo).map(Some).map_err(to_failure);
            drop(photo);
            let _ = worker_sender.send(Box::new(move |s: &mut ImageRecognizeState| {
                finish_recognize(s, generation, result);
            }));
        });
    if spawned.is_err() {
        finish(
            sender,
            Err(to_failure(ImageRecognitionError::Internal(
                "image_recognition spawn",
            ))),
        );
    }
}

#[cfg(target_os = "ios")]
mod ios_effect {
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
        ImageRecognizePhase, ImageRecognizeState, RecognizeFailure, finish_recognize,
        mark_recognizing, take_request,
    };
    use fandhe_device::file_picker::PickedFile;
    use fandhe_runtime::CompletionSender;
    use fandhe_runtime::request_slot::{RequestSlot, poll_and_finish};

    use fandhe_cap_photo_picker::{PhotoPickerError, present_ios_photo_picker};

    use super::{recognize_picked, to_failure};
    use crate::apple::AppleTextRecognizer;
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
    pub(crate) fn drive(
        state: &ImageRecognizeState,
        sender: &CompletionSender<ImageRecognizeState>,
    ) {
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
}

#[cfg(target_os = "android")]
mod android_effect {
    //! Android 向け文字認識の骨格（Issue #1250 で置いたスタブ。実装は #1253）。
    //! `crate::driver` の Android 経路から毎フレーム呼ばれる。#1253 が ML Kit の
    //! Java ヘルパークラス・JNI・`RequestSlot` を使った実装に本ファイルの `drive` を置き換える
    //! （dispatch・画面・文言は #1250 で最終形）。
    //!
    //! 現状は要求を観測したら `Unsupported` の失敗を 1 回返す（画面が処理中のまま止まらず、
    //! 嘘の成功にもならない）。

    use fandhe_core::screens::image_recognize::{
        ImageRecognizeState, RecognizeFailure, finish_recognize, mark_recognizing, take_request,
    };

    use fandhe_runtime::CompletionSender;

    /// Android 版 effect 本体。#1253 まで要求を `Unsupported` で完了させるだけ。
    pub(crate) fn drive(
        state: &ImageRecognizeState,
        sender: &CompletionSender<ImageRecognizeState>,
    ) {
        let Some(generation) = take_request(state) else {
            return;
        };
        let _ = sender.send(Box::new(mark_recognizing));
        let _ = sender.send(Box::new(move |s: &mut ImageRecognizeState| {
            finish_recognize(s, generation, Err(RecognizeFailure::Unsupported));
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::UnsupportedTextRecognizer;
    use fandhe_core::screens::image_recognize::{
        CANCELLED_STATUS, FAILED_PREFIX, INVALID_INPUT_STATUS, LANGUAGE_UNAVAILABLE_STATUS,
        NO_TEXT_STATUS, UNSUPPORTED_STATUS, request_recognize,
    };
    use fandhe_device::photo_picker::{PickedPhoto, validate_photo};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    type Rx = std::sync::mpsc::Receiver<fandhe_runtime::Completion<ImageRecognizeState>>;

    fn sender_with_channel() -> (CompletionSender<ImageRecognizeState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
    }

    /// 幅 × 高さをヘッダーに持つ PNG（IHDR の先頭だけ。形式判定とヘッダー検査にだけ使う）。
    fn png_with_size(w: u32, h: u32) -> PickedPhoto {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        validate_photo(b).expect("PNG と判定できる")
    }

    struct FakePicker(fn() -> Result<PickedPhoto, PhotoPickerError>);
    impl PhotoPicker for FakePicker {
        fn pick(&self) -> Result<PickedPhoto, PhotoPickerError> {
            (self.0)()
        }
    }

    struct FakeRecognizer {
        result: fn() -> Result<Vec<String>, ImageRecognitionError>,
        called: Arc<AtomicBool>,
    }
    impl TextRecognizer for FakeRecognizer {
        fn recognize(&self, _photo: &PickedPhoto) -> Result<Vec<String>, ImageRecognitionError> {
            self.called.store(true, Ordering::SeqCst);
            (self.result)()
        }
    }

    /// 要求を立てて `drive_desktop` し、届いた完了（mark と finish の 2 件）を順に適用した
    /// 状態と、recognizer が呼ばれたかを返す。
    fn run(
        picker: fn() -> Result<PickedPhoto, PhotoPickerError>,
        result: fn() -> Result<Vec<String>, ImageRecognitionError>,
    ) -> (ImageRecognizeState, bool) {
        let (sender, rx) = sender_with_channel();
        let mut state = ImageRecognizeState::default();
        request_recognize(&mut state);
        let called = Arc::new(AtomicBool::new(false));
        let rec = Arc::new(FakeRecognizer {
            result,
            called: called.clone(),
        });
        drive_desktop(&state, &sender, &FakePicker(picker), rec);
        for _ in 0..2 {
            let c = rx.recv_timeout(Duration::from_secs(5)).expect("完了が届く");
            c(&mut state);
        }
        (state, called.load(Ordering::SeqCst))
    }

    fn ok_photo() -> Result<PickedPhoto, PhotoPickerError> {
        Ok(png_with_size(10, 10))
    }

    #[test]
    fn noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        drive_desktop(
            &ImageRecognizeState::default(),
            &sender,
            &FakePicker(ok_photo),
            Arc::new(UnsupportedTextRecognizer),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_shows_lines() {
        let (s, called) = run(ok_photo, || Ok(vec!["a".into(), "b".into()]));
        assert!(called);
        assert_eq!(s.status, "2 行");
        assert_eq!(s.lines, ["a", "b"]);
        assert!(s.error.is_empty());
    }

    #[test]
    fn zero_lines_is_no_text_success() {
        let (s, _) = run(ok_photo, || Ok(vec![]));
        assert_eq!(s.status, NO_TEXT_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn picker_cancel_is_cancel_and_skips_recognizer() {
        let (s, called) = run(
            || Err(PhotoPickerError::Cancelled("photo_picker")),
            || Ok(vec![]),
        );
        assert!(!called);
        assert_eq!(s.status, CANCELLED_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn picker_failures_are_mapped_and_skip_recognizer() {
        let (s, called) = run(
            || Err(PhotoPickerError::Unsupported("photo_picker")),
            || Ok(vec![]),
        );
        assert!(!called);
        assert_eq!(s.error, UNSUPPORTED_STATUS);
        let (s, _) = run(
            || Err(PhotoPickerError::InvalidInput("photo_picker size")),
            || Ok(vec![]),
        );
        assert_eq!(s.error, INVALID_INPUT_STATUS);
        let (s, _) = run(
            || Err(PhotoPickerError::Internal("photo_picker")),
            || Ok(vec![]),
        );
        assert!(s.error.starts_with(FAILED_PREFIX));
        assert!(s.status.is_empty());
    }

    #[test]
    fn recognizer_failures_are_mapped() {
        let (s, _) = run(ok_photo, || {
            Err(ImageRecognitionError::Unsupported(LANGUAGE_UNAVAILABLE))
        });
        assert_eq!(s.error, LANGUAGE_UNAVAILABLE_STATUS);
        let (s, _) = run(ok_photo, || {
            Err(ImageRecognitionError::Unsupported("image_recognition"))
        });
        assert_eq!(s.error, UNSUPPORTED_STATUS);
        let (s, _) = run(ok_photo, || Err(ImageRecognitionError::InvalidInput("x")));
        assert_eq!(s.error, INVALID_INPUT_STATUS);
        let (s, _) = run(ok_photo, || {
            Err(ImageRecognitionError::Internal("secret-id"))
        });
        assert!(s.error.starts_with(FAILED_PREFIX));
        assert!(!s.error.contains("secret-id"), "内部識別子を出さない");
        assert!(s.lines.is_empty() && s.status.is_empty());
    }

    #[test]
    fn oversize_header_is_invalid_input_without_calling_recognizer() {
        let (s, called) = run(
            || {
                Ok(png_with_size(
                    1,
                    fandhe_device::photo_picker::MAX_PHOTO_SIDE + 1,
                ))
            },
            || Ok(vec!["x".into()]),
        );
        assert!(!called);
        assert_eq!(s.error, INVALID_INPUT_STATUS);
    }

    #[test]
    fn unreadable_header_is_failure_not_success() {
        // IHDR を欠く PNG（先頭マジックだけ）はヘッダーを読めない。
        let (s, called) = run(
            || {
                validate_photo(vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
                    .map_err(|_| PhotoPickerError::Internal("test"))
            },
            || Ok(vec!["x".into()]),
        );
        assert!(!called);
        assert!(s.error.starts_with(FAILED_PREFIX));
    }

    struct Rec(
        fn() -> Result<Vec<String>, ImageRecognitionError>,
        Arc<AtomicBool>,
    );
    impl TextRecognizer for Rec {
        fn recognize(&self, _p: &PickedPhoto) -> Result<Vec<String>, ImageRecognitionError> {
            self.1.store(true, Ordering::SeqCst);
            (self.0)()
        }
    }

    fn picked(photo: PickedPhoto) -> Result<Option<PickedFile>, PhotoPickerError> {
        Ok(Some(PickedFile {
            name: "a".into(),
            bytes: photo.bytes,
        }))
    }

    fn pick_run(
        input: Result<Option<PickedFile>, PhotoPickerError>,
        r: fn() -> Result<Vec<String>, ImageRecognitionError>,
    ) -> (Result<Option<Vec<String>>, RecognizeFailure>, bool) {
        let called = Arc::new(AtomicBool::new(false));
        let out = recognize_picked(input, &Rec(r, called.clone()));
        (out, called.load(Ordering::SeqCst))
    }

    #[test]
    fn picked_success_and_zero_lines() {
        let (o, c) = pick_run(picked(png_with_size(10, 10)), || Ok(vec!["a".into()]));
        assert!(c);
        assert_eq!(o, Ok(Some(vec!["a".to_string()])));
        let (o, _) = pick_run(picked(png_with_size(10, 10)), || Ok(vec![]));
        assert_eq!(o, Ok(Some(vec![])));
    }

    #[test]
    fn picked_cancel_skips_recognizer() {
        for input in [Ok(None), Err(PhotoPickerError::Cancelled("x"))] {
            let (o, c) = pick_run(input, || Ok(vec![]));
            assert_eq!(o, Ok(None));
            assert!(!c);
        }
    }

    #[test]
    fn picked_invalid_inputs_skip_recognizer() {
        let empty = Ok(Some(PickedFile {
            name: "a".into(),
            bytes: vec![],
        }));
        let (o, c) = pick_run(empty, || Ok(vec![]));
        assert!(matches!(o, Err(RecognizeFailure::Failed(_))) && !c);
        let big = picked(png_with_size(
            1,
            fandhe_device::photo_picker::MAX_PHOTO_SIDE + 1,
        ));
        let (o, c) = pick_run(big, || Ok(vec![]));
        assert_eq!(o, Err(RecognizeFailure::InvalidInput));
        assert!(!c);
        let no_ihdr = Ok(Some(PickedFile {
            name: "a".into(),
            bytes: vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        }));
        let (o, c) = pick_run(no_ihdr, || Ok(vec![]));
        assert!(matches!(o, Err(RecognizeFailure::Failed(_))) && !c);
    }

    #[test]
    fn picked_errors_are_mapped_without_internal_ids() {
        let (o, _) = pick_run(picked(png_with_size(10, 10)), || {
            Err(ImageRecognitionError::Unsupported(LANGUAGE_UNAVAILABLE))
        });
        assert_eq!(o, Err(RecognizeFailure::LanguageUnavailable));
        let (o, _) = pick_run(picked(png_with_size(10, 10)), || {
            Err(ImageRecognitionError::Internal("secret-id"))
        });
        match o {
            Err(RecognizeFailure::Failed(m)) => assert!(!m.contains("secret-id")),
            other => panic!("{other:?}"),
        }
        let (o, c) = pick_run(Err(PhotoPickerError::Internal("picker")), || Ok(vec![]));
        assert!(matches!(o, Err(RecognizeFailure::Failed(_))) && !c);
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    )))]
    #[test]
    fn fallback_recognizer_is_unsupported() {
        let (sender, rx) = sender_with_channel();
        let mut state = ImageRecognizeState::default();
        request_recognize(&mut state);
        drive_desktop(
            &state,
            &sender,
            &FakePicker(ok_photo),
            platform_image_recognition(),
        );
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        }
        assert_eq!(state.error, UNSUPPORTED_STATUS);
    }
}
