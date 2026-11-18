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
//! - iOS: `ios_effect::drive`（#558。PHPicker + Vision。macOS と同じ `.m` で読む）。`fandhe_cap_photo_picker::present_ios_photo_picker` へ
//!   要求ごとの受け取りを登録する。状態型に依存しない入口は begin / poll / discard 型の非同期（[`begin_barcode_scan`] / [`poll_barcode_scan`] /
//!   [`discard_barcode_scan`]）で、画面 effect と slot を持ち主で分けて共有する（`consumer-app-structure.md` §8.2）
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
use crate::error::BarcodeScanError;

/// 現在のプラットフォームに応じた [`BarcodeReader`] 実装を返す（プロセスで 1 個）。状態型に依存しない入口で、
/// フレームワークの外のアプリも自分の状態型の effect から使える。macOS は Vision、iOS・Windows・Android・Linux は `Unsupported`
/// （iOS は PHPicker が非同期に結果を返すため同期の入口では使えない。iOS は [`begin_barcode_scan`] / [`poll_barcode_scan`] を使う）。
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
/// 0 件は成功、失敗は種別のまま返す。バイト列は戻り時に解放される。ホストのテストで検証できるよう `ios` から分離。
#[cfg(any(test, target_os = "ios"))]
pub(crate) fn scan_picked_raw(
    result: Result<Option<PickedFile>, PhotoPickerError>,
    reader: &dyn BarcodeReader,
) -> Result<Option<Vec<String>>, BarcodeScanError> {
    let file = match result {
        Ok(Some(file)) => file,
        Ok(None) | Err(PhotoPickerError::Cancelled(_)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let photo = validate_photo(file.bytes).map_err(BarcodeScanError::from)?;
    precheck_dimensions(&photo).map_err(BarcodeScanError::from)?;
    reader.read(&photo).map(Some)
}

/// [`scan_picked_raw`] の失敗を画面の失敗種別へ写す（[`to_failure`] で固定文言へ）。実機では画面側が `to_failure` を直接使うため、テストだけが使う。
#[cfg(test)]
pub(crate) fn scan_picked(
    result: Result<Option<PickedFile>, PhotoPickerError>,
    reader: &dyn BarcodeReader,
) -> Result<Option<Vec<String>>, ScanFailure> {
    scan_picked_raw(result, reader).map_err(to_failure)
}

/// iOS の PHPicker 結果を受ける slot と持ち主（pull 型、`fandhe_runtime::request_slot::RequestSlot` の facade）。
///
/// 画面 effect（`ios_effect::drive`）と公開入口（[`begin_barcode_scan`] / [`poll_barcode_scan`] / [`discard_barcode_scan`]）が同じ slot を共有する。
/// 持ち主（[`bridge::SCREEN_OWNER`]・[`bridge::PUBLIC_OWNER`]）で自分の分の要求を区別し、
/// 進行中の要求は busy で断り、公開側の未取得の結果は守る。画面側の届いた未取得の結果は、公開入口の開始が捨てずに画面の受け取り待ちへ移し、画面はそのまま受け取る（`SlotOwner::begin_parking`）。
/// slot には変換前の生の結果（BarcodeScanError）を置き、画面へ渡すときに [`to_failure`] で写す。
///
/// テストでホストからも検証できるよう `cfg(test)` でもコンパイルする（OS 呼び出しは含まない）。
#[cfg(any(test, target_os = "ios"))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) mod bridge {
    use std::sync::Mutex;

    use fandhe_cap_photo_picker::PhotoPickerError;
    use fandhe_device::file_picker::PickedFile;
    use fandhe_runtime::request_slot::{RequestSlot, SlotOwner};

    use crate::contract::BarcodeReader;
    use crate::error::BarcodeScanError;

    /// 公開入口が追う自分の分の要求（画面側が置き換え・横取りしない）。
    pub(crate) static PUBLIC_OWNER: SlotOwner = SlotOwner::new();
    /// 画面 effect が追う自分の分の要求（公開入口と slot を共有するため区別する）。
    pub(crate) static SCREEN_OWNER: SlotOwner = SlotOwner::new();
    /// 公開側の poll・discard（持ち主の照合・取得・喪失判定・解放）を直列にする。並行 poll で成功と喪失が二重に返らないため。
    static PUBLIC_POLL_LOCK: Mutex<()> = Mutex::new(());

    /// 完了結果（`None` = キャンセル）。
    pub(crate) type Outcome = Result<Option<Vec<String>>, BarcodeScanError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 画面の要求を登録する。公開入口の進行中・未取得の要求があれば置き換えず `None`（busy）。
    pub(crate) fn begin_screen() -> Option<u64> {
        SCREEN_OWNER.begin_keeping_foreign(&SLOT)
    }

    /// 公開入口の要求を登録する。進行中の要求と自分の未取得の結果があれば `None`（busy）。画面側の届いた未取得の結果は捨てずに
    /// 画面の受け取り待ちへ移して始める（画面の effect が回らず取り出されないままでも詰まらず、生きている画面の結果も失わない）。
    /// 移した結果は画面の poll がそのまま受け取り、受け取られない分は画面の次の開始で解放される
    /// （残るのは最大 1 件で、結果（読み取った文字列の並び）の件数・長さに上限は無い（入力の画像は 8 MiB 以下に検証済み）。
    /// 本番の reset は無く、解放の契機は画面の次の開始だけ）。
    pub(crate) fn begin_public() -> Option<u64> {
        PUBLIC_OWNER.begin_parking(&SLOT, &SCREEN_OWNER)
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

    /// PHPicker の結果（`NSItemProvider` のキューから呼ばれる）を検査し、読み取りまで済ませて slot へ置く。
    /// 画面・公開入口のどちらの要求も同じ検査（`validate_photo` → ヘッダー検査 → OS）を通る。
    pub(crate) fn deliver_picked(
        request_id: u64,
        result: Result<Option<PickedFile>, PhotoPickerError>,
        reader: &dyn BarcodeReader,
    ) -> bool {
        deliver(request_id, super::scan_picked_raw(result, reader))
    }

    /// 公開入口の結果を取り出す（[`super::poll_barcode_scan`] の本体）。届いていれば 1 度だけ `Some`、未着・旧 id・公開入口でない id は `None`。
    /// slot の要求が失われたとき（reset 等）は、1 度だけ `Err(Internal("barcode_scan lost"))`、以後は `None`。
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
        Some(Err(BarcodeScanError::Internal("barcode_scan lost")))
    }

    /// 公開入口の自分の分の要求を捨てる（[`super::discard_barcode_scan`] の本体）。持ち主が `request_id` と一致するときだけ
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

/// 写真選択を開き、選ばれた画像を読み取り、結果を [`poll_barcode_scan`] で受け取るための要求 id を返す（iOS）。
///
/// 画面の状態型に依存しない非同期の入口で、フレームワークの外のアプリも使える。画面 effect と slot を共有する。
///
/// - 進行中の要求は busy で断る。
/// - 画面側の届いた未取得の結果があっても断らない（画面が描かれず取り出されないままでも詰まらない）。結果は捨てずに画面の受け取り待ちへ移し、
///   画面はそのまま受け取る（結果が届いてから次フレームの poll までの間に始めても、画面の結果は失われない）。
///   受け取られない分は画面の次の開始で解放される
///   （残るのは最大 1 件で、結果（読み取った文字列の並び）の件数・長さに上限は無い（入力の画像は 8 MiB 以下に検証済み）。
///   本番の reset は無く、解放の契機は画面の次の開始だけ）。
/// - 自分の未取得の結果があれば `Internal("barcode_scan busy")` で断る。
/// - 写真選択（PHPicker）が進行中のとき（[`discard_barcode_scan`] で捨てた後を含む）も、
///   OS の観測で写真選択側が busy を返し、結果は失敗（`Internal`）で届く。依頼自体の失敗は `Err`（結果は届かない）。
///
/// 結果の解放: 選んだ画像のバイト列は OS コールバックの中で検査・読み取り後に解放され、結果が握るのは読み取った中身（文字列）だけ。
/// 取り出さない要求が slot に残ると次の要求も画面 effect の開始も busy で断られるため、不要な要求は [`discard_barcode_scan`] で捨てる。
///
/// iOS 以外では `Unsupported`（macOS は同期の [`platform_barcode_scan`] と写真選択（`fandhe_cap_photo_picker::platform_photo_picker`）を使う。Windows・Android・Linux は対象外）。
#[cfg(target_os = "ios")]
pub fn begin_barcode_scan() -> Result<u64, BarcodeScanError> {
    let request_id =
        bridge::begin_public().ok_or(BarcodeScanError::Internal("barcode_scan busy"))?;
    let presented = fandhe_cap_photo_picker::present_ios_photo_picker(Box::new(move |result| {
        bridge::deliver_picked(request_id, result, &crate::apple::AppleBarcodeReader);
    }));
    if !presented {
        bridge::abort_public(request_id);
        return Err(BarcodeScanError::Internal("barcode_scan present"));
    }
    Ok(request_id)
}

/// iOS 以外では `Unsupported`（[`platform_barcode_scan`] 参照）。
#[cfg(not(target_os = "ios"))]
pub fn begin_barcode_scan() -> Result<u64, BarcodeScanError> {
    Err(BarcodeScanError::Unsupported("barcode_scan"))
}

/// [`begin_barcode_scan`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・公開入口でない id・取り出し済みは `None`。
///
/// 結果は 3 値: 選択・読み取り `Ok(Some(..))`（0 件・0 行は成功）、キャンセル `Ok(None)`（失敗ではない）、失敗 `Err`（上限超過は `InvalidInput`、
/// ヘッダー不正・OS の失敗・写真選択の失敗・busy は `Internal` 等）。slot の要求が失われたとき（reset 等）は、1 度だけ
/// `Err(Internal("barcode_scan lost"))` を返し、以後は `None`。並行して同じ id を poll しても、結果と喪失が二重に返ることはない。iOS 以外では常に `None`。
pub fn poll_barcode_scan(request_id: u64) -> Option<Result<Option<Vec<String>>, BarcodeScanError>> {
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

/// [`begin_barcode_scan`] の自分の要求を捨てて slot を空ける。`request_id` が公開入口の現在の要求のときだけ `true`
/// （他人の id・画面側の id・取り出し済み・0 は `false` で何もしない）。結果が未取得でも進行中でも捨てる。
///
/// discard は写真選択の画面を閉じない。画面が閉じるまで次の [`begin_barcode_scan`]（画面 effect の開始も）は、OS の観測で写真選択側が busy を返し、
/// 失敗（`Internal`）で終わる。後から届いた結果は id 不一致で捨てられ、新しい要求へは結びつかない。iOS 以外では常に `false`。
pub fn discard_barcode_scan(request_id: u64) -> bool {
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
    //! 1. `drive` が要求を観測し、[`bridge::SLOT`] で採番した id を持つ受け取り（読み取りまで行う閉包）を
    //!    `fandhe_cap_photo_picker::present_ios_photo_picker` へ登録して PHPicker を出す。公開入口（[`super::begin_barcode_scan`]）も
    //!    同じ slot・同じ検査を使い、持ち主（`SCREEN_OWNER` / `PUBLIC_OWNER`）で画面と分ける。
    //! 2. 完了は写真選択の crate の受け口（`NSItemProvider` のバックグラウンドキュー）が、登録された受け取りを id で 1 件取り出して呼ぶ。
    //!    読み取りはこのコールバックスレッドで同期に走る（`.m` が `@autoreleasepool` 内で実行し、`dispatch_sync` は使わない）。結果を
    //!    slot へ置いて戻った直後に、受け口が `request_repaint` で次フレームを起こす。
    //! 3. `drive` が毎フレーム `request_slot::poll_and_finish_owned` で自分の分の結果を拾い、`finish_scan` で反映する。slot の要求が
    //!    失われたとき（reset 等）は処理中のまま止めず失敗で確定する。
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
        BarcodeScanPhase, BarcodeScanState, finish_scan, mark_scanning, take_request,
    };
    use fandhe_runtime::CompletionSender;
    use fandhe_runtime::request_slot::poll_and_finish_owned;

    use fandhe_cap_photo_picker::present_ios_photo_picker;

    use super::{bridge, to_failure};
    use crate::apple::AppleBarcodeReader;
    use crate::error::BarcodeScanError;

    /// iOS 版 effect 本体。
    pub(crate) fn drive(state: &BarcodeScanState, sender: &CompletionSender<BarcodeScanState>) {
        if let Some(generation) = take_request(state) {
            let _ = sender.send(Box::new(mark_scanning));
            // 公開入口の進行中・未取得の要求は置き換えず、画面側を処理中の失敗で終える。
            let Some(id) = bridge::begin_screen() else {
                let failure = to_failure(BarcodeScanError::Internal("barcode_scan busy"));
                let _ = sender.send(Box::new(move |s: &mut BarcodeScanState| {
                    finish_scan(s, generation, Err(failure));
                }));
                return;
            };
            // iOS では通常 true（提示失敗も非同期に通知される）。false は提示依頼自体の同期的な失敗なので失敗で閉じる。
            let presented = present_ios_photo_picker(Box::new(move |result| {
                bridge::deliver_picked(id, result, &AppleBarcodeReader);
            }));
            if !presented {
                bridge::deliver(id, Err(BarcodeScanError::Internal("barcode_scan present")));
            }
            return;
        }
        if state.phase != BarcodeScanPhase::Scanning {
            return;
        }
        let generation = state.generation;
        poll_and_finish_owned(
            &bridge::SLOT,
            &bridge::SCREEN_OWNER,
            sender,
            |_| {},
            move |s: &mut BarcodeScanState, outcome| {
                // Idle へ戻っていれば反映せず slot に残す
                // （`false`。残った結果は公開入口の開始で画面の受け取り待ちへ移り、画面の次の開始で解放される）。
                // 別世代は反映せず畳む。
                if s.phase == BarcodeScanPhase::Idle {
                    return false;
                }
                if s.generation != generation {
                    return true;
                }
                finish_scan(s, generation, outcome.map_err(to_failure));
                true
            },
            // slot の要求が失われた（reset 等）。処理中のまま止めず失敗で確定する（`finish_scan` は世代違い・完了済みを無視する）。
            move |s: &mut BarcodeScanState| {
                let failure = to_failure(BarcodeScanError::Internal("barcode_scan lost"));
                finish_scan(s, generation, Err(failure));
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

    /// 公開入口の iOS 結果の検査を通す（`bridge::deliver_picked`）。キャンセルは `Ok(None)`、成功は検査・読み取りを通った値。
    fn ok_file() -> Result<Option<PickedFile>, PhotoPickerError> {
        picked(png_with_size(10, 10))
    }

    struct Reader(fn() -> Result<Vec<String>, BarcodeScanError>);
    impl BarcodeReader for Reader {
        fn read(&self, _p: &PickedPhoto) -> Result<Vec<String>, BarcodeScanError> {
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

    /// 起床を捨てるだけの waker（画面の poll を直接確かめるテスト用）。
    struct PollWaker;
    impl fandhe_runtime::RepaintWaker for PollWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    /// 画面の開始 → 結果の到着 → 公開入口の開始 → 画面の poll の順でも、画面は届いた結果をそのまま受け取り（lost にならない）、
    /// 公開入口もその後に自分の結果を受け取れる。
    #[test]
    fn public_begin_parks_unpolled_screen_result_for_screen() {
        let _g = lock();
        let screen = bridge::begin_screen().unwrap();
        assert!(bridge::deliver(screen, Ok(Some(vec!["a".to_string()]))));
        let public = bridge::begin_public().expect("画面の未取得の結果で詰まらない");
        assert_ne!(public, screen);
        // 受け取り待ちへ移した画面の id へ重ねて配送しても捨てられる。
        assert!(!bridge::deliver(screen, Ok(None)));
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = fandhe_runtime::CompletionSender::new(tx, std::sync::Arc::new(PollWaker));
        fandhe_runtime::request_slot::poll_and_finish_owned(
            &bridge::SLOT,
            &bridge::SCREEN_OWNER,
            &sender,
            |_| panic!("待機にはならない"),
            |s: &mut u8, o| {
                assert!(matches!(&o, Ok(Some(v)) if v == &["a"]));
                *s = 7;
                true
            },
            |_s: &mut u8| panic!("lost にならない"),
        );
        let mut state = 0u8;
        rx.try_recv().expect("画面の結果が送られる")(&mut state);
        assert_eq!(state, 7);
        assert_eq!(bridge::SCREEN_OWNER.current(), 0);
        assert!(bridge::SLOT.snapshot_of(screen).is_none());
        // 公開入口の結果は画面へ渡らず、公開入口が受け取る。
        assert!(bridge::deliver(public, Ok(None)));
        assert!(bridge::poll_public(public).is_some());
        bridge::reset();
    }

    /// 画面が受け取らないまま残った受け取り待ちの結果は、画面の次の開始で解放される。
    #[test]
    fn parked_screen_result_is_released_by_next_screen_begin() {
        let _g = lock();
        let screen = bridge::begin_screen().unwrap();
        assert!(bridge::deliver(screen, Ok(Some(vec!["a".to_string()]))));
        let public = bridge::begin_public().unwrap();
        assert!(bridge::deliver(public, Ok(None)));
        assert!(bridge::poll_public(public).is_some());
        let next = bridge::begin_screen().expect("公開入口が受け取った後は開始できる");
        assert!(bridge::SLOT.snapshot_of(screen).is_none());
        assert_eq!(bridge::SCREEN_OWNER.current(), next);
        bridge::SLOT.clear(next);
        bridge::SCREEN_OWNER.release(next);
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
            Some(Err(BarcodeScanError::Internal("photo_picker busy")))
        );
        let id = bridge::begin_public().unwrap();
        let big = picked(png_with_size(
            1,
            fandhe_device::photo_picker::MAX_PHOTO_SIDE + 1,
        ));
        bridge::deliver_picked(id, big, &reader);
        assert!(matches!(
            bridge::poll_public(id),
            Some(Err(BarcodeScanError::InvalidInput(_)))
        ));
        let id = bridge::begin_public().unwrap();
        let failing = Reader(|| Err(BarcodeScanError::Internal("secret-id")));
        bridge::deliver_picked(id, ok_file(), &failing);
        assert_eq!(
            bridge::poll_public(id),
            Some(Err(BarcodeScanError::Internal("secret-id")))
        );
        bridge::reset();
    }

    /// slot の要求が失われたとき（reset 等）は、1 度だけ `Internal("barcode_scan lost")` を返し、以後は `None`。
    #[test]
    fn public_request_lost_by_reset_fails_once() {
        let _g = lock();
        let id = bridge::begin_public().unwrap();
        assert!(bridge::poll_public(id).is_none());
        bridge::reset();
        assert_eq!(
            bridge::poll_public(id),
            Some(Err(BarcodeScanError::Internal("barcode_scan lost")))
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
            begin_barcode_scan(),
            Err(BarcodeScanError::Unsupported("barcode_scan"))
        );
        assert!(poll_barcode_scan(1).is_none());
        assert!(!discard_barcode_scan(1));
    }
}
