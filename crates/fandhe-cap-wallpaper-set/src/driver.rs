//! Phase 11（親 #188）型サンプル「壁紙・ロック画面の設定」（Issue #970）の effect ドライバ。
//! `fandhe_core::screens::wallpaper_set`（端末依存ゼロ）の要求を観測し、アプリが生成した PNG
//! （[`sample_png`]）を `fandhe_device::photo_library::PngImage` で検査してから OS へ渡し、
//! 結果を `CompletionSender` 経由で返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から呼ばれる契約）。契約と守る範囲は `crate::contract` のモジュール doc。
//!
//! # dispatch（最終形）
//!
//! Android は [`crate::android::begin_set`] を埋める形で、本ファイルの分岐は変えない。
//!
//! | OS | 経路 |
//! |---|---|
//! | macOS | 同期（egui の update は main スレッド）。[`crate::macos`] |
//! | Windows | 同期（UI スレッドで `SystemParametersInfoW` を 1 回）。[`crate::windows`] |
//! | Android | コールバック型。pull 型 [`bridge`] + [`crate::android::begin_set`] |
//! | iOS・Linux | `UnsupportedWallpaper`（「この端末では使えません」） |

use crate::contract::WallpaperTarget;
use crate::error::WallpaperSetError;
use fandhe_core::screens::wallpaper_set::{
    SetOutcome, WallpaperChoice, WallpaperSetState, finish_set, mark_setting, take_request,
};
use fandhe_device::photo_library::PngImage;

use fandhe_runtime::CompletionSender;

/// 設定する見本画像（256x256 のグラデーション PNG）。
///
/// ponytail: photo-save の見本画像と同じ生成を持つ複製（photo-save は未移行で、その `pub(crate)` は機能 crate から見えない）。
/// photo-save を移すときに、見本の生成を共有するか見直す。
pub fn sample_png() -> Vec<u8> {
    let img = image::RgbaImage::from_fn(256, 256, |x, y| image::Rgba([x as u8, y as u8, 160, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    // メモリ上への PNG 書き出しは失敗しない。万一失敗しても空バイト列は
    // `PngImage::new` が入力不正として弾くため、成功は作られない。
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

/// 見本画像を検査して `PngImage` にする。検査に通らなければ OS へ渡さない。
fn checked_sample() -> Result<PngImage, WallpaperSetError> {
    PngImage::new(sample_png()).map_err(|_| WallpaperSetError::InvalidInput("photo_library png"))
}

/// `WallpaperSetError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする
/// （security.md A09）。`Cancelled` は UI が無く届かないが、万一届いても成功にはしない。
fn to_outcome(result: Result<WallpaperTarget, WallpaperSetError>) -> Result<SetOutcome, String> {
    match result {
        Ok(t) => Ok(SetOutcome::Set(to_choice(t))),
        Err(WallpaperSetError::PermissionDenied) => Ok(SetOutcome::Denied),
        Err(WallpaperSetError::Unsupported) => Ok(SetOutcome::Unsupported),
        Err(WallpaperSetError::InvalidInput(_)) => Err("画像が不正です".to_string()),
        Err(_) => Err("OS の壁紙設定に失敗しました".to_string()),
    }
}

fn to_target(choice: WallpaperChoice) -> WallpaperTarget {
    match choice {
        WallpaperChoice::Home => WallpaperTarget::Home,
        WallpaperChoice::Lock => WallpaperTarget::Lock,
    }
}

fn to_choice(target: WallpaperTarget) -> WallpaperChoice {
    match target {
        WallpaperTarget::Home => WallpaperChoice::Home,
        WallpaperTarget::Lock => WallpaperChoice::Lock,
    }
}

/// OS 側コールバックとの橋渡し（Android、pull 型、`RequestSlot` の facade）。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) mod bridge {
    use crate::contract::WallpaperTarget;
    use crate::error::WallpaperSetError;

    use fandhe_runtime::request_slot::{RequestSlot, SlotOwner};

    pub(crate) type Outcome = Result<WallpaperTarget, WallpaperSetError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    /// 画面の effect が登録した要求の印。公開入口の要求を置き換えたり、その結果を取り出したりしないために使う。
    pub(crate) static SCREEN: SlotOwner = SlotOwner::new();

    /// 画面用の登録。公開入口の進行中の要求・未取得の結果があれば `None`（置き換えて落とさない）。
    pub(crate) fn begin() -> Option<u64> {
        SCREEN.begin_keeping_foreign(&SLOT)
    }

    /// OS からの結果を契約のまま置く（画面用への写像は effect ドライバ側、公開入口は生の結果を返す）。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<WallpaperTarget, WallpaperSetError>,
    ) -> bool {
        SLOT.deliver(request_id, result)
    }

    /// Activity 再生成直後に `android_main` から呼ぶ。保留中の要求は復元せず破棄し、
    /// 以後届く旧 `request_id` の [`deliver`] を無視させる（`photo_save::bridge::reset` と同じ）。
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// gallery の `android_main`（Activity 再生成）から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
}

/// Android 版の effect ドライバ本体。
#[cfg(target_os = "android")]
fn drive_callback(state: &WallpaperSetState, sender: &CompletionSender<WallpaperSetState>) {
    use fandhe_core::screens::wallpaper_set::SetPhase;
    if let Some(req) = take_request(state) {
        let _ = sender.send(Box::new(mark_setting));
        let Some(request_id) = bridge::begin() else {
            // 公開入口の要求が進行中・未取得。置き換えず、画面へ「処理中」の失敗を返す。
            let generation = req.generation;
            let outcome = to_outcome(Err(WallpaperSetError::Internal("wallpaper set busy")));
            let _ = sender.send(Box::new(move |s: &mut WallpaperSetState| {
                finish_set(s, generation, outcome);
            }));
            return;
        };
        // 検査を通らない画像は OS へ渡さずその場で画面へ返す。
        let accepted = checked_sample()
            .and_then(|png| crate::android::begin_set(request_id, &png, to_target(req.choice)));
        if let Err(e) = accepted {
            bridge::deliver(request_id, Err(e));
        }
        return;
    }

    if state.phase != SetPhase::Setting {
        return;
    }
    let generation = state.generation;
    let mine = bridge::SCREEN.current();
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &bridge::SCREEN,
        sender,
        |_request_id| {},
        move |s: &mut WallpaperSetState, result| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != SetPhase::Idle {
                finish_set(s, generation, to_outcome(result));
                true
            } else {
                // 反映できない完了は捨てる（残すと次の要求が置き換えるまで未取得のまま残る）。
                bridge::SLOT.clear(mine);
                false
            }
        },
        // 自分の要求が失われた（Activity 再生成の `reset` 等）。失敗を確定して画面を処理中のまま残さない。
        move |s: &mut WallpaperSetState| {
            let outcome = to_outcome(Err(WallpaperSetError::Internal("wallpaper set lost")));
            finish_set(s, generation, outcome);
        },
    );
}

/// `registry.rs` から配線される effect ドライバ（Android）。
#[cfg(target_os = "android")]
pub fn wallpaper_set_effect(
    state: &WallpaperSetState,
    sender: &CompletionSender<WallpaperSetState>,
) {
    drive_callback(state, sender);
}

/// 壁紙の設定を OS へ依頼し、結果を [`poll_wallpaper_set`] で受け取るための要求 id を返す（Android）。
///
/// 画面の状態型に依存しない非同期の入口で、フレームワークの外のアプリも使える。画像は呼び出し側が
/// `PngImage::new` で検査済みのものを渡す。進行中の要求があるうちは `Internal`（busy）で断る。
/// 依頼自体の失敗は `Err`（結果は届かない）。macOS・Windows は同期の [`platform_wallpaper_set`] を使う。
/// iOS・Linux は `Unsupported`。
#[cfg(target_os = "android")]
pub fn begin_wallpaper_set(
    image: &PngImage,
    target: WallpaperTarget,
) -> Result<u64, WallpaperSetError> {
    let request_id = bridge::SLOT
        .try_begin_unless(|_| true)
        .ok_or(WallpaperSetError::Internal("wallpaper set busy"))?;
    if let Err(e) = crate::android::begin_set(request_id, image, target) {
        bridge::SLOT.clear(request_id);
        return Err(e);
    }
    Ok(request_id)
}

/// Android 以外には OS コールバック型の実装がない（成功へは倒さず `Unsupported`）。
#[cfg(not(target_os = "android"))]
pub fn begin_wallpaper_set(
    _image: &PngImage,
    _target: WallpaperTarget,
) -> Result<u64, WallpaperSetError> {
    Err(WallpaperSetError::Unsupported)
}

/// [`begin_wallpaper_set`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・取り出し済みは `None`。
pub fn poll_wallpaper_set(request_id: u64) -> Option<Result<WallpaperTarget, WallpaperSetError>> {
    #[cfg(target_os = "android")]
    return bridge::SLOT.take_ready(request_id);
    #[cfg(not(target_os = "android"))]
    {
        let _ = request_id;
        None
    }
}

/// 同期経路（macOS・Windows・iOS・Linux）: 検査してから [`Wallpaper`] を呼ぶ。
///
/// [`Wallpaper`]: crate::contract::Wallpaper
#[cfg(not(target_os = "android"))]
fn drive(
    state: &WallpaperSetState,
    sender: &CompletionSender<WallpaperSetState>,
    wp: &dyn crate::contract::Wallpaper,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_setting));
    let result = checked_sample().and_then(|png| wp.set_wallpaper(&png, to_target(request.choice)));
    let outcome = to_outcome(result);
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut WallpaperSetState| {
        finish_set(s, generation, outcome);
    }));
}

/// `registry.rs` から配線される effect ドライバ（macOS・Windows・iOS・Linux）。
///
/// ponytail: Windows は `SPIF_SENDCHANGE` のブロードキャストで UI スレッドが数百 ms 止まりうる。
/// 目立つなら作業スレッドと bridge へ移す。
#[cfg(not(target_os = "android"))]
pub fn wallpaper_set_effect(
    state: &WallpaperSetState,
    sender: &CompletionSender<WallpaperSetState>,
) {
    drive(state, sender, platform_wallpaper_set().as_ref());
}

/// 現在のプラットフォームの同期の `Wallpaper` 実装を返す（macOS・Windows）。画面の状態型に依存しない入口で、
/// フレームワークの外のアプリも画像を直接渡して設定できる。Android はコールバック型で trait を通らず
/// （[`begin_wallpaper_set`]）、iOS・Linux と同じく `UnsupportedWallpaper` を返す（成功へは倒さない）。
pub fn platform_wallpaper_set() -> Box<dyn crate::contract::Wallpaper> {
    #[cfg(target_os = "macos")]
    return Box::new(crate::macos::MacosWallpaper);
    #[cfg(target_os = "windows")]
    return Box::new(crate::windows::WindowsWallpaper);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(crate::contract::UnsupportedWallpaper);
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 起床しないだけの口（受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    use fandhe_device::photo_library::PngImage;

    #[test]
    fn to_outcome_covers_all_paths() {
        assert_eq!(
            to_outcome(Ok(WallpaperTarget::Home)),
            Ok(SetOutcome::Set(WallpaperChoice::Home))
        );
        assert_eq!(
            to_outcome(Ok(WallpaperTarget::Lock)),
            Ok(SetOutcome::Set(WallpaperChoice::Lock))
        );
        assert_eq!(
            to_outcome(Err(WallpaperSetError::PermissionDenied)),
            Ok(SetOutcome::Denied)
        );
        assert_eq!(
            to_outcome(Err(WallpaperSetError::Unsupported)),
            Ok(SetOutcome::Unsupported)
        );
        for e in [
            WallpaperSetError::InvalidInput("photo_library png"),
            WallpaperSetError::Internal("wallpaper set"),
            WallpaperSetError::Cancelled,
        ] {
            assert!(to_outcome(Err(e)).is_err());
        }
    }

    #[test]
    fn choice_target_roundtrip() {
        for c in [WallpaperChoice::Home, WallpaperChoice::Lock] {
            assert_eq!(to_choice(to_target(c)), c);
        }
    }

    #[test]
    fn sample_png_passes_validation() {
        assert!(PngImage::new(sample_png()).is_ok());
    }

    #[test]
    fn unsupported_is_shown_and_never_set() {
        use fandhe_core::screens::wallpaper_set::{SetPhase, set_home};
        let mut s = WallpaperSetState::default();
        set_home(&mut s);
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        drive(&s, &sender, &crate::contract::UnsupportedWallpaper);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        assert_eq!(s.phase, SetPhase::Idle);
        assert!(!s.status.contains("設定しました"));
        assert!(!s.status.is_empty());
    }

    #[test]
    fn bridge_drops_stale_id_and_second_deliver() {
        let id = bridge::begin().expect("begin");
        assert!(!bridge::deliver(id + 1000, Ok(WallpaperTarget::Home)));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(
            id,
            Err(WallpaperSetError::PermissionDenied)
        ));
        assert!(!bridge::deliver(id, Ok(WallpaperTarget::Home)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(ready, Some(Err(WallpaperSetError::PermissionDenied)));
        bridge::SLOT.clear(id);
    }
}
