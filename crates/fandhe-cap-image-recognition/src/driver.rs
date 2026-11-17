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
//! - iOS: `ios_effect::drive`（#1252。PHPicker + Vision（`.m` は macOS と共用））。写真選択がコールバック型のため同期の `PhotoPicker` を通らず、
//!   `fandhe_cap_photo_picker::present_ios_photo_picker` へ要求ごとの受け取りを登録する。状態型に依存しない入口は begin / poll / discard 型の
//!   非同期（[`begin_image_recognition`] / [`poll_image_recognition`] / [`discard_image_recognition`]）で、画面 effect と slot を持ち主で分けて共有する
//!   （`consumer-app-structure.md` §8.2）
//! - Android: `unsupported_effect::drive`（#1253 まで要求を `Unsupported` で完了させる骨格。写真選択の crate には依存しない）
//! - wasm32: 同じく `unsupported_effect::drive`（Web は対象外。写真選択の crate に依存しない）
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

#[cfg(any(
    test,
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
))]
use fandhe_cap_photo_picker::PhotoPicker;
#[cfg(any(test, target_os = "ios"))]
use fandhe_cap_photo_picker::PhotoPickerError;
use fandhe_core::screens::image_recognize::{ImageRecognizeState, RecognizeFailure};
#[cfg(any(
    test,
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
))]
use fandhe_core::screens::image_recognize::{finish_recognize, mark_recognizing, take_request};
#[cfg(any(test, not(any(target_os = "android", target_arch = "wasm32"))))]
use fandhe_device::photo_picker::{PickedPhoto, precheck_dimensions};
#[cfg(any(test, target_os = "ios"))]
use fandhe_device::{file_picker::PickedFile, photo_picker::validate_photo};
use fandhe_runtime::CompletionSender;

use crate::contract::LANGUAGE_UNAVAILABLE;
#[cfg(any(test, not(any(target_os = "android", target_arch = "wasm32"))))]
use crate::contract::TextRecognizer;
use crate::error::ImageRecognitionError;

/// 現在のプラットフォームに応じた [`TextRecognizer`] 実装を返す（プロセスで 1 個）。状態型に依存しない入口で、
/// フレームワークの外のアプリも自分の状態型の effect から使える。macOS は Vision、Windows は `OcrEngine`、
/// iOS・Android・Linux は `Unsupported` 固定（iOS は PHPicker が非同期に結果を返すため、同期の入口では使えない。
/// iOS は [`begin_image_recognition`] / [`poll_image_recognition`] を使う。Android は #1253 まで骨格）。
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
#[cfg_attr(any(target_os = "android", target_arch = "wasm32"), allow(dead_code))]
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
#[cfg(any(test, not(any(target_os = "android", target_arch = "wasm32"))))]
pub(crate) fn precheck(photo: &PickedPhoto) -> Result<(), ImageRecognitionError> {
    precheck_dimensions(photo).map_err(ImageRecognitionError::from)
}

/// iOS の PHPicker 結果（`ios_effect::drive`・[`begin_image_recognition`] が登録した受け取りが OS コールバックスレッドで渡す）を
/// 検査して OCR する純粋関数。キャンセルは `Ok(None)`（失敗にしない）、0 行は成功、失敗は種別のまま返す。
/// ホストのテストで検証できるよう分離。
#[cfg(any(test, target_os = "ios"))]
pub(crate) fn recognize_picked_raw(
    result: Result<Option<PickedFile>, PhotoPickerError>,
    recognizer: &dyn TextRecognizer,
) -> Result<Option<Vec<String>>, ImageRecognitionError> {
    let file = match result {
        Ok(Some(file)) => file,
        Ok(None) | Err(PhotoPickerError::Cancelled(_)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let photo = validate_photo(file.bytes).map_err(ImageRecognitionError::from)?;
    precheck(&photo)?;
    recognizer.recognize(&photo).map(Some)
}

/// [`recognize_picked_raw`] の失敗を画面の失敗種別へ写す（[`to_failure`] で固定文言へ）。実機では画面側が `to_failure` を直接使うため、テストだけが使う。
#[cfg(test)]
pub(crate) fn recognize_picked(
    result: Result<Option<PickedFile>, PhotoPickerError>,
    recognizer: &dyn TextRecognizer,
) -> Result<Option<Vec<String>>, RecognizeFailure> {
    recognize_picked_raw(result, recognizer).map_err(to_failure)
}

/// iOS の PHPicker 結果を受ける slot と持ち主（pull 型、`fandhe_runtime::request_slot::RequestSlot` の facade）。
///
/// 画面 effect（`ios_effect::drive`）と公開入口（[`begin_image_recognition`] / [`poll_image_recognition`] / [`discard_image_recognition`]）が同じ slot を共有する。
/// 持ち主（[`bridge::SCREEN_OWNER`]・[`bridge::PUBLIC_OWNER`]）で自分の分の要求を区別し、片方が進行中・結果未取得のとき他方は置き換えず
/// busy で断る。slot には変換前の生の結果（ImageRecognitionError）を置き、画面へ渡すときに [`to_failure`] で写す。
///
/// テストでホストからも検証できるよう `cfg(test)` でもコンパイルする（OS 呼び出しは含まない）。
#[cfg(any(test, target_os = "ios"))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) mod bridge {
    use std::sync::Mutex;

    use fandhe_cap_photo_picker::PhotoPickerError;
    use fandhe_device::file_picker::PickedFile;
    use fandhe_runtime::request_slot::{RequestSlot, SlotOwner};

    use crate::contract::TextRecognizer;
    use crate::error::ImageRecognitionError;

    /// 公開入口が追う自分の分の要求（画面側が置き換え・横取りしない）。
    pub(crate) static PUBLIC_OWNER: SlotOwner = SlotOwner::new();
    /// 画面 effect が追う自分の分の要求（公開入口と slot を共有するため区別する）。
    pub(crate) static SCREEN_OWNER: SlotOwner = SlotOwner::new();
    /// 公開側の poll・discard（持ち主の照合・取得・喪失判定・解放）を直列にする。並行 poll で成功と喪失が二重に返らないため。
    static PUBLIC_POLL_LOCK: Mutex<()> = Mutex::new(());

    /// 完了結果（`None` = キャンセル）。
    pub(crate) type Outcome = Result<Option<Vec<String>>, ImageRecognitionError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 画面の要求を登録する。公開入口の進行中・未取得の要求があれば置き換えず `None`（busy）。
    pub(crate) fn begin_screen() -> Option<u64> {
        SCREEN_OWNER.begin_keeping_foreign(&SLOT)
    }

    /// 公開入口の要求を登録する。進行中の要求と自分の未取得の結果があれば `None`（busy）。画面側の届いた未取得の結果は置き換える
    /// （画面の effect が回らず取り出されないままでも公開入口が詰まらないため。置き換えられた画面の poll は lost の失敗になる。結果の到着から poll までの間に呼ばれても置き換え、画面の本物の成功結果が lost の失敗として報告される。成功には化けない）。
    pub(crate) fn begin_public() -> Option<u64> {
        PUBLIC_OWNER.begin_excluding(&SLOT, &[&PUBLIC_OWNER])
    }

    /// 公開入口の提示依頼が同期的に失敗したときに、登録を取り消す。
    pub(crate) fn abort_public(request_id: u64) {
        SLOT.clear(request_id);
        PUBLIC_OWNER.release(request_id);
    }

    /// 結果を置く（任意スレッド）。`request_id` 不一致・未登録・重複は捨てて `false`。
    pub(crate) fn deliver(request_id: u64, outcome: Outcome) -> bool {
        SLOT.deliver(request_id, outcome)
    }

    /// PHPicker の結果（`NSItemProvider` のキューから呼ばれる）を検査し、OCRまで済ませて slot へ置く。
    /// 画面・公開入口のどちらの要求も同じ検査（`validate_photo` → ヘッダー検査 → OS）を通る。
    pub(crate) fn deliver_picked(
        request_id: u64,
        result: Result<Option<PickedFile>, PhotoPickerError>,
        recognizer: &dyn TextRecognizer,
    ) -> bool {
        deliver(request_id, super::recognize_picked_raw(result, recognizer))
    }

    /// 公開入口の結果を取り出す（[`super::poll_image_recognition`] の本体）。届いていれば 1 度だけ `Some`、未着・旧 id・公開入口でない id は `None`。
    /// slot の要求が失われたとき（reset 等）は、1 度だけ `Err(Internal("image_recognition lost"))`、以後は `None`。
    /// 照合から解放までを直列にし、並行 poll でも成功と喪失が二重に返らない。
    pub(crate) fn poll_public(request_id: u64) -> Option<Outcome> {
        let _guard = PUBLIC_POLL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if request_id == 0 || PUBLIC_OWNER.current() != request_id {
            return None;
        }
        if let Some(outcome) = SLOT.take_ready(request_id) {
            PUBLIC_OWNER.release(request_id);
            return Some(outcome);
        }
        if SLOT.snapshot().is_some_and(|(id, _)| id == request_id) {
            return None;
        }
        PUBLIC_OWNER.release(request_id);
        Some(Err(ImageRecognitionError::Internal(
            "image_recognition lost",
        )))
    }

    /// 公開入口の自分の分の要求を捨てる（[`super::discard_image_recognition`] の本体）。持ち主が `request_id` と一致するときだけ
    /// slot を空にして持ち主を外し `true`。他人の id・取り出し済み・0 は `false`。poll と同じロックで直列にする。
    pub(crate) fn discard_public(request_id: u64) -> bool {
        let _guard = PUBLIC_POLL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if request_id == 0 || PUBLIC_OWNER.current() != request_id {
            return false;
        }
        SLOT.clear(request_id);
        PUBLIC_OWNER.release(request_id);
        true
    }

    /// テスト用: slot を空にする（持ち主の印は残る。喪失の検査に使う）。
    #[cfg(test)]
    pub(crate) fn reset() {
        SLOT.reset();
    }
}

/// 写真選択を開き、選ばれた画像をOCR し、結果を [`poll_image_recognition`] で受け取るための要求 id を返す（iOS）。
///
/// 画面の状態型に依存しない非同期の入口で、フレームワークの外のアプリも使える。画面 effect と slot を共有する。
///
/// - 進行中の要求は busy で断る。
/// - 画面側の届いた未取得の結果は公開入口が置き換える（画面が描かれず取り出されないままでも詰まらない）。置き換えられた画面が後で poll すると lost の失敗になり、成功にはならない。
///   結果が届いてから次フレームの poll までの間に公開入口を始めた場合も、画面の本物の成功結果が置き換えで lost の失敗として報告される（失敗側に倒れ、成功には化けない）。
/// - 自分の未取得の結果があれば `Internal("image_recognition busy")` で断る。
/// - 写真選択（PHPicker）が進行中のとき（[`discard_image_recognition`] で捨てた後を含む）も、
///   OS の観測で写真選択側が busy を返し、結果は失敗（`Internal`）で届く。依頼自体の失敗は `Err`（結果は届かない）。
///
/// 結果の解放: 選んだ画像のバイト列は OS コールバックの中で検査・OCR後に解放され、結果が握るのは認識した行だけ。
/// 取り出さない要求が slot に残ると次の要求も画面 effect の開始も busy で断られるため、不要な要求は [`discard_image_recognition`] で捨てる。
///
/// iOS 以外では `Unsupported`（macOS は同期の [`platform_image_recognition`] と写真選択（`fandhe_cap_photo_picker::platform_photo_picker`）を使う。Android は骨格、Windows・Linux は対象外）。
#[cfg(target_os = "ios")]
pub fn begin_image_recognition() -> Result<u64, ImageRecognitionError> {
    let request_id =
        bridge::begin_public().ok_or(ImageRecognitionError::Internal("image_recognition busy"))?;
    let presented = fandhe_cap_photo_picker::present_ios_photo_picker(Box::new(move |result| {
        bridge::deliver_picked(request_id, result, &crate::apple::AppleTextRecognizer);
    }));
    if !presented {
        bridge::abort_public(request_id);
        return Err(ImageRecognitionError::Internal("image_recognition present"));
    }
    Ok(request_id)
}

/// iOS 以外では `Unsupported`（[`platform_image_recognition`] 参照）。
#[cfg(not(target_os = "ios"))]
pub fn begin_image_recognition() -> Result<u64, ImageRecognitionError> {
    Err(ImageRecognitionError::Unsupported("image_recognition"))
}

/// [`begin_image_recognition`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・公開入口でない id・取り出し済みは `None`。
///
/// 結果は 3 値: 選択・認識 `Ok(Some(..))`（0 件・0 行は成功）、キャンセル `Ok(None)`（失敗ではない）、失敗 `Err`（上限超過は `InvalidInput`、
/// ヘッダー不正・OS の失敗・写真選択の失敗・busy は `Internal` 等）。slot の要求が失われたとき（reset 等）は、1 度だけ
/// `Err(Internal("image_recognition lost"))` を返し、以後は `None`。並行して同じ id を poll しても、結果と喪失が二重に返ることはない。iOS 以外では常に `None`。
pub fn poll_image_recognition(
    request_id: u64,
) -> Option<Result<Option<Vec<String>>, ImageRecognitionError>> {
    #[cfg(target_os = "ios")]
    {
        bridge::poll_public(request_id)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = request_id;
        None
    }
}

/// [`begin_image_recognition`] の自分の要求を捨てて slot を空ける。`request_id` が公開入口の現在の要求のときだけ `true`
/// （他人の id・画面側の id・取り出し済み・0 は `false` で何もしない）。結果が未取得でも進行中でも捨てる。
///
/// discard は写真選択の画面を閉じない。画面が閉じるまで次の [`begin_image_recognition`]（画面 effect の開始も）は、OS の観測で写真選択側が busy を返し、
/// 失敗（`Internal`）で終わる。後から届いた結果は id 不一致で捨てられ、新しい要求へは結びつかない。iOS 以外では常に `false`。
pub fn discard_image_recognition(request_id: u64) -> bool {
    #[cfg(target_os = "ios")]
    {
        bridge::discard_public(request_id)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = request_id;
        false
    }
}

/// `registry.rs` の `effect` 区画から `image_recognize` 画面用に配線される effect ドライバ。
pub fn image_recognize_effect(
    state: &ImageRecognizeState,
    sender: &CompletionSender<ImageRecognizeState>,
) {
    #[cfg(target_os = "ios")]
    ios_effect::drive(state, sender);
    #[cfg(any(target_os = "android", target_arch = "wasm32"))]
    unsupported_effect::drive(state, sender);
    #[cfg(not(any(target_os = "ios", target_os = "android", target_arch = "wasm32")))]
    drive_desktop(
        state,
        sender,
        &*fandhe_cap_photo_picker::platform_photo_picker(),
        platform_image_recognition(),
    );
}

/// デスクトップ版の要求 1 件の処理（テストから任意の実装を差し込めるよう切り出す）。
#[cfg(any(
    test,
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
))]
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
    //! 1. `drive` が要求を観測し、[`bridge::SLOT`] で採番した id を持つ受け取り（OCR まで行う閉包）を
    //!    `fandhe_cap_photo_picker::present_ios_photo_picker` へ登録して PHPicker を出す。公開入口（[`super::begin_image_recognition`]）も
    //!    同じ slot・同じ検査を使い、持ち主（`SCREEN_OWNER` / `PUBLIC_OWNER`）で画面と分ける。
    //! 2. 完了は写真選択の crate の受け口（`NSItemProvider` のバックグラウンドキュー）が、登録された受け取りを id で 1 件取り出して呼ぶ。
    //!    受け取りは元のバイト列をここで検査・OCR する（写真選択の `bridge` は表示用に縮小した画素しか持たず OCR に使えない）。OCR は
    //!    main ではなくこのコールバックスレッドで同期に走る（`.m` が `@autoreleasepool` 内で実行し、`dispatch_sync` は使わない）。
    //!    結果を slot へ置いて戻った直後に、受け口が `request_repaint` で次フレームを起こす。
    //! 3. `drive` が毎フレーム `request_slot::poll_and_finish_owned` で自分の分の結果を拾い、`finish_recognize` で反映する。slot の要求が
    //!    失われたとき（reset 等）は処理中のまま止めず失敗で確定する。
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
        ImageRecognizePhase, ImageRecognizeState, finish_recognize, mark_recognizing, take_request,
    };
    use fandhe_runtime::CompletionSender;
    use fandhe_runtime::request_slot::poll_and_finish_owned;

    use fandhe_cap_photo_picker::present_ios_photo_picker;

    use super::{bridge, to_failure};
    use crate::apple::AppleTextRecognizer;
    use crate::error::ImageRecognitionError;

    /// iOS 版 effect 本体。
    pub(crate) fn drive(
        state: &ImageRecognizeState,
        sender: &CompletionSender<ImageRecognizeState>,
    ) {
        if let Some(generation) = take_request(state) {
            let _ = sender.send(Box::new(mark_recognizing));
            // 公開入口の進行中・未取得の要求は置き換えず、画面側を処理中の失敗で終える。
            let Some(id) = bridge::begin_screen() else {
                let failure = to_failure(ImageRecognitionError::Internal("image_recognition busy"));
                let _ = sender.send(Box::new(move |s: &mut ImageRecognizeState| {
                    finish_recognize(s, generation, Err(failure));
                }));
                return;
            };
            // iOS では通常 true（提示失敗も非同期に通知される）。false は提示依頼自体の同期的な失敗なので失敗で閉じる。
            let presented = present_ios_photo_picker(Box::new(move |result| {
                bridge::deliver_picked(id, result, &AppleTextRecognizer);
            }));
            if !presented {
                bridge::deliver(
                    id,
                    Err(ImageRecognitionError::Internal("image_recognition present")),
                );
            }
            return;
        }
        if state.phase != ImageRecognizePhase::Recognizing {
            return;
        }
        let generation = state.generation;
        poll_and_finish_owned(
            &bridge::SLOT,
            &bridge::SCREEN_OWNER,
            sender,
            |_| {},
            move |s: &mut ImageRecognizeState, outcome| {
                // Idle へ戻っていれば反映せず slot に残す（`false`。残った結果は公開入口・画面の次の開始が置き換える）。別世代は反映せず畳む。
                if s.phase == ImageRecognizePhase::Idle {
                    return false;
                }
                if s.generation != generation {
                    return true;
                }
                finish_recognize(s, generation, outcome.map_err(to_failure));
                true
            },
            // slot の要求が失われた（reset 等）。処理中のまま止めず失敗で確定する（`finish_recognize` は世代違い・完了済みを無視する）。
            move |s: &mut ImageRecognizeState| {
                let failure = to_failure(ImageRecognitionError::Internal("image_recognition lost"));
                finish_recognize(s, generation, Err(failure));
            },
        );
    }
}

#[cfg(any(target_os = "android", target_arch = "wasm32"))]
mod unsupported_effect {
    //! Android 向け文字認識の骨格（Issue #1250 で置いたスタブ。実装は #1253）と、wasm32（Web は対象外。写真選択の crate に依存しない）の
    //! 固定の失敗。`crate::driver` の Android・wasm32 経路から毎フレーム呼ばれる。#1253 が ML Kit の
    //! Java ヘルパークラス・JNI・`RequestSlot` を使った実装に本ファイルの `drive` を置き換える
    //! （dispatch・画面・文言は #1250 で最終形）。
    //!
    //! 現状は要求を観測したら `Unsupported` の失敗を 1 回返す（画面が処理中のまま止まらず、
    //! 嘘の成功にもならない）。

    use fandhe_core::screens::image_recognize::{
        ImageRecognizeState, RecognizeFailure, finish_recognize, mark_recognizing, take_request,
    };

    use fandhe_runtime::CompletionSender;

    /// Android・wasm32 版 effect 本体。#1253 まで要求を `Unsupported` で完了させるだけ。
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

    /// 公開入口の iOS 結果の検査を通す（`bridge::deliver_picked`）。キャンセルは `Ok(None)`、成功は検査・OCRを通った値。
    fn ok_file() -> Result<Option<PickedFile>, PhotoPickerError> {
        picked(png_with_size(10, 10))
    }

    struct Reader(fn() -> Result<Vec<String>, ImageRecognitionError>);
    impl TextRecognizer for Reader {
        fn recognize(&self, _p: &PickedPhoto) -> Result<Vec<String>, ImageRecognitionError> {
            (self.0)()
        }
    }

    /// `bridge` の static（slot・持ち主）を触るテストを直列にする。
    static BRIDGE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        let g = BRIDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        g
    }

    /// 起床を捨てるだけの waker（置き去り画面の poll を直接確かめるテスト用）。
    struct LostWaker;
    impl fandhe_runtime::RepaintWaker for LostWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    /// 画面の届いた未取得の結果は公開入口が置き換え、置き換えられた画面が後で poll すると lost の失敗になる（成功値は反映されない）。
    #[test]
    fn public_begin_replaces_unpolled_screen_result_and_screen_poll_is_lost() {
        let _g = lock();
        let screen = bridge::begin_screen().unwrap();
        assert!(bridge::deliver(screen, Ok(None)));
        let public = bridge::begin_public().expect("画面の取り出されない結果で詰まらない");
        assert_ne!(public, screen);
        // 古い画面の id への配送は捨てられる。
        assert!(!bridge::deliver(screen, Ok(None)));
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = fandhe_runtime::CompletionSender::new(tx, std::sync::Arc::new(LostWaker));
        fandhe_runtime::request_slot::poll_and_finish_owned(
            &bridge::SLOT,
            &bridge::SCREEN_OWNER,
            &sender,
            |_| panic!("待機にはならない"),
            |_s: &mut u8, _o| panic!("成功値は反映されない"),
            |s: &mut u8| *s = 9,
        );
        let mut state = 0u8;
        rx.try_recv().expect("on_lost が送られる")(&mut state);
        assert_eq!(state, 9);
        assert_eq!(bridge::SCREEN_OWNER.current(), 0);
        assert!(rx.try_recv().is_err());
        bridge::reset();
    }

    /// 画面の要求が進行中なら公開入口は busy で、画面の持ち主は変わらない。
    #[test]
    fn public_begin_is_busy_while_screen_request_is_in_flight() {
        let _g = lock();
        let screen = bridge::begin_screen().unwrap();
        assert!(bridge::begin_public().is_none());
        assert_eq!(bridge::SCREEN_OWNER.current(), screen);
        bridge::reset();
    }

    /// 公開入口の届いた未取得の結果は、公開入口も画面も置き換えない。
    #[test]
    fn public_begin_keeps_unpolled_public_result() {
        let _g = lock();
        let public = bridge::begin_public().unwrap();
        assert!(bridge::deliver(public, Ok(None)));
        assert!(bridge::begin_public().is_none());
        assert!(bridge::begin_screen().is_none());
        assert!(bridge::poll_public(public).is_some());
        assert!(bridge::poll_public(public).is_none());
        bridge::reset();
    }

    /// 公開入口の要求は画面と排他で、進行中も結果の未取得も画面側が落とさない。結果は 1 度だけ取れる。
    #[test]
    fn public_request_is_exclusive_with_screen_and_polled_once() {
        let _g = lock();
        let public = bridge::begin_public().unwrap();
        assert!(bridge::begin_public().is_none());
        assert!(bridge::begin_screen().is_none());
        assert!(bridge::poll_public(public).is_none());
        let reader = Reader(|| Ok(vec!["a".into()]));
        assert!(bridge::deliver_picked(public, ok_file(), &reader));
        // 未取得の公開入口の結果を画面側が落とさない。
        assert!(bridge::begin_screen().is_none());
        assert_eq!(
            bridge::poll_public(public),
            Some(Ok(Some(vec!["a".to_string()])))
        );
        assert!(bridge::poll_public(public).is_none());
        // 取り出したあとは画面側が使え、画面の進行中は公開入口が断られる。画面の id は公開入口から見えず捨てられない。
        let screen = bridge::begin_screen().unwrap();
        assert!(bridge::begin_public().is_none());
        assert!(bridge::poll_public(screen).is_none());
        assert!(!bridge::discard_public(screen));
        bridge::reset();
    }

    /// 検査・OS の結果の 3 値（キャンセル・成功・失敗）が公開入口へそのまま届く。
    #[test]
    fn public_result_keeps_three_values() {
        let _g = lock();
        let reader = Reader(|| Ok(vec![]));
        let id = bridge::begin_public().unwrap();
        bridge::deliver_picked(
            id,
            Err(PhotoPickerError::Cancelled("photo_picker")),
            &reader,
        );
        assert_eq!(bridge::poll_public(id), Some(Ok(None)));
        // 0 件・0 行は成功（空の一覧）。
        let id = bridge::begin_public().unwrap();
        bridge::deliver_picked(id, ok_file(), &reader);
        assert_eq!(bridge::poll_public(id), Some(Ok(Some(vec![]))));
        // 写真選択の失敗・上限超過・OS の失敗は成功にしない。
        let id = bridge::begin_public().unwrap();
        bridge::deliver_picked(
            id,
            Err(PhotoPickerError::Internal("photo_picker busy")),
            &reader,
        );
        assert_eq!(
            bridge::poll_public(id),
            Some(Err(ImageRecognitionError::Internal("photo_picker busy")))
        );
        let id = bridge::begin_public().unwrap();
        let big = picked(png_with_size(
            1,
            fandhe_device::photo_picker::MAX_PHOTO_SIDE + 1,
        ));
        bridge::deliver_picked(id, big, &reader);
        assert!(matches!(
            bridge::poll_public(id),
            Some(Err(ImageRecognitionError::InvalidInput(_)))
        ));
        let id = bridge::begin_public().unwrap();
        let failing = Reader(|| Err(ImageRecognitionError::Internal("secret-id")));
        bridge::deliver_picked(id, ok_file(), &failing);
        assert_eq!(
            bridge::poll_public(id),
            Some(Err(ImageRecognitionError::Internal("secret-id")))
        );
        bridge::reset();
    }

    /// slot の要求が失われたとき（reset 等）は、1 度だけ `Internal("image_recognition lost")` を返し、以後は `None`。
    #[test]
    fn public_request_lost_by_reset_fails_once() {
        let _g = lock();
        let id = bridge::begin_public().unwrap();
        assert!(bridge::poll_public(id).is_none());
        bridge::reset();
        assert_eq!(
            bridge::poll_public(id),
            Some(Err(ImageRecognitionError::Internal(
                "image_recognition lost"
            )))
        );
        assert!(bridge::poll_public(id).is_none());
        assert!(bridge::begin_public().is_some());
        bridge::reset();
    }

    #[test]
    fn concurrent_poll_delivers_once() {
        let _g = lock();
        let reader = Reader(|| Ok(vec!["x".into()]));
        for _ in 0..200 {
            bridge::reset();
            let id = bridge::begin_public().unwrap();
            assert!(bridge::deliver_picked(id, ok_file(), &reader));
            let barrier = std::sync::Barrier::new(2);
            let results: Vec<_> = std::thread::scope(|s| {
                let hs: Vec<_> = (0..2)
                    .map(|_| {
                        s.spawn(|| {
                            barrier.wait();
                            bridge::poll_public(id)
                        })
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap()).collect()
            });
            assert_eq!(results.iter().filter(|r| r.is_some()).count(), 1);
            assert!(results.iter().flatten().all(|r| r.is_ok()));
        }
        bridge::reset();
    }

    /// discard は公開入口と画面の両方を再び開始できるようにし、遅れて届いた結果は新しい要求へ結びつかず捨てられる。
    #[test]
    fn discard_frees_slot_and_drops_late_result() {
        let _g = lock();
        let reader = Reader(|| Ok(vec!["late".into()]));
        let id = bridge::begin_public().unwrap();
        // 他人の id・0 では捨てられない。進行中の間は画面も断られる。
        assert!(!bridge::discard_public(id + 1));
        assert!(!bridge::discard_public(0));
        assert!(bridge::begin_screen().is_none());
        assert!(bridge::discard_public(id));
        assert!(!bridge::discard_public(id));
        assert!(bridge::poll_public(id).is_none());
        // 新しい要求が始まったあとに古い結果が遅れて届いても、新しい要求へは結びつかない。
        let next = bridge::begin_public().unwrap();
        assert_ne!(id, next);
        assert!(!bridge::deliver_picked(id, ok_file(), &reader));
        assert!(bridge::poll_public(next).is_none());
        // 結果が未取得でも捨てられ、遅れて届いた結果は捨てられる。
        assert!(bridge::deliver_picked(next, ok_file(), &reader));
        assert!(bridge::discard_public(next));
        assert!(!bridge::deliver_picked(next, ok_file(), &reader));
        assert!(bridge::poll_public(next).is_none());
        // 捨てたあとは画面が使える。
        assert!(bridge::begin_screen().is_some());
        bridge::reset();
    }

    /// 提示依頼の失敗で登録を取り消すと、slot も持ち主も空になり、再び開始できる。
    #[test]
    fn abort_public_empties_slot_and_owner() {
        let _g = lock();
        let id = bridge::begin_public().unwrap();
        bridge::abort_public(id);
        assert!(bridge::SLOT.snapshot().is_none());
        assert_eq!(bridge::PUBLIC_OWNER.current(), 0);
        assert!(bridge::begin_public().is_some());
        bridge::reset();
    }

    /// iOS 以外の公開入口は `Unsupported`。
    #[cfg(not(target_os = "ios"))]
    #[test]
    fn async_entry_is_unsupported_off_ios() {
        assert_eq!(
            begin_image_recognition(),
            Err(ImageRecognitionError::Unsupported("image_recognition"))
        );
        assert!(poll_image_recognition(1).is_none());
        assert!(!discard_image_recognition(1));
    }
}
