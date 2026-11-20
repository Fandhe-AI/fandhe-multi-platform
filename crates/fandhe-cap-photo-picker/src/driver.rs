//! Phase 11（親 #1101）型サンプル「写真選択」（Issue #1103）の effect ドライバ（機能 crate `fandhe-cap-photo-picker` の中核、
//! Issue #1355）。`fandhe_core::screens::photo_pick`（端末依存ゼロ）の要求を観測し、[`PhotoPicker`] で画像を 1 枚選ばせ、
//! **実際に復号できることを確かめてから**（契約の 2 段目、`fandhe_device::photo_picker` モジュール doc）画素を
//! `fandhe_runtime::image_store` へ置き、`CompletionSender` 経由で成功を返す（`fandhe-gallery` の `screens/registry.rs` の
//! `effect` 区画から呼ばれる契約、`fandhe_runtime::EffectFn` 参照）。復号そのものは `fandhe_device::photo_picker::decode_for_display`
//! （写真選択・書類スキャン・スキャナー機器が共有する 1 か所）に任せ、ここは形式名・サイズ・失敗の文言への写像だけを持つ。
//!
//! Web（wasm32）は本経路を通らず `fandhe-web-dom` の `file_picker.rs` が `createImageBitmap` で復号を確かめる
//! （gallery は wasm32 では本 crate を依存に入れない）。
//!
//! # スレッド
//!
//! macOS の `NSOpenPanel` は main スレッド必須のため、`file_pick` と同じく呼ばれたスレッド（egui の update ループ = main）で
//! `pick()` を同期に呼ぶ。固定タイムアウトは掛けない。
//!
//! # 対応 OS
//!
//! macOS / Windows: [`RfdPhotoPicker`](crate::RfdPhotoPicker)。Linux は `UnsupportedPhotoPicker` 固定（ダミーの成功実装へは倒さない）。
//!
//! iOS / Android（Issue #1104）は `PhotoPicker` トレイトを通らない。OS のピッカー（`PHPickerViewController` / `PickVisualMedia`）は
//! コールバック型のため、`biometric_auth` と同じ pull 型 `bridge`（`fandhe_runtime::request_slot::RequestSlot`）で結果を受ける。
//! 画面の状態型に依存しない入口は begin / poll / discard 型の非同期（[`begin_photo_pick`] / [`poll_photo_pick`] / [`discard_photo_pick`]）で、
//! 画面 effect と同じ slot を持ち主（`SlotOwner`）で分けて共有する（`consumer-app-structure.md` §8.2）。復号確認は OS コールバックのスレッド側（`bridge::deliver`）で
//! 済ませ、egui の UI スレッドでは復号しない。保留中の要求は Activity 再生成で **復元せず破棄**する（契約: `fandhe_device::photo_picker`
//! モジュール doc。`file_pick` の復元方式は採らない。`fandhe-gallery` の `android_main` が `reset_pending_tracking` を呼ぶ）。
//! OS 側が JPEG / PNG 以外（HEIC・GIF・WebP 等）を JPEG へ変換して渡し、変換できなければ `Internal` として失敗表示にする。
//! 形式の最終判定は OS の名乗る MIME ではなく `validate_photo` の先頭バイトで行う。
//!
//! # 復号の上限（A04 リソース枯渇）
//!
//! 入力は 8 MiB 上限（1 段目）。復号の辺・確保の上限と表示用の縮小は `decode_for_display` が掛け、超えたら `Internal`
//! （成功にしない）。デスクトップ・モバイルとも PNG / JPEG のみ復号する（GIF / WebP は `Internal`）。

use fandhe_core::screens::photo_pick::{
    PhotoPickState, ShownPhoto, finish_pick, mark_picking, take_request,
};
use fandhe_device::photo_picker::{
    ImageFormat, MAX_PHOTO_BYTES, PickedPhoto, decode_for_display as decode_pixels,
};
use fandhe_runtime::{CompletionSender, image_store};

#[cfg(any(target_os = "ios", target_os = "android"))]
use fandhe_core::screens::photo_pick::PickPhase;
#[cfg(any(test, target_os = "ios", target_os = "android"))]
use fandhe_device::file_picker::PickedFile;
#[cfg(any(test, target_os = "ios", target_os = "android"))]
use fandhe_device::photo_picker::validate_photo;

use crate::contract::PhotoPicker;
#[cfg(test)]
use crate::contract::UnsupportedPhotoPicker;
use crate::error::PhotoPickerError;

use std::sync::Arc;

/// 現在のプラットフォームに応じた同期の [`PhotoPicker`] を返す（プロセスで 1 個）。状態型に依存しない入口で、
/// 文字認識・バーコード読み取りの機能 crate もデスクトップ経路でこれを使う。macOS・Windows は OS のダイアログ
/// （[`RfdPhotoPicker`](crate::RfdPhotoPicker)）、iOS・Android は結果が非同期に届き同期の trait を通らないため `Unsupported`
/// 固定（成功を偽装しない。iOS・Android は [`begin_photo_pick`] / [`poll_photo_pick`] を使う）、それ以外（Linux）も `Unsupported` 固定。
pub fn platform_photo_picker() -> Arc<dyn PhotoPicker> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn PhotoPicker>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                Arc::new(crate::desktop::RfdPhotoPicker::new())
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                Arc::new(crate::contract::UnsupportedPhotoPicker)
            }
        })
        .clone()
}

/// 復号確認を通った画像。
pub(crate) struct Decoded {
    pub(crate) image: fandhe_core::RgbaPixels,
    pub(crate) format: &'static str,
    pub(crate) size: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

fn format_name(f: ImageFormat) -> &'static str {
    match f {
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Png => "PNG",
        ImageFormat::Gif => "GIF",
        ImageFormat::Webp => "WebP",
    }
}

/// 2 段目: 実際に復号できるかを確かめ、表示用の画素を作る。失敗は `Internal`。
/// ファイル名・パス・バイト列はエラーに含めない（A09）。
pub(crate) fn decode_for_display(photo: &PickedPhoto) -> Result<Decoded, PhotoPickerError> {
    let d = decode_pixels(&photo.bytes, photo.format)
        .ok_or(PhotoPickerError::Internal("photo_picker decode"))?;
    Ok(Decoded {
        image: d.image,
        format: format_name(photo.format),
        size: photo.bytes.len(),
        width: d.width,
        height: d.height,
    })
}

/// `PhotoPickerError` を画面の失敗文言へ写す（`Display` の内部識別子は出さない）。
fn failure_message(e: &PhotoPickerError) -> String {
    match e {
        PhotoPickerError::InvalidInput(_) => format!(
            "{} MiB を超える画像は選べません",
            MAX_PHOTO_BYTES / (1024 * 1024)
        ),
        PhotoPickerError::Unsupported(_) => "この環境では写真選択を使えません".to_string(),
        PhotoPickerError::Internal(s) if s.contains("decode") => {
            "画像を表示できませんでした".to_string()
        }
        // 内部識別子（`photo_picker format` 等）を利用者向け文言へ出さない。
        _ => "写真を選べませんでした".to_string(),
    }
}

/// 画素を `image_store` へ置く。置けなかった（今見えている（今のフレームで描かれた）画像だけで予算いっぱいのとき。`PutResult::RejectedOverBudget`）ときは、
/// 画像を見せられないので成功にせず失敗の文言にする（嘘の成功の禁止）。見えている画像が減っていれば、次に選び直したときに置ける。
/// モバイルで OS の選択画面を出している間は描画ループが止まり `frame` が進まないので、出す直前に見えていた画像が「今のフレーム」のまま残る。
fn store_shown(source: String, image: fandhe_core::RgbaPixels) -> Result<String, String> {
    match image_store::put(source.clone(), image) {
        image_store::PutResult::Stored => Ok(source),
        image_store::PutResult::RejectedOverBudget => Err("画像を表示できませんでした".to_string()),
    }
}

/// 選択 → 復号確認までを行い 3 値へ写す。キャンセルは `Ok(None)`。
#[cfg(any(test, not(any(target_os = "ios", target_os = "android"))))]
fn pick_and_decode(picker: &dyn PhotoPicker) -> Result<Option<Decoded>, String> {
    match picker.pick() {
        Ok(photo) => decode_for_display(&photo)
            .map(Some)
            .map_err(|e| failure_message(&e)),
        Err(PhotoPickerError::Cancelled(_)) => Ok(None),
        Err(e) => Err(failure_message(&e)),
    }
}

/// OS から届いた選択結果（iOS / Android）を 2 段検査に通して 3 値へ写す。
/// キャンセル・選択なしは `Ok(None)`（失敗にしない）、0 バイト・形式不明・上限超・復号不能・
/// 変換失敗は利用者向け文言の `Err`（内部識別子・パス・名前は出さない、A09）。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
fn mobile_outcome(
    result: Result<Option<PickedFile>, PhotoPickerError>,
) -> Result<Option<Decoded>, String> {
    match result {
        Ok(None) | Err(PhotoPickerError::Cancelled(_)) => Ok(None),
        Ok(Some(file)) => validate_photo(file.bytes)
            .map_err(PhotoPickerError::from)
            .and_then(|photo| decode_for_display(&photo))
            .map(Some)
            .map_err(|e| failure_message(&e)),
        Err(e) => Err(failure_message(&e)),
    }
}

/// iOS / Android の OS コールバックとの橋渡し（pull 型、`fandhe_runtime::request_slot::RequestSlot` の facade。モジュール doc 参照）。
///
/// 画面 effect と公開入口（[`begin_photo_pick`] / [`poll_photo_pick`]）が同じ slot を共有する。持ち主（[`bridge::SCREEN_OWNER`]・
/// [`bridge::PUBLIC_OWNER`]）で自分の分の要求を区別し、
/// 進行中の要求は busy で断り、公開側の未取得の結果は守る。画面側の届いた未取得の結果は、公開入口の開始が捨てずに画面の受け取り待ちへ移し、画面はそのまま受け取る（`SlotOwner::begin_parking`）。
/// 画面の結果は復号確認済みの画素、公開入口の結果は [`validate_photo`] 済みの [`PickedPhoto`]（同期の [`PhotoPicker`] と同じ。
/// 復号は保証しない）で、どちらを置くかは `deliver` が持ち主で決める。
///
/// テストでホストからも検証できるよう `cfg(test)` でもコンパイルする（OS 呼び出しは含まない）。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
pub(crate) mod bridge {
    use std::sync::{Arc, Mutex};

    use fandhe_device::file_picker::PickedFile;
    use fandhe_device::photo_picker::PickedPhoto;
    use fandhe_runtime::request_slot::{RequestSlot, SlotOwner};

    use super::Decoded;
    use crate::error::PhotoPickerError;

    /// 公開入口が追う自分の分の要求（画面側が置き換え・横取りしない）。
    pub(crate) static PUBLIC_OWNER: SlotOwner = SlotOwner::new();
    /// 画面 effect が追う自分の分の要求（公開入口と slot を共有するため区別する）。
    pub(crate) static SCREEN_OWNER: SlotOwner = SlotOwner::new();
    /// 公開側の poll・discard（持ち主の照合・取得・喪失判定・解放）を直列にする。並行 poll で成功と喪失が二重に返らないため。
    static PUBLIC_POLL_LOCK: Mutex<()> = Mutex::new(());

    /// 完了結果。毎フレームの `snapshot` クローンが画素・バイト列を複製しないよう `Arc`。
    #[derive(Clone)]
    pub(crate) enum Outcome {
        /// 画面向け: 復号確認済みの画素（`None` = キャンセル）。失敗は画面へ出す文言。
        Screen(Result<Option<Arc<Decoded>>, String>),
        /// 公開入口向け: 検証済みの写真。キャンセルは `Err(Cancelled)`（同期の [`PhotoPicker`](crate::PhotoPicker) と同じ）。
        Public(Result<Arc<PickedPhoto>, PhotoPickerError>),
    }

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 画面の要求を登録する。公開入口の進行中・未取得の要求があれば置き換えず `None`（busy）。
    pub(crate) fn begin_screen() -> Option<u64> {
        SCREEN_OWNER.begin_keeping_foreign(&SLOT)
    }

    /// 公開入口の要求を登録する。進行中の要求と自分の未取得の結果があれば `None`（busy）。画面側の届いた未取得の結果は捨てずに
    /// 画面の受け取り待ちへ移して始める（画面の effect が回らず取り出されないままでも詰まらず、生きている画面の結果も失わない）。
    /// 移した結果は画面の poll がそのまま受け取り、受け取られない分は画面の次の開始か Android の reset で解放される
    /// （残るのは最大 1 件で、結果の写真は上限 `MAX_PHOTO_BYTES`（8 MiB）まで）。
    pub(crate) fn begin_public() -> Option<u64> {
        PUBLIC_OWNER.begin_parking(&SLOT, &SCREEN_OWNER)
    }

    /// 公開入口の提示依頼が同期的に失敗したときに、登録を取り消す。
    pub(crate) fn abort_public(request_id: u64) {
        SLOT.clear(request_id);
        PUBLIC_OWNER.release(request_id);
    }

    /// OS からの結果を置く。呼び出しスレッドは OS コールバック側（iOS は NSItemProvider のキュー、Android はワーカースレッド）で、
    /// UI スレッドではない。画面の要求は 2 段検査（復号確認込み）まで、公開入口の要求は 1 段目（[`validate_photo`]）までを置く前に済ませる。
    /// `request_id` 不一致・未登録・重複は捨てて `false`（捨てられる結果でも検査は走る。1 要求 1 回なので許容）。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<Option<PickedFile>, PhotoPickerError>,
    ) -> bool {
        let outcome = if PUBLIC_OWNER.current() == request_id {
            Outcome::Public(super::public_outcome(result).map(Arc::new))
        } else {
            Outcome::Screen(super::mobile_outcome(result).map(|opt| opt.map(Arc::new)))
        };
        SLOT.deliver(request_id, outcome)
    }

    /// 公開入口の結果を取り出す（[`super::poll_photo_pick`] の本体）。届いていれば 1 度だけ `Some`、未着・旧 id・公開入口でない id は `None`。
    /// slot の要求が失われたとき（reset 等）は、1 度だけ `Err(Internal("photo_picker lost"))`、以後は `None`。
    /// 照合から解放までを直列にし、並行 poll でも成功と喪失が二重に返らない。
    pub(crate) fn poll_public(request_id: u64) -> Option<Result<PickedPhoto, PhotoPickerError>> {
        let _guard = PUBLIC_POLL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if request_id == 0 || PUBLIC_OWNER.current() != request_id {
            return None;
        }
        if let Some(outcome) = SLOT.take_ready(request_id) {
            PUBLIC_OWNER.release(request_id);
            return Some(match outcome {
                // 取り出した値が唯一の所有者のため、複製せず剥がす（共有が残っていれば複製へ倒す）。
                Outcome::Public(r) => {
                    r.map(|a| Arc::try_unwrap(a).unwrap_or_else(|a| (*a).clone()))
                }
                // 持ち主が公開入口の id に画面向けの結果は置かれない（`deliver` が持ち主で決める）。届いたら成功にせず失敗にする。
                Outcome::Screen(_) => Err(PhotoPickerError::Internal("photo_picker outcome")),
            });
        }
        if SLOT.snapshot().is_some_and(|(id, _)| id == request_id) {
            return None;
        }
        PUBLIC_OWNER.release(request_id);
        Some(Err(PhotoPickerError::Internal("photo_picker lost")))
    }

    /// 公開入口の自分の分の要求を捨てる（[`super::discard_photo_pick`] の本体）。持ち主が `request_id` と一致するときだけ
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

    /// Activity 再生成直後に `fandhe-gallery` の `android_main` から呼ぶ。保留中の要求を捨て、以後届く旧 `request_id` の
    /// [`deliver`] を不一致で無視させる（契約は「復元せず破棄」）。
    #[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// 公開入口向けに OS の結果を 1 段目（[`validate_photo`]）だけ通して 3 値へ写す（同期の [`PhotoPicker::pick`] と同じ形。
/// キャンセル・選択なしは `Err(Cancelled)`）。復号は確かめない。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
fn public_outcome(
    result: Result<Option<PickedFile>, PhotoPickerError>,
) -> Result<PickedPhoto, PhotoPickerError> {
    match result {
        Ok(None) => Err(PhotoPickerError::Cancelled("photo_picker")),
        Ok(Some(file)) => validate_photo(file.bytes).map_err(PhotoPickerError::from),
        Err(e) => Err(e),
    }
}

/// 写真選択を OS へ依頼し、結果を [`poll_photo_pick`] で受け取るための要求 id を返す（iOS・Android）。
///
/// 画面の状態型に依存しない非同期の入口で、フレームワークの外のアプリも使える。画面 effect と slot を共有する。
///
/// - 進行中の要求は busy で断る。
/// - 画面側の届いた未取得の結果があっても断らない（画面が描かれず取り出されないままでも詰まらない）。結果は捨てずに画面の受け取り待ちへ移し、
///   画面はそのまま受け取る（結果が届いてから次フレームの poll までの間に始めても、画面の結果は失われない）。
///   受け取られない分は画面の次の開始か Android の reset で解放される
///   （残るのは最大 1 件で、結果の写真は上限 `MAX_PHOTO_BYTES`（8 MiB）まで）。
/// - 自分の未取得の結果があれば `Internal("photo_picker busy")` で断る。
/// - OS 側のピッカーが進行中のとき
///   （[`discard_photo_pick`] で捨てた後を含む）も、OS の観測（Android は追跡中の要求、iOS は提示中のピッカー）で busy になる。
///   依頼自体の失敗は `Err`（結果は届かない）。
///
/// 結果の解放: 届く結果は [`validate_photo`] 済みのバイト列（上限 `MAX_PHOTO_BYTES`）だけで、fd・一時パスは届く前に解放済み。
/// 取り出さない要求が slot に残ると次の要求も画面 effect の開始も busy で断られるため、不要な要求は [`discard_photo_pick`] で捨てる。
///
/// 既知の天井（iOS）: シーンが切断されたときは `.m` が `UISceneDidDisconnectNotification` の観測で失敗（code 2）を 1 度だけ届けて確定する（切断でピッカーが completion を呼ばずに消える挙動は公式文書で未確認・実機でも未確認）。
/// 残るのは、UIKit が提示を捨てて completion も来ず、シーンも接続されたままの場合と、提示先のシーンが特定できず切断で確定しない場合（別シーンの切断で正しい選択を落とさないため）で、要求は確定せず slot に残る（これを観測する OS の通知は無い）。
/// 画面 effect は busy で断られ続け、公開入口は [`discard_photo_pick`] で捨てるまで確定しない（捨てたあとの次の begin が `.m` の残りを失敗で回収する）。
/// 最前面が遷移中なら提示せず busy にするため、捨てられること自体はほぼ起きない前提。推測のタイマーは持たない。
///
/// デスクトップ（macOS・Windows）は同期の [`platform_photo_picker`] を使うため、この入口は持たず `Unsupported`。Linux 等も `Unsupported`。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn begin_photo_pick() -> Result<u64, PhotoPickerError> {
    let request_id =
        bridge::begin_public().ok_or(PhotoPickerError::Internal("photo_picker busy"))?;
    if !present(request_id) {
        bridge::abort_public(request_id);
        return Err(PhotoPickerError::Internal("photo_picker present"));
    }
    Ok(request_id)
}

/// iOS・Android 以外では `Unsupported`（[`platform_photo_picker`] 参照）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn begin_photo_pick() -> Result<u64, PhotoPickerError> {
    Err(PhotoPickerError::Unsupported("photo_picker"))
}

/// [`begin_photo_pick`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・公開入口でない id・取り出し済みは `None`。
///
/// 結果は 3 値: 選択 `Ok(PickedPhoto)`（[`validate_photo`] 済み。復号できることは保証しない）、キャンセル `Err(Cancelled)`
/// （失敗として見せない）、失敗 `Err`（上限超過は `InvalidInput`、形式不明・変換失敗・提示失敗・busy は `Internal`）。
/// slot の要求が失われたとき（reset 等）は、1 度だけ `Err(Internal("photo_picker lost"))` を返し、以後は `None`。
/// 並行して同じ id を poll しても、結果と喪失が二重に返ることはない。iOS・Android 以外では常に `None`。
pub fn poll_photo_pick(request_id: u64) -> Option<Result<PickedPhoto, PhotoPickerError>> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        bridge::poll_public(request_id)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = request_id;
        None
    }
}

/// [`begin_photo_pick`] の自分の要求を捨てて slot を空ける。`request_id` が公開入口の現在の要求のときだけ `true`
/// （他人の id・画面側の id・取り出し済み・0 は `false` で何もしない）。結果が未取得でも進行中でも捨てる。
///
/// discard は OS のピッカーを閉じない。ピッカーが閉じるまで次の [`begin_photo_pick`]（画面 effect の開始も）は OS 側が busy で断る。
/// 後から届いた結果は id 不一致で捨てられ、新しい要求へは結びつかない。iOS・Android 以外では常に `false`。
pub fn discard_photo_pick(request_id: u64) -> bool {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        bridge::discard_public(request_id)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = request_id;
        false
    }
}

/// Activity 再生成直後に `fandhe-gallery` の `android_main` が呼ぶ。保留中の写真選択の要求を捨てる（契約は「復元せず破棄」。
/// `bridge::SLOT` がプロセス static のため再生成をまたいで残るのを、新しい `Idle` の画面状態を作る前に断つ）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
}

/// 提示を依頼する。結果は非同期に `bridge::deliver` へ届く。`false` は提示依頼自体の同期的な失敗（コールバックは二度と来ない）。
#[cfg(target_os = "ios")]
fn present(request_id: u64) -> bool {
    crate::ios::present_ios_photo_picker(Box::new(move |result| {
        bridge::deliver(request_id, result);
    }))
}

#[cfg(target_os = "android")]
fn present(request_id: u64) -> bool {
    crate::android::request_photo_pick(request_id)
}

/// iOS / Android 版 effect 本体（毎フレーム・UI スレッドから呼ばれる。待機でブロックしない）。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn drive_mobile(state: &PhotoPickState, sender: &CompletionSender<PhotoPickState>) {
    if let Some(generation) = take_request(state) {
        // 前回の画素を先に解放する（失敗・キャンセルの横に前の画像を残さない）。
        image_store::remove_prefix("fandhe.photo:");
        let _ = sender.send(Box::new(mark_picking));
        // 公開入口の進行中・未取得の要求は置き換えず、画面側を処理中の失敗で終える。
        let Some(request_id) = bridge::begin_screen() else {
            let message = failure_message(&PhotoPickerError::Internal("photo_picker busy"));
            let _ = sender.send(Box::new(move |s: &mut PhotoPickState| {
                finish_pick(s, generation, Err(message));
            }));
            return;
        };
        if !present(request_id) {
            // 提示依頼自体が同期的に失敗した（コールバックは二度と来ない）。
            bridge::deliver(
                request_id,
                Err(PhotoPickerError::Internal("photo_picker present")),
            );
        }
        return;
    }
    if state.phase != PickPhase::Picking {
        return;
    }
    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &bridge::SCREEN_OWNER,
        sender,
        |_| {},
        move |s: &mut PhotoPickState, outcome| {
            // 既に `Idle` へ戻っていれば反映せず slot に残す
            // （`false`。残った結果は公開入口の開始で画面の受け取り待ちへ移り、画面の次の開始で解放される）。
            // 別世代は反映せず畳む。
            if s.phase == PickPhase::Idle {
                return false;
            }
            if s.generation != generation {
                return true;
            }
            // 画面の持ち主の id に公開入口向けの結果は置かれない。届いたら成功にせず失敗にする。
            let result = match outcome {
                bridge::Outcome::Screen(r) => r,
                bridge::Outcome::Public(_) => Err(failure_message(&PhotoPickerError::Internal(
                    "photo_picker outcome",
                ))),
            };
            let result = result.and_then(|opt| {
                opt.map(|d| {
                    // 画素を先に置いてから成功を届ける（届いた時点で描ける）。
                    // ponytail: slot が参照を持つため表示用複製（<= 1024px、約 4 MiB）を 1 回 clone する。
                    // 気になれば `Arc<ColorImage>` を `image_store` が受けるようにする。
                    let source = format!("fandhe.photo:{generation}");
                    store_shown(source, d.image.clone()).map(|source| ShownPhoto {
                        source,
                        format: d.format.to_string(),
                        size: d.size,
                        width: d.width,
                        height: d.height,
                    })
                })
                .transpose()
            });
            finish_pick(s, generation, result);
            true
        },
        // slot の要求が失われた（再生成の reset 等）。処理中のまま止めず失敗で確定する（`finish_pick` は世代違い・完了済みを無視する）。
        move |s: &mut PhotoPickState| {
            let message = failure_message(&PhotoPickerError::Internal("photo_picker lost"));
            finish_pick(s, generation, Err(message));
        },
    );
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
#[cfg(any(test, not(any(target_os = "ios", target_os = "android"))))]
fn drive(
    state: &PhotoPickState,
    sender: &CompletionSender<PhotoPickState>,
    picker: &dyn PhotoPicker,
) {
    let Some(generation) = take_request(state) else {
        return;
    };
    // 前回の画素を先に解放する（失敗・キャンセルの横に前の画像を残さない）。
    image_store::remove_prefix("fandhe.photo:");
    let _ = sender.send(Box::new(mark_picking));

    let result = pick_and_decode(picker).and_then(|opt| {
        opt.map(|d| {
            // 画素を先に置いてから成功を届ける（届いた時点で描ける）。
            let source = format!("fandhe.photo:{generation}");
            store_shown(source, d.image).map(|source| ShownPhoto {
                source,
                format: d.format.to_string(),
                size: d.size,
                width: d.width,
                height: d.height,
            })
        })
        .transpose()
    });
    let _ = sender.send(Box::new(move |s: &mut PhotoPickState| {
        finish_pick(s, generation, result);
    }));
}

/// `registry.rs` の `effect` 区画から `photo_pick` 画面用に配線される effect ドライバ。
pub fn photo_pick_effect(state: &PhotoPickState, sender: &CompletionSender<PhotoPickState>) {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    drive_mobile(state, sender);
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    drive(state, sender, platform_photo_picker().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::photo_picker::MAX_PHOTO_SIDE;
    use std::io::Cursor;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn photo(bytes: Vec<u8>) -> PickedPhoto {
        validate_photo(bytes).unwrap()
    }

    #[test]
    fn valid_png_decodes_and_reports_format_and_size() {
        let bytes = png(8, 4);
        let len = bytes.len();
        let d = decode_for_display(&photo(bytes)).unwrap();
        assert_eq!((d.width, d.height), (8, 4));
        assert_eq!((d.image.width(), d.image.height()), (8, 4));
        assert_eq!(d.format, "PNG");
        assert_eq!(d.size, len);
    }

    #[test]
    fn undecodable_inputs_fail_stage_two() {
        // 途中で切れた PNG・ヘッダーだけの JPEG・GIF・WebP・辺の上限超え。
        let mut cut = png(64, 64);
        cut.truncate(cut.len() / 2);
        for bytes in [
            cut,
            vec![0xFF, 0xD8, 0xFF],
            b"GIF89a....".to_vec(),
            b"RIFF\0\0\0\0WEBPVP8 ".to_vec(),
            png(MAX_PHOTO_SIDE + 1, 1),
        ] {
            assert!(matches!(
                decode_for_display(&photo(bytes)),
                Err(PhotoPickerError::Internal(_))
            ));
        }
    }

    struct Fake(fn() -> Result<PickedPhoto, PhotoPickerError>);
    impl PhotoPicker for Fake {
        fn pick(&self) -> Result<PickedPhoto, PhotoPickerError> {
            (self.0)()
        }
    }

    fn file(bytes: Vec<u8>) -> PickedFile {
        PickedFile {
            name: "photo".to_string(),
            bytes,
        }
    }

    #[test]
    fn mobile_outcome_maps_cancel_success_and_failures() {
        assert!(matches!(mobile_outcome(Ok(None)), Ok(None)));
        let cancel = mobile_outcome(Err(PhotoPickerError::Cancelled("photo_picker")));
        assert!(matches!(cancel, Ok(None)));
        let ok = mobile_outcome(Ok(Some(file(png(8, 4))))).unwrap().unwrap();
        assert_eq!((ok.width, ok.height), (8, 4));
        // 0 バイト・形式不明は成功にしない。内部識別子も文言へ出さない。
        for bytes in [vec![], b"not an image at all".to_vec()] {
            let e = mobile_outcome(Ok(Some(file(bytes)))).err().unwrap();
            assert_eq!(e, "写真を選べませんでした");
        }
        let e = mobile_outcome(Ok(Some(file(vec![0xFF, 0xD8, 0xFF]))))
            .err()
            .unwrap();
        assert_eq!(e, "画像を表示できませんでした");
        let e = mobile_outcome(Ok(Some(file(vec![0; MAX_PHOTO_BYTES + 1]))))
            .err()
            .unwrap();
        assert!(e.contains("8 MiB"), "{e}");
        // OS 側の変換失敗（code 2）は失敗として見せる。
        let e = mobile_outcome(Err(PhotoPickerError::Internal("photo_picker pick")))
            .err()
            .unwrap();
        assert_eq!(e, "写真を選べませんでした");
    }

    #[test]
    fn outcome_mapping_distinguishes_cancel_and_failures() {
        let cancel = pick_and_decode(&Fake(|| Err(PhotoPickerError::Cancelled("photo_picker"))));
        assert!(matches!(cancel, Ok(None)));
        let unsupported = pick_and_decode(&UnsupportedPhotoPicker).err().unwrap();
        assert_eq!(unsupported, "この環境では写真選択を使えません");
        let big = pick_and_decode(&Fake(|| {
            Err(PhotoPickerError::InvalidInput("photo_picker size"))
        }))
        .err()
        .unwrap();
        assert!(big.contains("8 MiB"), "{big}");
        let undecodable = pick_and_decode(&Fake(|| Ok(photo(vec![0xFF, 0xD8, 0xFF]))))
            .err()
            .unwrap();
        assert_eq!(undecodable, "画像を表示できませんでした");
        // 内部識別子（`photo_picker format` 等）は利用者向け文言へ漏らさない。
        let internal = pick_and_decode(&Fake(|| {
            Err(PhotoPickerError::Internal("photo_picker format"))
        }))
        .err()
        .unwrap();
        assert_eq!(internal, "写真を選べませんでした");
    }

    #[test]
    fn drive_without_request_is_noop_and_request_completes_with_picked_image() {
        use fandhe_core::screens::photo_pick::request_pick;
        use fandhe_runtime::{Completion, RepaintWaker};
        /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
        struct NoopWaker;
        impl RepaintWaker for NoopWaker {
            fn wake(&self) {}
            fn wake_after(&self, _delay: std::time::Duration) {}
        }
        let (tx, rx) = std::sync::mpsc::channel::<Completion<PhotoPickState>>();
        let sender = CompletionSender::new(tx, Arc::new(NoopWaker));
        let mut state = PhotoPickState::default();
        drive(
            &state,
            &sender,
            &Fake(|| Err(PhotoPickerError::Unsupported("x"))),
        );
        assert!(rx.try_recv().is_err());

        request_pick(&mut state);
        drive(&state, &sender, &Fake(|| Ok(photo(png(8, 4)))));
        for _ in 0..2 {
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("完了が届く")(&mut state);
        }
        let shown = state.shown.as_ref().expect("成功が反映される");
        assert_eq!((shown.width, shown.height), (8, 4));
        image_store::clear();
    }

    /// `bridge` の static（slot・持ち主）を触るテストを直列にする。
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        assert!(bridge::deliver(screen, Ok(Some(file(png(8, 4))))));
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
                assert!(matches!(&o, bridge::Outcome::Screen(Ok(Some(_)))));
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
        assert!(bridge::deliver(public, Ok(Some(file(png(8, 4))))));
        assert!(bridge::poll_public(public).is_some());
        bridge::reset();
    }

    /// 画面が受け取らないまま残った受け取り待ちの結果は、画面の次の開始で解放される。
    #[test]
    fn parked_screen_result_is_released_by_next_screen_begin() {
        let _g = lock();
        let screen = bridge::begin_screen().unwrap();
        assert!(bridge::deliver(screen, Ok(Some(file(png(8, 4))))));
        let public = bridge::begin_public().unwrap();
        assert!(bridge::deliver(public, Ok(Some(file(png(8, 4))))));
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
        assert!(bridge::deliver(public, Ok(Some(file(png(8, 4))))));
        assert!(bridge::begin_public().is_none());
        assert!(bridge::begin_screen().is_none());
        assert!(bridge::poll_public(public).is_some());
        assert!(bridge::poll_public(public).is_none());
        bridge::reset();
    }

    /// 公開入口の要求は画面と排他で、進行中も結果の未取得も画面側が落とさない。画面の進行中の要求も公開入口が置き換えない。
    #[test]
    fn public_request_is_exclusive_with_screen_and_polled_once() {
        let _g = lock();
        let public = bridge::begin_public().unwrap();
        assert!(bridge::begin_public().is_none());
        assert!(bridge::begin_screen().is_none());
        assert!(bridge::poll_public(public).is_none());
        assert!(bridge::deliver(public, Ok(Some(file(png(8, 4))))));
        // 未取得の公開入口の結果を画面側が落とさない。
        assert!(bridge::begin_screen().is_none());
        // 公開入口の結果は検証済みの写真で、1 度だけ取れる。
        let photo = bridge::poll_public(public).unwrap().unwrap();
        assert_eq!(photo.format, ImageFormat::Png);
        assert!(bridge::poll_public(public).is_none());
        // 取り出したあとは画面側が使え、画面の進行中は公開入口が断られる。
        let screen = bridge::begin_screen().unwrap();
        assert!(bridge::begin_public().is_none());
        assert!(!bridge::discard_public(screen));
        assert!(bridge::poll_public(screen).is_none());
        bridge::reset();
    }

    /// 公開入口はキャンセルを `Err(Cancelled)`、失敗を元の種別のまま返し、画面向けの復号を要しない。
    #[test]
    fn public_outcome_keeps_three_values() {
        let _g = lock();
        let id = bridge::begin_public().unwrap();
        bridge::deliver(id, Ok(None));
        assert_eq!(
            bridge::poll_public(id),
            Some(Err(PhotoPickerError::Cancelled("photo_picker")))
        );
        let id = bridge::begin_public().unwrap();
        bridge::deliver(id, Err(PhotoPickerError::Internal("photo_picker busy")));
        assert_eq!(
            bridge::poll_public(id),
            Some(Err(PhotoPickerError::Internal("photo_picker busy")))
        );
        // 復号できない（途中で切れた）画像でも 1 段目は通る。0 バイトは通らない。
        let id = bridge::begin_public().unwrap();
        bridge::deliver(id, Ok(Some(file(vec![0xFF, 0xD8, 0xFF]))));
        assert!(bridge::poll_public(id).unwrap().is_ok());
        let id = bridge::begin_public().unwrap();
        bridge::deliver(id, Ok(Some(file(vec![]))));
        assert!(bridge::poll_public(id).unwrap().is_err());
        bridge::reset();
    }

    /// 画面の結果は復号確認済みで、公開入口の要求は画面の持ち主の目に見えない。
    #[test]
    fn screen_result_is_decoded_and_public_is_hidden_from_screen() {
        let _g = lock();
        let screen = bridge::begin_screen().unwrap();
        assert!(bridge::deliver(screen, Ok(Some(file(png(8, 4))))));
        match bridge::SLOT.snapshot() {
            Some((id, Some(bridge::Outcome::Screen(Ok(Some(d)))))) => {
                assert_eq!((id, d.width, d.height), (screen, 8, 4));
            }
            _ => panic!("画面の結果は復号済みで届く"),
        }
        bridge::SLOT.clear(screen);
        bridge::SCREEN_OWNER.release(screen);
        let public = bridge::begin_public().unwrap();
        assert_ne!(bridge::SCREEN_OWNER.current(), public);
        bridge::reset();
    }

    /// slot の要求が失われたとき（reset 等）は、1 度だけ `Internal("photo_picker lost")` を返し、以後は `None`。
    #[test]
    fn public_request_lost_by_reset_fails_once() {
        let _g = lock();
        let id = bridge::begin_public().unwrap();
        assert!(bridge::poll_public(id).is_none());
        bridge::reset();
        assert_eq!(
            bridge::poll_public(id),
            Some(Err(PhotoPickerError::Internal("photo_picker lost")))
        );
        assert!(bridge::poll_public(id).is_none());
        assert!(bridge::begin_public().is_some());
        bridge::reset();
    }

    #[test]
    fn concurrent_poll_delivers_once() {
        let _g = lock();
        for _ in 0..200 {
            bridge::reset();
            let id = bridge::begin_public().unwrap();
            assert!(bridge::deliver(id, Ok(Some(file(png(4, 4))))));
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
        assert!(!bridge::deliver(id, Ok(Some(file(png(8, 4))))));
        assert!(bridge::poll_public(next).is_none());
        // 結果が未取得でも捨てられ、遅れて届いた結果は捨てられる。
        assert!(bridge::deliver(next, Ok(None)));
        assert!(bridge::discard_public(next));
        assert!(!bridge::deliver(next, Ok(None)));
        assert!(bridge::poll_public(next).is_none());
        // 画面の id は公開入口から捨てられない。
        let screen = bridge::begin_screen().unwrap();
        assert!(!bridge::discard_public(screen));
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

    /// iOS・Android 以外の公開入口は `Unsupported`（デスクトップは同期の `platform_photo_picker` を使う）。
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    #[test]
    fn async_entry_is_unsupported_off_mobile() {
        assert_eq!(
            begin_photo_pick(),
            Err(PhotoPickerError::Unsupported("photo_picker"))
        );
        assert!(poll_photo_pick(1).is_none());
        assert!(!discard_photo_pick(1));
    }
}
