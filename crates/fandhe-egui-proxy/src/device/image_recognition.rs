//! Phase 11（親 #1120）型サンプル「画像の文字認識」（Issue #1250）の effect ドライバ。
//! `fandhe_core::screens::image_recognize`（端末依存ゼロ）の要求を観測し、写真選択
//! （`device/photo_pick.rs` と同じ `PhotoPicker`）で選ばせた画像 1 枚を
//! `fandhe_device::image_recognition::TextRecognizer` で OCR して `CompletionSender` 経由で
//! 結果を返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web は対象外
//! （対応表で `no`）。契約（結果の形・失敗の区別・守る範囲）の SSOT は
//! `fandhe_device::image_recognition`。
//!
//! # OS 別の分担（dispatch は #1250 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: [`super::apple_image_recognition::AppleTextRecognizer`]（#1250）
//! - Windows: `windows_image_recognition`（#1251。WinRT `OcrEngine`）
//! - iOS: `ios_image_recognition::drive`（#1252。PHPicker + Vision（`.m` は macOS と共用）。写真選択が
//!   コールバック型のため同期の `PhotoPicker` を通らず、専用の `drive` を持つ）
//! - Android: `android_image_recognition::drive`（#1253。同上）
//! - それ以外（Linux）: `UnsupportedTextRecognizer` 固定（ダミーの成功実装へは倒さない）
//!
//! # スレッド（デスクトップ）
//!
//! `NSOpenPanel` は main スレッド必須のため、`photo_pick` と同じく呼ばれたスレッド
//! （egui の update ループ = main）で `pick()` を同期に呼ぶ。OCR は数百 ms〜秒単位で UI を
//! 止めるため、選択後はワーカースレッドで実行して完了を送る（`http_fetch` と同型）。
//! 二重起動は、`mark_recognizing` の適用前に effect が再度走っても、`pick()` が main を
//! 塞いで完了が先に積まれるため起きず、画面側の `Idle` 以外 no-op でも防がれる。
//! ワーカーが持つバイト列は完了送信後に drop される。
//!
//! # 入力の検査
//!
//! 写真は `validate_photo`（8 MiB 上限・形式）済み。加えて OCR に渡す前にヘッダーだけで
//! 辺・画素数を `dimensions_within_limits` で検査し、超えたら OS を呼ばず `InvalidInput`
//! （圧縮爆弾対策。切り詰めて成功にしない）。ヘッダーを読めない画像も失敗にする。

#[cfg(not(any(target_os = "ios", target_os = "android")))]
use std::sync::Arc;

#[cfg(any(test, not(any(target_os = "ios", target_os = "android"))))]
use fandhe_core::screens::image_recognize::{finish_recognize, mark_recognizing, take_request};
use fandhe_core::screens::image_recognize::{ImageRecognizeState, RecognizeFailure};
#[cfg(any(test, not(target_os = "android")))]
use fandhe_device::image_recognition::TextRecognizer;
use fandhe_device::image_recognition::LANGUAGE_UNAVAILABLE;
#[cfg(any(test, not(any(target_os = "ios", target_os = "android"))))]
use fandhe_device::photo_picker::PhotoPicker;
#[cfg(any(test, not(target_os = "android")))]
use fandhe_device::photo_picker::{dimensions_within_limits, image_dimensions, PickedPhoto};
use fandhe_device::DeviceError;
#[cfg(any(test, target_os = "ios"))]
use fandhe_device::{file_picker::PickedFile, photo_picker::validate_photo};

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `TextRecognizer` 実装を返す（プロセスで 1 個）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn recognizer() -> Arc<dyn TextRecognizer> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::apple_image_recognition::AppleTextRecognizer)
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(super::windows_image_recognition::WindowsTextRecognizer)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Arc::new(fandhe_device::image_recognition::UnsupportedTextRecognizer)
    }
}

/// `DeviceError` を画面の失敗種別へ写す。内部識別子・パス・認識文字は利用者向け文言へ
/// 出さず、上記以外は固定文言にする（security.md A09）。
#[cfg_attr(target_os = "android", allow(dead_code))]
pub(crate) fn to_failure(e: DeviceError) -> RecognizeFailure {
    match e {
        DeviceError::Unsupported(LANGUAGE_UNAVAILABLE) => RecognizeFailure::LanguageUnavailable,
        DeviceError::Unsupported(_) => RecognizeFailure::Unsupported,
        DeviceError::InvalidInput(_) => RecognizeFailure::InvalidInput,
        _ => RecognizeFailure::Failed("文字を読み取れませんでした".to_string()),
    }
}

/// OS を呼ぶ前にヘッダーだけで辺・画素数を検査する（デスクトップ・iOS 共通。圧縮爆弾対策）。
/// 上限超過は `InvalidInput`、ヘッダーを読めない画像は `Internal`（切り詰めて成功にしない）。
#[cfg(any(test, not(target_os = "android")))]
pub(crate) fn precheck(photo: &PickedPhoto) -> Result<(), DeviceError> {
    match image_dimensions(photo) {
        Some((w, h)) if dimensions_within_limits(w, h) => Ok(()),
        Some(_) => Err(DeviceError::InvalidInput("image_recognition size")),
        None => Err(DeviceError::Internal("image_recognition header")),
    }
}

/// iOS の PHPicker 結果（`ios_image_recognition::deliver` が OS コールバックスレッドで渡す）を
/// 検査して OCR する純粋関数。キャンセルは `Ok(None)`（失敗にしない）、0 行は成功、
/// それ以外の失敗は `to_failure` で固定文言へ写す。ホストのテストで検証できるよう分離。
#[cfg(any(test, target_os = "ios"))]
pub(crate) fn recognize_picked(
    result: Result<Option<PickedFile>, DeviceError>,
    recognizer: &dyn TextRecognizer,
) -> Result<Option<Vec<String>>, RecognizeFailure> {
    let file = match result {
        Ok(Some(file)) => file,
        Ok(None) | Err(DeviceError::Cancelled(_)) => return Ok(None),
        Err(e) => return Err(to_failure(e)),
    };
    let photo = validate_photo(file.bytes).map_err(to_failure)?;
    precheck(&photo).map_err(to_failure)?;
    recognizer.recognize(&photo).map(Some).map_err(to_failure)
}

/// `registry.rs` の `effect` 区画から `image_recognize` 画面用に配線される effect ドライバ。
/// OS ごとの分岐はここで最終形にしてあり、後続の子 issue は触らない。
pub fn image_recognize_effect(
    state: &ImageRecognizeState,
    sender: &CompletionSender<ImageRecognizeState>,
) {
    #[cfg(target_os = "ios")]
    super::ios_image_recognition::drive(state, sender);
    #[cfg(target_os = "android")]
    super::android_image_recognition::drive(state, sender);
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    drive_desktop(
        state,
        sender,
        &*super::photo_pick::implementation(),
        recognizer(),
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
        Err(DeviceError::Cancelled(_)) => return finish(sender, Ok(None)),
        Err(e) => return finish(sender, Err(to_failure(e))),
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
            Err(to_failure(DeviceError::Internal("image_recognition spawn"))),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::image_recognize::{
        request_recognize, CANCELLED_STATUS, FAILED_PREFIX, INVALID_INPUT_STATUS,
        LANGUAGE_UNAVAILABLE_STATUS, NO_TEXT_STATUS, UNSUPPORTED_STATUS,
    };
    use fandhe_device::image_recognition::UnsupportedTextRecognizer;
    use fandhe_device::photo_picker::{validate_photo, PickedPhoto};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<ImageRecognizeState>>;

    fn sender_with_channel() -> (CompletionSender<ImageRecognizeState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 幅 × 高さをヘッダーに持つ PNG（IHDR の先頭だけ。形式判定とヘッダー検査にだけ使う）。
    fn png_with_size(w: u32, h: u32) -> PickedPhoto {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        validate_photo(b).expect("PNG と判定できる")
    }

    struct FakePicker(fn() -> Result<PickedPhoto, DeviceError>);
    impl PhotoPicker for FakePicker {
        fn pick(&self) -> Result<PickedPhoto, DeviceError> {
            (self.0)()
        }
    }

    struct FakeRecognizer {
        result: fn() -> Result<Vec<String>, DeviceError>,
        called: Arc<AtomicBool>,
    }
    impl TextRecognizer for FakeRecognizer {
        fn recognize(&self, _photo: &PickedPhoto) -> Result<Vec<String>, DeviceError> {
            self.called.store(true, Ordering::SeqCst);
            (self.result)()
        }
    }

    /// 要求を立てて `drive_desktop` し、届いた完了（mark と finish の 2 件）を順に適用した
    /// 状態と、recognizer が呼ばれたかを返す。
    fn run(
        picker: fn() -> Result<PickedPhoto, DeviceError>,
        result: fn() -> Result<Vec<String>, DeviceError>,
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

    fn ok_photo() -> Result<PickedPhoto, DeviceError> {
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
            || Err(DeviceError::Cancelled("photo_picker")),
            || Ok(vec![]),
        );
        assert!(!called);
        assert_eq!(s.status, CANCELLED_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn picker_failures_are_mapped_and_skip_recognizer() {
        let (s, called) = run(
            || Err(DeviceError::Unsupported("photo_picker")),
            || Ok(vec![]),
        );
        assert!(!called);
        assert_eq!(s.error, UNSUPPORTED_STATUS);
        let (s, _) = run(
            || Err(DeviceError::InvalidInput("photo_picker size")),
            || Ok(vec![]),
        );
        assert_eq!(s.error, INVALID_INPUT_STATUS);
        let (s, _) = run(|| Err(DeviceError::Internal("photo_picker")), || Ok(vec![]));
        assert!(s.error.starts_with(FAILED_PREFIX));
        assert!(s.status.is_empty());
    }

    #[test]
    fn recognizer_failures_are_mapped() {
        let (s, _) = run(ok_photo, || {
            Err(DeviceError::Unsupported(LANGUAGE_UNAVAILABLE))
        });
        assert_eq!(s.error, LANGUAGE_UNAVAILABLE_STATUS);
        let (s, _) = run(ok_photo, || {
            Err(DeviceError::Unsupported("image_recognition"))
        });
        assert_eq!(s.error, UNSUPPORTED_STATUS);
        let (s, _) = run(ok_photo, || Err(DeviceError::InvalidInput("x")));
        assert_eq!(s.error, INVALID_INPUT_STATUS);
        let (s, _) = run(ok_photo, || Err(DeviceError::Internal("secret-id")));
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
                    .map_err(|_| DeviceError::Internal("test"))
            },
            || Ok(vec!["x".into()]),
        );
        assert!(!called);
        assert!(s.error.starts_with(FAILED_PREFIX));
    }

    struct Rec(fn() -> Result<Vec<String>, DeviceError>, Arc<AtomicBool>);
    impl TextRecognizer for Rec {
        fn recognize(&self, _p: &PickedPhoto) -> Result<Vec<String>, DeviceError> {
            self.1.store(true, Ordering::SeqCst);
            (self.0)()
        }
    }

    fn picked(photo: PickedPhoto) -> Result<Option<PickedFile>, DeviceError> {
        Ok(Some(PickedFile {
            name: "a".into(),
            bytes: photo.bytes,
        }))
    }

    fn pick_run(
        input: Result<Option<PickedFile>, DeviceError>,
        r: fn() -> Result<Vec<String>, DeviceError>,
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
        for input in [Ok(None), Err(DeviceError::Cancelled("x"))] {
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
            Err(DeviceError::Unsupported(LANGUAGE_UNAVAILABLE))
        });
        assert_eq!(o, Err(RecognizeFailure::LanguageUnavailable));
        let (o, _) = pick_run(picked(png_with_size(10, 10)), || {
            Err(DeviceError::Internal("secret-id"))
        });
        match o {
            Err(RecognizeFailure::Failed(m)) => assert!(!m.contains("secret-id")),
            other => panic!("{other:?}"),
        }
        let (o, c) = pick_run(Err(DeviceError::Internal("picker")), || Ok(vec![]));
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
        drive_desktop(&state, &sender, &FakePicker(ok_photo), recognizer());
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        }
        assert_eq!(state.error, UNSUPPORTED_STATUS);
    }
}
