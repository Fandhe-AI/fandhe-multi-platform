//! Phase 11（親 #116）型サンプル「バーコード・QR 読み取り」（Issue #557）の effect ドライバ（機能 crate `fandhe-cap-barcode-scan` の中核、
//! Issue #1355）。`fandhe_core::screens::barcode_scan`（端末依存ゼロ）の要求を観測し、写真選択（機能 crate `fandhe-cap-photo-picker`）で
//! 選ばせた画像 1 枚を [`BarcodeReader`] で読んで `CompletionSender` 経由で結果を返す（`fandhe-gallery` の `screens/registry.rs` の
//! `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`fandhe_runtime::EffectFn` 参照）。
//!
//! Web は `fandhe-web-dom` 側（#559）で扱い、契約の純関数 [`normalize_payloads`] だけをこの crate から使う（wasm32 では写真選択の crate
//! に依存しないため、この driver の OS 経路は wasm32 でコンパイルされない）。契約（結果の形・失敗の区別・守る範囲）の SSOT は
//! [`crate::contract`]。
//!
//! # OS 別の分担
//!
//! - macOS: `apple::AppleBarcodeReader`（#557）
//! - iOS: `ios::drive`（#558。PHPicker + Vision。macOS と同じ `.m` で読む）。`fandhe_cap_photo_picker::present_ios_photo_picker` へ
//!   要求ごとの受け取りを登録する
//! - それ以外（Windows・Android・Linux）: 対応表で `no` / 対象外。写真選択を開かず `Unsupported` で完了させる
//!   （ダミーの成功実装へは倒さない）。Android は写真選択の crate に依存しない
//!
//! # スレッド（デスクトップ）
//!
//! `NSOpenPanel` は main スレッド必須のため、`photo_pick` と同じく呼ばれたスレッド（egui の update ループ = main）で `pick()` を同期に
//! 呼ぶ。読み取りは UI を止めうるため、選択後はワーカースレッドで実行して完了を送る。二重起動は、`mark_scanning` の適用前に effect が
//! 再度走っても `pick()` が main を塞いで完了が先に積まれるため起きず、画面側の `Idle` 以外 no-op でも防がれる。ワーカーが持つ
//! バイト列は完了送信後に drop される。
//!
//! # 入力の検査
//!
//! 写真は `validate_photo`（8 MiB 上限・形式）済み。加えて OS に渡す前にヘッダーだけで辺・画素数を検査し
//! （`fandhe_device::photo_picker::precheck_dimensions`。文字認識と共有）、超えたら OS を呼ばず `InvalidInput`
//! （圧縮爆弾対策。切り詰めて成功にしない）。ヘッダーを読めない画像も失敗にする。

#[cfg(test)]
use std::sync::Arc;

#[cfg(any(test, target_os = "macos"))]
use fandhe_cap_photo_picker::PhotoPicker;
#[cfg(any(test, target_os = "ios"))]
use fandhe_cap_photo_picker::PhotoPickerError;
use fandhe_core::screens::barcode_scan::{BarcodeScanState, ScanFailure};
#[cfg(any(test, not(target_os = "ios")))]
use fandhe_core::screens::barcode_scan::{finish_scan, mark_scanning, take_request};
#[cfg(any(test, target_os = "ios"))]
use fandhe_device::file_picker::PickedFile;
#[cfg(any(test, target_os = "macos", target_os = "ios"))]
use fandhe_device::photo_picker::precheck_dimensions;
#[cfg(any(test, target_os = "ios"))]
use fandhe_device::photo_picker::validate_photo;
use fandhe_runtime::CompletionSender;

#[cfg(any(test, target_os = "macos", target_os = "ios"))]
use crate::contract::BarcodeReader;
#[cfg(any(test, target_os = "macos", target_os = "ios"))]
use crate::error::BarcodeScanError;

/// 現在のプラットフォームに応じた [`BarcodeReader`] 実装を返す（プロセスで 1 個）。状態型に依存しない入口で、
/// フレームワークの外のアプリも自分の状態型の effect から使える。macOS は Vision、iOS・Windows・Android・Linux は `Unsupported`
/// （iOS は PHPicker が非同期に結果を返すため同期の入口では使えない。画面は [`barcode_scan_effect`] の経路を使う）。
// ponytail: iOS の同期入口は要求が出たら足す（`ios::drive` の登録・受け取りを同期に直すにはスレッド待ちが要る）。
pub fn platform_barcode_scan() -> std::sync::Arc<dyn crate::contract::BarcodeReader> {
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(crate::apple::AppleBarcodeReader)
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::sync::Arc::new(crate::contract::UnsupportedBarcodeReader)
    }
}

/// `BarcodeScanError` を画面の失敗種別へ写す。内部識別子・パス・読み取った中身は利用者向け文言へ
/// 出さず、上記以外は固定文言にする（security.md A09）。
#[cfg(any(test, target_os = "macos", target_os = "ios"))]
pub(crate) fn to_failure(e: BarcodeScanError) -> ScanFailure {
    match e {
        BarcodeScanError::Unsupported(_) => ScanFailure::Unsupported,
        BarcodeScanError::InvalidInput(_) => ScanFailure::InvalidInput,
        _ => ScanFailure::Failed("コードを読み取れませんでした".to_string()),
    }
}

/// PHPicker の結果を検査して読む純粋関数（iOS 経路）。キャンセルは `Ok(None)`（失敗にしない）、
/// 0 件は成功、それ以外の失敗は `to_failure` で固定文言へ写す。バイト列は戻り時に解放される。
/// ホストのテストで検証できるよう `ios` から分離。
#[cfg(any(test, target_os = "ios"))]
pub(crate) fn scan_picked(
    result: Result<Option<PickedFile>, PhotoPickerError>,
    reader: &dyn BarcodeReader,
) -> Result<Option<Vec<String>>, ScanFailure> {
    let file = match result {
        Ok(Some(file)) => file,
        Ok(None) | Err(PhotoPickerError::Cancelled(_)) => return Ok(None),
        Err(e) => return Err(to_failure(e.into())),
    };
    let photo = validate_photo(file.bytes)
        .map_err(BarcodeScanError::from)
        .map_err(to_failure)?;
    precheck_dimensions(&photo)
        .map_err(BarcodeScanError::from)
        .map_err(to_failure)?;
    reader.read(&photo).map(Some).map_err(to_failure)
}

/// `registry.rs` の `effect` 区画から `barcode_scan` 画面用に配線される effect ドライバ。
pub fn barcode_scan_effect(state: &BarcodeScanState, sender: &CompletionSender<BarcodeScanState>) {
    #[cfg(target_os = "macos")]
    drive_desktop(
        state,
        sender,
        &*fandhe_cap_photo_picker::platform_photo_picker(),
        platform_barcode_scan(),
    );
    #[cfg(target_os = "ios")]
    ios_effect::drive(state, sender);
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
    reader: std::sync::Arc<dyn BarcodeReader>,
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
        Err(fandhe_cap_photo_picker::PhotoPickerError::Cancelled(_)) => {
            return finish(sender, Ok(None));
        }
        Err(e) => return finish(sender, Err(to_failure(e.into()))),
    };
    if let Err(e) = precheck_dimensions(&photo) {
        return finish(sender, Err(to_failure(e.into())));
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
            Err(to_failure(BarcodeScanError::Internal("barcode_scan spawn"))),
        );
    }
}

#[cfg(target_os = "ios")]
mod ios_effect {
    //! iOS 向けバーコード・QR 読み取りの effect ドライバ（Issue #558）。`crate::driver` の iOS 経路（`barcode_scan_effect`）から毎フレーム
    //! 呼ばれる。写真選択（PHPicker）の結果を受け、`apple::AppleBarcodeReader`（Vision `VNDetectBarcodesRequest`。`.m` は macOS と共用）で
    //! 読んで画面状態へ反映する。契約（結果の形・失敗の区別）の SSOT は [`crate::contract`]。下敷きは文字認識（`fandhe-cap-image-recognition`）の
    //! iOS 経路。
    //!
    //! # 経路
    //!
    //! 1. `drive` が要求を観測し、自前の [`SLOT`] で採番した id を持つ受け取り（読み取りまで行う閉包）を
    //!    `fandhe_cap_photo_picker::present_ios_photo_picker` へ登録して PHPicker を出す。
    //! 2. 完了は写真選択の crate の受け口（`NSItemProvider` のバックグラウンドキュー）が、登録された受け取りを id で 1 件取り出して呼ぶ。
    //!    読み取りはこのコールバックスレッドで同期に走る（`.m` が `@autoreleasepool` 内で実行し、`dispatch_sync` は使わない）。結果を
    //!    slot へ置いて戻った直後に、受け口が `request_repaint` で次フレームを起こす。
    //! 3. `drive` が毎フレーム `request_slot::poll_and_finish` で結果を拾い、`finish_scan` で反映する。
    //!
    //! 以前は要求 id の目印（`1 << 62`）で写真選択・文字認識と受け口を共有していたが、要求ごとに受け取りを登録する形にしたため、
    //! 機能ごとの目印・振り分けの順序への依存は無い。
    //!
    //! # 守る範囲 / 守らない範囲
    //!
    //! 守る: 1 プロセス内の要求と完了の順序（世代照合・スロット照合）、嘘の成功の禁止（キャンセル・成功・失敗の三値。0 件は成功）、
    //! バイト列の解放。守らない: 実行中の読み取りの取り消し（結果は世代照合で捨てる）、処理時間、OS ごとの検出結果の違い、複数プロセス。
    //! 保留中の要求は復帰時に復元しない（写真選択と同じ）。読んだ中身はログに出さず、URL でも開かない。

    use fandhe_core::screens::barcode_scan::{
        BarcodeScanPhase, BarcodeScanState, ScanFailure, finish_scan, mark_scanning, take_request,
    };
    use fandhe_device::file_picker::PickedFile;
    use fandhe_runtime::CompletionSender;
    use fandhe_runtime::request_slot::{RequestSlot, poll_and_finish};

    use fandhe_cap_photo_picker::{PhotoPickerError, present_ios_photo_picker};

    use super::{scan_picked, to_failure};
    use crate::apple::AppleBarcodeReader;
    use crate::error::BarcodeScanError;

    /// 画面へ反映する完了結果（`None` = キャンセル）。
    type Outcome = Result<Option<Vec<String>>, ScanFailure>;

    static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// PHPicker の結果（`NSItemProvider` のキューから呼ばれる）を読んで slot へ置く。
    /// id 不一致・未登録・重複は捨てて `false`。
    fn deliver(id: u64, result: Result<Option<PickedFile>, PhotoPickerError>) -> bool {
        let outcome = scan_picked(result, &AppleBarcodeReader);
        SLOT.deliver(id, outcome)
    }

    /// iOS 版 effect 本体。
    pub(crate) fn drive(state: &BarcodeScanState, sender: &CompletionSender<BarcodeScanState>) {
        if take_request(state).is_some() {
            let _ = sender.send(Box::new(mark_scanning));
            let id = SLOT.begin();
            // iOS では通常 true（提示失敗も非同期に通知される）。false は提示依頼自体の同期的な失敗なので失敗で閉じる。
            let presented = present_ios_photo_picker(Box::new(move |result| {
                deliver(id, result);
            }));
            if !presented {
                SLOT.deliver(
                    id,
                    Err(to_failure(BarcodeScanError::Internal(
                        "barcode_scan present",
                    ))),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::UnsupportedBarcodeReader;
    use fandhe_core::screens::barcode_scan::{
        CANCELLED_STATUS, FAILED_PREFIX, INVALID_INPUT_STATUS, NO_CODE_STATUS, UNSUPPORTED_STATUS,
        request_scan,
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

    type Rx = std::sync::mpsc::Receiver<fandhe_runtime::Completion<BarcodeScanState>>;

    fn sender_with_channel() -> (CompletionSender<BarcodeScanState>, Rx) {
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

    struct FakeReader {
        result: fn() -> Result<Vec<String>, BarcodeScanError>,
        called: Arc<AtomicBool>,
    }
    impl BarcodeReader for FakeReader {
        fn read(&self, _photo: &PickedPhoto) -> Result<Vec<String>, BarcodeScanError> {
            self.called.store(true, Ordering::SeqCst);
            (self.result)()
        }
    }

    /// 要求を立てて `drive_desktop` し、届いた完了（mark と finish の 2 件）を順に適用した
    /// 状態と、reader が呼ばれたかを返す。
    fn run(
        picker: fn() -> Result<PickedPhoto, PhotoPickerError>,
        result: fn() -> Result<Vec<String>, BarcodeScanError>,
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

    fn ok_photo() -> Result<PickedPhoto, PhotoPickerError> {
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
            || Err(PhotoPickerError::Cancelled("photo_picker")),
            || Ok(vec![]),
        );
        assert!(!called);
        assert_eq!(s.status, CANCELLED_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn picker_failures_are_mapped_and_skip_reader() {
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
    fn reader_failures_are_mapped() {
        let (s, _) = run(ok_photo, || {
            Err(BarcodeScanError::Unsupported("barcode_scan"))
        });
        assert_eq!(s.error, UNSUPPORTED_STATUS);
        let (s, _) = run(ok_photo, || Err(BarcodeScanError::InvalidInput("x")));
        assert_eq!(s.error, INVALID_INPUT_STATUS);
        let (s, _) = run(ok_photo, || Err(BarcodeScanError::Internal("secret-id")));
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
                    .map_err(|_| PhotoPickerError::Internal("test"))
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

    fn picked(photo: PickedPhoto) -> Result<Option<PickedFile>, PhotoPickerError> {
        Ok(Some(PickedFile {
            name: "a".into(),
            bytes: photo.bytes,
        }))
    }

    fn scan_run(
        input: Result<Option<PickedFile>, PhotoPickerError>,
        result: fn() -> Result<Vec<String>, BarcodeScanError>,
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
        for input in [Ok(None), Err(PhotoPickerError::Cancelled("x"))] {
            let (o, c) = scan_run(input, || Ok(vec![]));
            assert_eq!(o, Ok(None));
            assert!(!c);
        }
    }

    #[test]
    fn scan_picked_invalid_inputs_skip_reader() {
        let (o, c) = scan_run(Err(PhotoPickerError::Internal("x")), || Ok(vec![]));
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
            Err(BarcodeScanError::Internal("secret-id"))
        });
        match o {
            Err(ScanFailure::Failed(m)) => assert!(!m.contains("secret-id")),
            other => panic!("{other:?}"),
        }
    }
}
