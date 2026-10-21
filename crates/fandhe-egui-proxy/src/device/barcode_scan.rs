//! Phase 11（親 #116）型サンプル「バーコード・QR 読み取り」（Issue #557）の effect ドライバ。
//! `fandhe_core::screens::barcode_scan`（端末依存ゼロ）の要求を観測し、写真選択
//! （`device/photo_pick.rs` と同じ `PhotoPicker`）で選ばせた画像 1 枚を
//! `fandhe_device::barcode_scan::BarcodeReader` で読んで `CompletionSender` 経由で
//! 結果を返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web は
//! `fandhe-web-dom` 側（#559）で扱う。契約（結果の形・失敗の区別・守る範囲）の SSOT は
//! `fandhe_device::barcode_scan`。
//!
//! # OS 別の分担（dispatch は #557 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: [`super::apple_barcode_scan::AppleBarcodeReader`]（#557）
//! - iOS: `ios_barcode_scan::drive`（#558。PHPicker + Vision。現状は `Unsupported` のスタブ）
//! - それ以外（Windows・Android・Linux）: 対応表で `no` / 対象外。写真選択を開かず `Unsupported`
//!   で完了させる（ダミーの成功実装へは倒さない）
//!
//! # スレッド（デスクトップ）
//!
//! `NSOpenPanel` は main スレッド必須のため、`photo_pick` と同じく呼ばれたスレッド
//! （egui の update ループ = main）で `pick()` を同期に呼ぶ。読み取りは UI を止めうるため、
//! 選択後はワーカースレッドで実行して完了を送る。二重起動は、`mark_scanning` の適用前に effect が
//! 再度走っても `pick()` が main を塞いで完了が先に積まれるため起きず、画面側の `Idle` 以外
//! no-op でも防がれる。ワーカーが持つバイト列は完了送信後に drop される。
//!
//! # 入力の検査
//!
//! 写真は `validate_photo`（8 MiB 上限・形式）済み。加えて OS に渡す前にヘッダーだけで
//! 辺・画素数を検査し（`image_recognition::precheck` を共用）、超えたら OS を呼ばず `InvalidInput`
//! （圧縮爆弾対策。切り詰めて成功にしない）。ヘッダーを読めない画像も失敗にする。

#[cfg(any(test, target_os = "macos"))]
use std::sync::Arc;

#[cfg(any(test, not(target_os = "ios")))]
use fandhe_core::screens::barcode_scan::{finish_scan, mark_scanning, take_request};
use fandhe_core::screens::barcode_scan::{BarcodeScanState, ScanFailure};
#[cfg(any(test, target_os = "macos", target_os = "ios"))]
use fandhe_device::barcode_scan::BarcodeReader;
#[cfg(any(test, target_os = "ios"))]
use fandhe_device::file_picker::PickedFile;
#[cfg(any(test, target_os = "ios"))]
use fandhe_device::photo_picker::validate_photo;
#[cfg(any(test, target_os = "macos"))]
use fandhe_device::photo_picker::PhotoPicker;
#[cfg(any(test, target_os = "macos", target_os = "ios"))]
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `DeviceError` を画面の失敗種別へ写す。内部識別子・パス・読み取った中身は利用者向け文言へ
/// 出さず、上記以外は固定文言にする（security.md A09）。
#[cfg(any(test, target_os = "macos", target_os = "ios"))]
#[cfg_attr(target_os = "ios", allow(dead_code))] // #558 が使う
pub(crate) fn to_failure(e: DeviceError) -> ScanFailure {
    match e {
        DeviceError::Unsupported(_) => ScanFailure::Unsupported,
        DeviceError::InvalidInput(_) => ScanFailure::InvalidInput,
        _ => ScanFailure::Failed("コードを読み取れませんでした".to_string()),
    }
}

/// iOS で barcode 要求の PHPicker 呼び出しに立てる目印（`1 << 62`）。写真選択の id・
/// 文字認識の目印（`1 << 63`）と区別し、`ios.rs` の同じ受け口を共有する。
#[cfg(any(test, target_os = "ios"))]
pub(crate) const IOS_PICKER_TAG: u64 = 1 << 62;

/// 文字認識の目印。`ios_image_recognition::PICKER_TAG` と同じ値（そちらは iOS 専用で
/// ホストのテストから参照できないため写す）。
#[cfg(any(test, target_os = "ios"))]
const OCR_PICKER_TAG: u64 = 1 << 63;

/// `id` が barcode 要求の PHPicker 呼び出しか。振り分け順に頼らず、OCR の目印が立った id は除く。
/// `RequestSlot` の採番は 1 から始まり 2^62 には届かない。
#[cfg(any(test, target_os = "ios"))]
pub(crate) fn owns_picker_id(id: u64) -> bool {
    id & OCR_PICKER_TAG == 0 && id & IOS_PICKER_TAG != 0
}

/// PHPicker の結果を検査して読む純粋関数（iOS 経路）。キャンセルは `Ok(None)`（失敗にしない）、
/// 0 件は成功、それ以外の失敗は `to_failure` で固定文言へ写す。バイト列は戻り時に解放される。
/// ホストのテストで検証できるよう `ios_barcode_scan` から分離。
#[cfg(any(test, target_os = "ios"))]
pub(crate) fn scan_picked(
    result: Result<Option<PickedFile>, DeviceError>,
    reader: &dyn BarcodeReader,
) -> Result<Option<Vec<String>>, ScanFailure> {
    let file = match result {
        Ok(Some(file)) => file,
        Ok(None) | Err(DeviceError::Cancelled(_)) => return Ok(None),
        Err(e) => return Err(to_failure(e)),
    };
    let photo = validate_photo(file.bytes).map_err(to_failure)?;
    super::image_recognition::precheck(&photo).map_err(to_failure)?;
    reader.read(&photo).map(Some).map_err(to_failure)
}

/// `registry.rs` の `effect` 区画から `barcode_scan` 画面用に配線される effect ドライバ。
/// OS ごとの分岐はここで最終形にしてあり、後続の子 issue は触らない。
pub fn barcode_scan_effect(state: &BarcodeScanState, sender: &CompletionSender<BarcodeScanState>) {
    #[cfg(target_os = "macos")]
    drive_desktop(
        state,
        sender,
        &*super::photo_pick::implementation(),
        Arc::new(super::apple_barcode_scan::AppleBarcodeReader),
    );
    #[cfg(target_os = "ios")]
    super::ios_barcode_scan::drive(state, sender);
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    finish_unsupported(state, sender);
}

/// 対応表で `no` / 対象外の OS: 写真選択を開かず `Unsupported` で完了させる。
#[cfg(any(test, not(any(target_os = "macos", target_os = "ios"))))]
fn finish_unsupported(state: &BarcodeScanState, sender: &CompletionSender<BarcodeScanState>) {
    let Some(generation) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_scanning));
    let _ = sender.send(Box::new(move |s: &mut BarcodeScanState| {
        finish_scan(s, generation, Err(ScanFailure::Unsupported));
    }));
}

/// デスクトップ版の要求 1 件の処理（テストから任意の実装を差し込めるよう切り出す）。
#[cfg(any(test, target_os = "macos"))]
fn drive_desktop(
    state: &BarcodeScanState,
    sender: &CompletionSender<BarcodeScanState>,
    picker: &dyn PhotoPicker,
    reader: Arc<dyn BarcodeReader>,
) {
    let Some(generation) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_scanning));

    let finish = |sender: &CompletionSender<BarcodeScanState>,
                  result: Result<Option<Vec<String>>, ScanFailure>| {
        // 受信側のドロップ（アプリ終了中）による送信失敗は無視してよい。
        let _ = sender.send(Box::new(move |s: &mut BarcodeScanState| {
            finish_scan(s, generation, result);
        }));
    };

    let photo = match picker.pick() {
        Ok(photo) => photo,
        // キャンセルは失敗ではない（契約表 10）。
        Err(DeviceError::Cancelled(_)) => return finish(sender, Ok(None)),
        Err(e) => return finish(sender, Err(to_failure(e))),
    };
    if let Err(e) = super::image_recognition::precheck(&photo) {
        return finish(sender, Err(to_failure(e)));
    }

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("barcode-scan".into())
        .spawn(move || {
            let result = reader.read(&photo).map(Some).map_err(to_failure);
            drop(photo);
            let _ = worker_sender.send(Box::new(move |s: &mut BarcodeScanState| {
                finish_scan(s, generation, result);
            }));
        });
    if spawned.is_err() {
        finish(
            sender,
            Err(to_failure(DeviceError::Internal("barcode_scan spawn"))),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::barcode_scan::{
        request_scan, CANCELLED_STATUS, FAILED_PREFIX, INVALID_INPUT_STATUS, NO_CODE_STATUS,
        UNSUPPORTED_STATUS,
    };
    use fandhe_device::barcode_scan::UnsupportedBarcodeReader;
    use fandhe_device::photo_picker::{validate_photo, PickedPhoto};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<BarcodeScanState>>;

    fn sender_with_channel() -> (CompletionSender<BarcodeScanState>, Rx) {
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

    struct FakeReader {
        result: fn() -> Result<Vec<String>, DeviceError>,
        called: Arc<AtomicBool>,
    }
    impl BarcodeReader for FakeReader {
        fn read(&self, _photo: &PickedPhoto) -> Result<Vec<String>, DeviceError> {
            self.called.store(true, Ordering::SeqCst);
            (self.result)()
        }
    }

    /// 要求を立てて `drive_desktop` し、届いた完了（mark と finish の 2 件）を順に適用した
    /// 状態と、reader が呼ばれたかを返す。
    fn run(
        picker: fn() -> Result<PickedPhoto, DeviceError>,
        result: fn() -> Result<Vec<String>, DeviceError>,
    ) -> (BarcodeScanState, bool) {
        let (sender, rx) = sender_with_channel();
        let mut state = BarcodeScanState::default();
        request_scan(&mut state);
        let called = Arc::new(AtomicBool::new(false));
        let reader = Arc::new(FakeReader {
            result,
            called: called.clone(),
        });
        drive_desktop(&state, &sender, &FakePicker(picker), reader);
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
            &BarcodeScanState::default(),
            &sender,
            &FakePicker(ok_photo),
            Arc::new(UnsupportedBarcodeReader),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_shows_payloads() {
        let (s, called) = run(ok_photo, || Ok(vec!["a".into(), "b".into()]));
        assert!(called);
        assert_eq!(s.status, "2 件");
        assert_eq!(s.payloads, ["a", "b"]);
        assert!(s.error.is_empty());
    }

    #[test]
    fn zero_payloads_is_no_code_success() {
        let (s, _) = run(ok_photo, || Ok(vec![]));
        assert_eq!(s.status, NO_CODE_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn picker_cancel_is_cancel_and_skips_reader() {
        let (s, called) = run(
            || Err(DeviceError::Cancelled("photo_picker")),
            || Ok(vec![]),
        );
        assert!(!called);
        assert_eq!(s.status, CANCELLED_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn picker_failures_are_mapped_and_skip_reader() {
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
    fn reader_failures_are_mapped() {
        let (s, _) = run(ok_photo, || Err(DeviceError::Unsupported("barcode_scan")));
        assert_eq!(s.error, UNSUPPORTED_STATUS);
        let (s, _) = run(ok_photo, || Err(DeviceError::InvalidInput("x")));
        assert_eq!(s.error, INVALID_INPUT_STATUS);
        let (s, _) = run(ok_photo, || Err(DeviceError::Internal("secret-id")));
        assert!(s.error.starts_with(FAILED_PREFIX));
        assert!(!s.error.contains("secret-id"), "内部識別子を出さない");
        assert!(s.payloads.is_empty() && s.status.is_empty());
    }

    #[test]
    fn oversize_header_is_invalid_input_without_calling_reader() {
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

    #[test]
    fn fallback_finishes_unsupported_without_picker() {
        let (sender, rx) = sender_with_channel();
        let mut state = BarcodeScanState::default();
        request_scan(&mut state);
        finish_unsupported(&state, &sender);
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        }
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert!(state.payloads.is_empty());
    }

    #[test]
    fn stale_generation_completion_is_not_applied() {
        let mut state = BarcodeScanState::default();
        request_scan(&mut state);
        mark_scanning(&mut state);
        finish_scan(&mut state, 99, Ok(Some(vec!["x".into()])));
        assert!(state.payloads.is_empty());
        assert_eq!(
            state.status,
            fandhe_core::screens::barcode_scan::SCANNING_STATUS
        );
    }

    #[test]
    fn owns_picker_id_is_exclusive_with_ocr_and_plain_ids() {
        assert!(owns_picker_id(IOS_PICKER_TAG | 1));
        assert!(owns_picker_id(IOS_PICKER_TAG | 12345));
        for id in [1u64, 2, u32::MAX as u64] {
            assert!(!owns_picker_id(id));
        }
        assert!(!owns_picker_id((1 << 63) | 1));
        assert!(!owns_picker_id((1 << 63) | IOS_PICKER_TAG | 1));
    }

    fn picked(photo: PickedPhoto) -> Result<Option<PickedFile>, DeviceError> {
        Ok(Some(PickedFile {
            name: "a".into(),
            bytes: photo.bytes,
        }))
    }

    fn scan_run(
        input: Result<Option<PickedFile>, DeviceError>,
        result: fn() -> Result<Vec<String>, DeviceError>,
    ) -> (Result<Option<Vec<String>>, ScanFailure>, bool) {
        let called = Arc::new(AtomicBool::new(false));
        let out = scan_picked(
            input,
            &FakeReader {
                result,
                called: called.clone(),
            },
        );
        (out, called.load(Ordering::SeqCst))
    }

    #[test]
    fn scan_picked_success_and_zero_codes() {
        let (o, c) = scan_run(picked(png_with_size(10, 10)), || Ok(vec!["a".into()]));
        assert!(c);
        assert_eq!(o, Ok(Some(vec!["a".to_string()])));
        let (o, _) = scan_run(picked(png_with_size(10, 10)), || Ok(vec![]));
        assert_eq!(o, Ok(Some(vec![])));
    }

    #[test]
    fn scan_picked_cancel_skips_reader() {
        for input in [Ok(None), Err(DeviceError::Cancelled("x"))] {
            let (o, c) = scan_run(input, || Ok(vec![]));
            assert_eq!(o, Ok(None));
            assert!(!c);
        }
    }

    #[test]
    fn scan_picked_invalid_inputs_skip_reader() {
        let (o, c) = scan_run(Err(DeviceError::Internal("x")), || Ok(vec![]));
        assert!(matches!(o, Err(ScanFailure::Failed(_))));
        assert!(!c);
        let big = png_with_size(1, fandhe_device::photo_picker::MAX_PHOTO_SIDE + 1);
        let (o, c) = scan_run(picked(big), || Ok(vec!["x".into()]));
        assert_eq!(o, Err(ScanFailure::InvalidInput));
        assert!(!c);
        // IHDR を欠く PNG と形式不明のバイト列は成功にしない。
        for bytes in [
            vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            vec![1, 2, 3, 4],
        ] {
            let input = Ok(Some(PickedFile {
                name: "a".into(),
                bytes,
            }));
            let (o, c) = scan_run(input, || Ok(vec!["x".into()]));
            assert!(matches!(o, Err(ScanFailure::Failed(_))), "{o:?}");
            assert!(!c);
        }
    }

    #[test]
    fn scan_picked_reader_failure_hides_internal_id() {
        let (o, _) = scan_run(picked(png_with_size(10, 10)), || {
            Err(DeviceError::Internal("secret-id"))
        });
        match o {
            Err(ScanFailure::Failed(m)) => assert!(!m.contains("secret-id")),
            other => panic!("{other:?}"),
        }
    }
}
