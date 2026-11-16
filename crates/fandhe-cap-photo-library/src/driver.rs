//! Phase 11（親 #1121）型サンプル「写真の保存」（Issue #1254）の effect ドライバ。
//! `fandhe_core::screens::photo_save`（端末依存ゼロ）の要求を観測し、アプリが生成した PNG
//! （[`sample_png`]）を `fandhe_device::photo_library::PngImage` で検査してから OS へ渡し、
//! 結果を `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、
//! `fandhe-cap-calendar` の driver と同型）。契約と守る範囲は `crate::contract` の
//! モジュール doc を参照。
//!
//! - macOS（#1254）/ iOS（#1255）/ Android（#1256）: コールバック型のため pull 型 [`bridge`]
//!   （`RequestSlot` の facade）で毎フレーム [`drive_callback`] が結果を反映する。
//!   Android は API 28 以下のみ `Unsupported` を返す
//! - Windows（#1254）/ それ以外: [`PhotoLibrary`] を同期に呼ぶ。Windows は Pictures フォルダへ
//!   書き、Linux は `UnsupportedPhotoLibrary` で「非対応」を表示する

use crate::contract::{PhotoLibrary, PngImage, SavedTo};
use crate::error::PhotoLibraryError;
use fandhe_core::screens::photo_save::{
    PhotoSaveState, SaveOutcome, SavedPlace, finish_save, mark_saving, take_request,
};

use fandhe_runtime::CompletionSender;

/// 保存する見本画像（256x256 のグラデーション PNG）。
pub(crate) fn sample_png() -> Vec<u8> {
    let img = image::RgbaImage::from_fn(256, 256, |x, y| image::Rgba([x as u8, y as u8, 160, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    // メモリ上への PNG 書き出しは失敗しない。万一失敗しても空バイト列は
    // `PngImage::new` が入力不正として弾くため、成功は作られない。
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

/// `PhotoLibraryError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする
/// （security.md A09）。
fn to_outcome(result: Result<SavedTo, PhotoLibraryError>) -> Result<SaveOutcome, String> {
    match result {
        Ok(SavedTo::PhotoLibrary) => Ok(SaveOutcome::Saved(SavedPlace::PhotoLibrary)),
        Ok(SavedTo::MediaStore) => Ok(SaveOutcome::Saved(SavedPlace::Gallery)),
        Ok(SavedTo::PicturesFolder) => Ok(SaveOutcome::Saved(SavedPlace::PicturesFolder)),
        Err(PhotoLibraryError::PermissionDenied(_)) => Ok(SaveOutcome::Denied),
        Err(PhotoLibraryError::Unsupported(_)) => Ok(SaveOutcome::Unsupported),
        Err(PhotoLibraryError::InvalidInput(_)) => Err("画像が不正です".to_string()),
        Err(_) => Err("OS の保存処理に失敗しました".to_string()),
    }
}

/// `.m` 側の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 0 | 保存確定 | `Ok(PhotoLibrary)` |
/// | 1 | 拒否・制限・未決定のまま・入力の受け渡し不正 | `PermissionDenied` |
/// | 2 | macOS 11 / iOS 14 未満 | `Unsupported` |
/// | 3 | `performChanges` の失敗 | `Internal("photo_library save")` |
/// | 4 | 認可要求の異常 | `Internal("photo_library access")` |
/// | 他 | 未知 | `Internal("photo_library outcome")` |
///
/// ホストでもテストできるよう cfg は割かない。呼び出し元は macOS / iOS。
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(crate) fn outcome_from_apple_code(code: i32) -> Result<SavedTo, PhotoLibraryError> {
    match code {
        0 => Ok(SavedTo::PhotoLibrary),
        1 => Err(PhotoLibraryError::PermissionDenied("photo_library")),
        2 => Err(PhotoLibraryError::Unsupported("photo_library")),
        3 => Err(PhotoLibraryError::Internal("photo_library save")),
        4 => Err(PhotoLibraryError::Internal("photo_library access")),
        _ => Err(PhotoLibraryError::Internal("photo_library outcome")),
    }
}

/// OS 側コールバックとの橋渡し（macOS / iOS / Android、pull 型、`RequestSlot` の facade）。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use crate::contract::SavedTo;
    use crate::error::PhotoLibraryError;

    use fandhe_runtime::request_slot::RequestSlot;

    /// OS の結果をそのまま保持する（画面用の写像は `drive_callback` が行う。画面非依存の公開入口と共有する）。
    pub(crate) type Outcome = Result<SavedTo, PhotoLibraryError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(request_id: u64, result: Result<SavedTo, PhotoLibraryError>) -> bool {
        SLOT.deliver(request_id, result)
    }

    /// Android: Activity 再生成直後に `android_main` から呼ぶ。[`SLOT`] はプロセス static の
    /// ため再生成をまたいで残る。保留中の要求は復元せず破棄し、以後届く旧 `request_id` の
    /// [`deliver`] を無視させる（`calendar::bridge::reset` と同じ位置づけ）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
}

/// 画面の状態型に依存しない入口。Windows は Pictures フォルダへ同期に書く実装、それ以外は `Unsupported` 固定。
/// iOS・macOS・Android は OS がコールバック型で結果が非同期に届くため、同期の [`PhotoLibrary`] 実装は提供せず
/// （成功の偽装を避けるため「未対応」を返す）、画面は [`photo_save_effect`] の経路を使う。
// ponytail: コールバック型 OS の同期 trait 実装は要求が出たら足す（`bridge` の結果を待つワーカーが要る）。
pub fn platform_photo_library() -> Box<dyn PhotoLibrary> {
    #[cfg(target_os = "windows")]
    return Box::new(crate::windows::WindowsPhotoLibrary);
    #[cfg(not(target_os = "windows"))]
    return Box::new(crate::contract::UnsupportedPhotoLibrary);
}

/// 任意の検査済み PNG の保存を OS へ依頼し、結果を [`poll_photo_save`] で受け取るための要求 id を返す
/// （macOS・iOS・Android）。
///
/// 画面の状態型に依存しない非同期の入口で、フレームワークの外のアプリも使える。1 プロセスに 1 要求で、
/// 進行中、または結果が未取得の要求があるうちは `Internal`（busy）で断る。依頼自体の失敗は `Err`（結果は届かない）。
/// Windows は同期の [`platform_photo_library`] を使う。Linux は `Unsupported`。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
pub fn begin_photo_save(image: &PngImage) -> Result<u64, PhotoLibraryError> {
    let request_id = PUBLIC_OWNER
        .begin_excluding(&bridge::SLOT, &[&SCREEN_OWNER, &PUBLIC_OWNER])
        .ok_or(PhotoLibraryError::Internal("photo_library busy"))?;
    if let Err(e) = begin_save(request_id, image) {
        bridge::SLOT.clear(request_id);
        PUBLIC_OWNER.release(request_id);
        return Err(e);
    }
    Ok(request_id)
}

/// OS コールバック型の実装がない OS では成功へ倒さず `Unsupported`。
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
pub fn begin_photo_save(image: &PngImage) -> Result<u64, PhotoLibraryError> {
    let _ = image;
    Err(PhotoLibraryError::Unsupported("photo_library"))
}

/// [`begin_photo_save`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・取り出し済みは `None`。
pub fn poll_photo_save(request_id: u64) -> Option<Result<SavedTo, PhotoLibraryError>> {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    {
        // 照合・取得・消去を同じロックで行う（並行 poll でも Some は 1 件だけ）。
        let result = bridge::SLOT.take_ready(request_id)?;
        PUBLIC_OWNER.release(request_id);
        Some(result)
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    {
        let _ = request_id;
        None
    }
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
/// 公開入口 [`begin_photo_save`] が追う自分の分の要求（画面側が置き換え・横取りしない）。
static PUBLIC_OWNER: fandhe_runtime::request_slot::SlotOwner =
    fandhe_runtime::request_slot::SlotOwner::new();

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
/// 画面 effect が追う自分の分の要求（公開入口と slot を共有するため区別する）。
static SCREEN_OWNER: fandhe_runtime::request_slot::SlotOwner =
    fandhe_runtime::request_slot::SlotOwner::new();

/// OS へ保存を依頼する。`Ok` は受理済みで結果は後から [`bridge::deliver`] で届く。
/// `Err` は依頼自体の失敗（呼び出し元がその場で `deliver` する）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn begin_save(request_id: u64, image: &PngImage) -> Result<(), PhotoLibraryError> {
    crate::apple::begin_save(request_id, image)
}

#[cfg(target_os = "android")]
fn begin_save(request_id: u64, image: &PngImage) -> Result<(), PhotoLibraryError> {
    crate::android::begin_save(request_id, image)
}

/// macOS / iOS / Android 版の effect ドライバ本体。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
fn drive_callback(state: &PhotoSaveState, sender: &CompletionSender<PhotoSaveState>) {
    use fandhe_core::screens::photo_save::SavePhase;

    if let Some(request) = take_request(state) {
        let _ = sender.send(Box::new(mark_saving));
        // 進行中の保存（公開入口・画面自身とも）は置き換えず、画面には失敗として返す。保存は OS 側で取り消せず、
        // 置き換えると両方が走って写真が重複するため（lan-discovery・map-view は OS 側で置き換わり、捨てた結果に
        // 副作用が残らないので置き換え型）。画面自身の完了済み・未取得の結果は置き換える。
        let Some(request_id) = SCREEN_OWNER.begin_keeping_foreign(&bridge::SLOT) else {
            let generation = request.generation;
            let _ = sender.send(Box::new(move |s: &mut PhotoSaveState| {
                finish_save(
                    s,
                    generation,
                    Err("他の保存が処理中です。完了後にもう一度お試しください".to_string()),
                );
            }));
            return;
        };
        // 検査を通らない画像は OS へ渡さずその場で画面へ返す。
        let accepted = PngImage::new(sample_png())
            .map_err(PhotoLibraryError::from)
            .and_then(|png| begin_save(request_id, &png));
        if let Err(e) = accepted {
            bridge::deliver(request_id, Err(e));
        }
        return;
    }

    if state.phase != SavePhase::Saving {
        return;
    }
    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &SCREEN_OWNER,
        sender,
        |_request_id| {},
        move |s: &mut PhotoSaveState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != SavePhase::Idle {
                finish_save(s, generation, to_outcome(outcome));
                true
            } else {
                false
            }
        },
        move |s: &mut PhotoSaveState| {
            finish_save(
                s,
                generation,
                Err("保存が中断されました。もう一度お試しください".to_string()),
            );
        },
    );
}

/// `registry.rs` から `photo_save` 画面用に配線される effect ドライバ（macOS / iOS / Android）。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
pub fn photo_save_effect(state: &PhotoSaveState, sender: &CompletionSender<PhotoSaveState>) {
    drive_callback(state, sender);
}

/// 同期経路（Windows・Linux）: 検査してから [`PhotoLibrary`] を呼ぶ。
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
fn drive(
    state: &PhotoSaveState,
    sender: &CompletionSender<PhotoSaveState>,
    lib: &dyn crate::contract::PhotoLibrary,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_saving));
    let result = PngImage::new(sample_png())
        .map_err(PhotoLibraryError::from)
        .and_then(|png| lib.save_png(&png));
    let outcome = to_outcome(result);
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut PhotoSaveState| {
        finish_save(s, generation, outcome);
    }));
}

/// `registry.rs` から配線される effect ドライバ（Windows・Linux）。
///
/// ponytail: Windows は小さな PNG 1 枚の書き込みなので UI スレッドで同期に書く。Pictures が
/// ネットワーク先へリダイレクトされて遅いと分かったら、作業スレッド + bridge へ移す。
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
pub fn photo_save_effect(state: &PhotoSaveState, sender: &CompletionSender<PhotoSaveState>) {
    #[cfg(target_os = "windows")]
    drive(state, sender, &crate::windows::WindowsPhotoLibrary);
    #[cfg(not(target_os = "windows"))]
    drive(state, sender, &crate::contract::UnsupportedPhotoLibrary);
}

/// `bridge::SLOT` と持ち主の印は static なので、それを触るテストどうしを直列化する（並行に走ると判定が揺れる）。
#[cfg(all(
    test,
    any(target_os = "macos", target_os = "ios", target_os = "android")
))]
static SLOT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    #[allow(dead_code)] // 非対応 OS の同期経路のテストだけが使う
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    /// 画面の保存は自分の進行中の保存を置き換えず、自分の未取得の結果は置き換え、公開入口の結果は守る。
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    #[test]
    fn screen_save_refuses_in_flight_replaces_own_ready_and_keeps_public() {
        let _g = SLOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 自分の保存が進行中なら断る（取り消せない保存を二重に始めない）。
        let first = SCREEN_OWNER.begin_keeping_foreign(&bridge::SLOT).unwrap();
        assert!(SCREEN_OWNER.begin_keeping_foreign(&bridge::SLOT).is_none());
        // 自分の古い未取得の結果は置き換える（残すと永久に busy になる）。
        assert!(bridge::deliver(
            first,
            Err(PhotoLibraryError::Internal("x"))
        ));
        let second = SCREEN_OWNER.begin_keeping_foreign(&bridge::SLOT).unwrap();
        assert_ne!(first, second);
        bridge::SLOT.clear(second);
        SCREEN_OWNER.release(second);

        // 公開入口の進行中・未取得の結果は守る。
        let public = PUBLIC_OWNER
            .begin_excluding(&bridge::SLOT, &[&SCREEN_OWNER, &PUBLIC_OWNER])
            .unwrap();
        assert!(SCREEN_OWNER.begin_keeping_foreign(&bridge::SLOT).is_none());
        assert!(bridge::deliver(
            public,
            Err(PhotoLibraryError::Internal("x"))
        ));
        assert!(SCREEN_OWNER.begin_keeping_foreign(&bridge::SLOT).is_none());
        assert!(poll_photo_save(public).is_some());
    }

    #[test]
    fn to_outcome_covers_all_paths() {
        assert_eq!(
            to_outcome(Ok(SavedTo::PhotoLibrary)),
            Ok(SaveOutcome::Saved(SavedPlace::PhotoLibrary))
        );
        assert_eq!(
            to_outcome(Ok(SavedTo::MediaStore)),
            Ok(SaveOutcome::Saved(SavedPlace::Gallery))
        );
        assert_eq!(
            to_outcome(Ok(SavedTo::PicturesFolder)),
            Ok(SaveOutcome::Saved(SavedPlace::PicturesFolder))
        );
        assert_eq!(
            to_outcome(Err(PhotoLibraryError::PermissionDenied("photo_library"))),
            Ok(SaveOutcome::Denied)
        );
        assert_eq!(
            to_outcome(Err(PhotoLibraryError::Unsupported("photo_library"))),
            Ok(SaveOutcome::Unsupported)
        );
        assert!(to_outcome(Err(PhotoLibraryError::InvalidInput("photo_library png"))).is_err());
        assert!(to_outcome(Err(PhotoLibraryError::Internal("photo_library save"))).is_err());
    }

    #[test]
    fn outcome_from_apple_code_table() {
        assert!(matches!(
            outcome_from_apple_code(0),
            Ok(SavedTo::PhotoLibrary)
        ));
        assert!(matches!(
            outcome_from_apple_code(1),
            Err(PhotoLibraryError::PermissionDenied("photo_library"))
        ));
        assert!(matches!(
            outcome_from_apple_code(2),
            Err(PhotoLibraryError::Unsupported("photo_library"))
        ));
        for code in [3, 4, 5, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_apple_code(code),
                Err(PhotoLibraryError::Internal(_))
            ));
        }
    }

    #[test]
    fn sample_png_passes_validation() {
        assert!(PngImage::new(sample_png()).is_ok());
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    mod sync_path {
        use super::*;
        use crate::contract::UnsupportedPhotoLibrary;
        use fandhe_core::screens::photo_save::{SavePhase, save};

        #[test]
        fn unsupported_is_shown_and_never_saved() {
            let mut s = PhotoSaveState::default();
            save(&mut s);
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
            drive(&s, &sender, &UnsupportedPhotoLibrary);
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            assert_eq!(s.phase, SavePhase::Idle);
            assert!(!s.status.contains("保存しました"));
            assert!(!s.status.is_empty());
        }
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
mod bridge_tests {
    use super::bridge;
    use super::*;

    #[test]
    fn stale_request_id_is_dropped_and_second_deliver_ignored() {
        let _g = SLOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = bridge::SLOT.begin();
        assert!(!bridge::deliver(id + 1000, Ok(SavedTo::PhotoLibrary)));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(
            id,
            Err(PhotoLibraryError::PermissionDenied("photo_library"))
        ));
        assert!(!bridge::deliver(id, Ok(SavedTo::PhotoLibrary)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(
            ready,
            Some(Err(PhotoLibraryError::PermissionDenied("photo_library")))
        );
        bridge::SLOT.clear(id);
    }
}
