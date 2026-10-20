//! Phase 11（親 #108）型サンプル「音楽ライブラリ」（Issue #482）の effect ドライバ。
//! `fandhe_core::screens::music_library_list`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::music_library::MusicLibrary` で曲のタイトルとアーティストを読んで
//! `CompletionSender` 経由で結果を返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `fandhe_device::music_library`。
//!
//! # スレッド
//!
//! どの OS の実装も許可ダイアログ・期限付きのブロッキング待ちになるため、UI を止めないよう OS を
//! 問わずワーカースレッドで呼ぶ。完了は世代照合つきで画面状態へ入れる（古い世代は捨てられる）。
//!
//! # OS 別実装の差し込み口
//!
//! [`implementation`] の cfg 分岐が最終形で、各 OS は `implementation()` を持つモジュールだけを
//! 埋める（macOS: `macos_music_library`（#482）、Windows: `windows_music_library`（#483）、
//! iOS: `ios_music_library`（#484）、Android: `android_music_library`（#485））。
//! それ以外（Linux 等）は `UnsupportedMusicLibrary` 固定で、ダミーの成功実装へは倒さない。
//!
//! 文字列バッファの解釈 [`tracks_from_nul_buffer`] は macOS・iOS の `.m` が共用する。

use std::sync::Arc;

use fandhe_core::screens::music_library_list::{
    finish_list, mark_loading, take_request, ListFailure, MusicLibraryListState, TrackListing,
};
#[cfg(any(target_os = "macos", target_os = "ios", test))]
use fandhe_device::music_library::tracks_from;
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
use fandhe_device::music_library::UnsupportedMusicLibrary;
use fandhe_device::music_library::{MusicLibrary, MusicLibraryFailure, TrackList, LIST_TIMEOUT};

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `MusicLibrary` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn MusicLibrary> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn MusicLibrary>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                super::macos_music_library::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_music_library::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_music_library::implementation()
            }
            #[cfg(target_os = "windows")]
            {
                super::windows_music_library::implementation()
            }
            #[cfg(not(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "android",
                target_os = "windows"
            )))]
            {
                Arc::new(UnsupportedMusicLibrary)
            }
        })
        .clone()
}

/// `MusicLibraryFailure` を画面の失敗種別へ写す。`Failed` は固定文言へ畳み、OS 固有の値を
/// 画面へ漏らさない（security.md A09）。
fn to_failure(e: MusicLibraryFailure) -> ListFailure {
    match e {
        MusicLibraryFailure::Unsupported => ListFailure::Unsupported,
        MusicLibraryFailure::PermissionDenied => ListFailure::PermissionDenied,
        MusicLibraryFailure::Timeout => ListFailure::Timeout,
        MusicLibraryFailure::Failed => ListFailure::Failed("曲の一覧の読み取り".to_string()),
    }
}

/// device 側の一覧を画面の表示用へ写す（`fandhe-core` は `fandhe-device` を参照できないため）。
fn to_listing(list: TrackList) -> TrackListing {
    TrackListing {
        rows: list
            .tracks
            .into_iter()
            .map(|t| (t.title, t.artist))
            .collect(),
        truncated: list.truncated,
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &MusicLibraryListState,
    sender: &CompletionSender<MusicLibraryListState>,
    library: Arc<dyn MusicLibrary>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    let _ = sender.send(Box::new(mark_loading));

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("music-library-list".to_string())
        .spawn(move || {
            let result = library
                .list_tracks(LIST_TIMEOUT)
                .map(to_listing)
                .map_err(to_failure);
            let _ = worker_sender.send(Box::new(move |s: &mut MusicLibraryListState| {
                finish_list(s, generation, result);
            }));
        });
    if spawned.is_err() {
        // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
        let _ = sender.send(Box::new(move |s: &mut MusicLibraryListState| {
            finish_list(
                s,
                generation,
                Err(ListFailure::Failed("曲の一覧の読み取り".to_string())),
            );
        }));
    }
}

/// `registry.rs` の `effect` 区画から `music_library_list` 画面用に配線される effect ドライバ。
pub fn music_library_list_effect(
    state: &MusicLibraryListState,
    sender: &CompletionSender<MusicLibraryListState>,
) {
    drive(state, sender, implementation());
}

/// `.m` が詰めた「title\0artist\0」の組の列を曲の一覧へ解釈する（macOS・iOS 共用）。
/// `count` は組の数で、`overflowed` は 20 件超または領域不足で打ち切ったこと。
/// 要素数が奇数・UTF-8 として不正・組の数が `count` と食い違う場合は `Failed`
/// （不正な項目を捨てると組がずれて曲名とアーティストが入れ替わるため、捨てずに失敗にする）。
#[cfg(any(target_os = "macos", target_os = "ios", test))]
// iOS は #484 が使い始めるまでスタブで未使用（そのとき本 allow を macos 限定から外す）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn tracks_from_nul_buffer(
    buf: &[u8],
    count: u32,
    overflowed: bool,
) -> Result<TrackList, MusicLibraryFailure> {
    let fail = MusicLibraryFailure::Failed;
    let parts: Vec<&[u8]> = match buf {
        [] => Vec::new(),
        // 末尾は必ず NUL 終端。
        [rest @ .., 0] => rest.split(|b| *b == 0).collect(),
        _ => return Err(fail),
    };
    if parts.len() % 2 != 0 || parts.len() / 2 != count as usize {
        return Err(fail);
    }
    let mut rows = Vec::with_capacity(count as usize);
    for pair in parts.chunks(2) {
        let to_opt = |b: &[u8]| -> Result<Option<String>, MusicLibraryFailure> {
            let s = std::str::from_utf8(b).map_err(|_| fail)?;
            Ok((!s.is_empty()).then(|| s.to_string()))
        };
        rows.push((to_opt(pair[0])?, to_opt(pair[1])?));
    }
    let mut list = tracks_from(rows);
    list.truncated |= overflowed;
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::music_library_list::{
        fetch, PERMISSION_DENIED_STATUS, TIMEOUT_STATUS, UNSUPPORTED_STATUS,
    };
    use fandhe_device::music_library::{
        tracks_from as device_tracks_from, UnsupportedMusicLibrary as Unsupported,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn sender_with_channel() -> (
        CompletionSender<MusicLibraryListState>,
        std::sync::mpsc::Receiver<crate::app::Completion<MusicLibraryListState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake {
        result: Result<TrackList, MusicLibraryFailure>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(result: Result<TrackList, MusicLibraryFailure>) -> Arc<Self> {
            Arc::new(Self {
                result,
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl MusicLibrary for Fake {
        fn list_tracks(&self, _timeout: Duration) -> Result<TrackList, MusicLibraryFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.clone()
        }
    }

    /// 要求を立てて `drive` し、届いた完了（mark_loading と結果）をすべて適用した状態を返す。
    fn run(library: Arc<dyn MusicLibrary>) -> MusicLibraryListState {
        let (sender, rx) = sender_with_channel();
        let mut state = MusicLibraryListState::default();
        fetch(&mut state);
        drive(&state, &sender, library);
        for _ in 0..2 {
            let c = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("mark_loading と完了が届く");
            c(&mut state);
        }
        state
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(Ok(TrackList::default()));
        drive(&MusicLibraryListState::default(), &sender, fake.clone());
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn zero_tracks_is_shown_as_success() {
        let state = run(Fake::new(Ok(TrackList::default())));
        assert_eq!(state.listing, Some(TrackListing::default()));
        assert_eq!(state.error, "");
    }

    #[test]
    fn tracks_are_passed_through() {
        let list = device_tracks_from([(Some("t".to_string()), None)]);
        let state = run(Fake::new(Ok(list)));
        assert_eq!(
            state.listing.map(|l| l.rows),
            Some(vec![(Some("t".to_string()), None)])
        );
    }

    #[test]
    fn each_failure_is_error_not_success() {
        let cases = [
            (MusicLibraryFailure::Unsupported, UNSUPPORTED_STATUS),
            (
                MusicLibraryFailure::PermissionDenied,
                PERMISSION_DENIED_STATUS,
            ),
            (MusicLibraryFailure::Timeout, TIMEOUT_STATUS),
        ];
        for (f, expected) in cases {
            let state = run(Fake::new(Err(f)));
            assert_eq!(state.error, expected);
            assert_eq!(state.listing, None);
        }
        let state = run(Fake::new(Err(MusicLibraryFailure::Failed)));
        assert!(
            state.error.starts_with("読めませんでした: "),
            "{}",
            state.error
        );
        assert_eq!(state.listing, None);
    }

    #[test]
    fn second_drive_while_loading_does_not_call_os_again() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(Ok(TrackList::default()));
        let mut state = MusicLibraryListState::default();
        fetch(&mut state);
        drive(&state, &sender, fake.clone());
        // mark_loading を適用した状態では要求が観測されない。
        rx.recv_timeout(Duration::from_secs(5))
            .expect("mark_loading")(&mut state);
        drive(&state, &sender, fake.clone());
        rx.recv_timeout(Duration::from_secs(5)).expect("finish")(&mut state);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_completion_does_not_overwrite_newer_request() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(Ok(TrackList::default()));
        let mut state = MusicLibraryListState::default();
        fetch(&mut state);
        drive(&state, &sender, fake);
        rx.recv_timeout(Duration::from_secs(5))
            .expect("mark_loading")(&mut state);
        // 完了が届く前に世代が進んだ（利用者が画面を作り直した等）状況を再現する。
        state.phase = fandhe_core::screens::music_library_list::Phase::Idle;
        fetch(&mut state);
        rx.recv_timeout(Duration::from_secs(5)).expect("finish")(&mut state);
        assert_eq!(state.generation, 2);
        assert_eq!(state.listing, None);
        assert_eq!(
            state.phase,
            fandhe_core::screens::music_library_list::Phase::Requested
        );
    }

    #[test]
    fn unsupported_library_never_yields_tracks() {
        let state = run(Arc::new(Unsupported));
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.listing, None);
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert_eq!(
            implementation()
                .list_tracks(Duration::from_millis(1))
                .map(|_| ()),
            Err(MusicLibraryFailure::Unsupported)
        );
    }

    #[test]
    fn nul_buffer_parses_pairs() {
        let l = tracks_from_nul_buffer(b"T1\0A1\0\0A2\0T3\0\0", 3, false).unwrap();
        assert_eq!(l.tracks.len(), 3);
        assert_eq!(l.tracks[0].title.as_deref(), Some("T1"));
        assert_eq!(l.tracks[1].title, None);
        assert_eq!(l.tracks[1].artist.as_deref(), Some("A2"));
        assert_eq!(l.tracks[2].artist, None);
        assert!(!l.truncated);
    }

    #[test]
    fn nul_buffer_overflow_sets_truncated() {
        assert!(
            tracks_from_nul_buffer(b"T\0A\0", 1, true)
                .unwrap()
                .truncated
        );
    }

    #[test]
    fn nul_buffer_empty_with_zero_count_is_empty_success() {
        let l = tracks_from_nul_buffer(b"", 0, false).unwrap();
        assert!(l.tracks.is_empty());
    }

    #[test]
    fn nul_buffer_rejects_broken_input() {
        let failed = Err(MusicLibraryFailure::Failed);
        // 要素数が奇数
        assert_eq!(tracks_from_nul_buffer(b"T\0", 1, false).map(|_| ()), failed);
        // 不正な UTF-8
        assert_eq!(
            tracks_from_nul_buffer(b"\xff\0A\0", 1, false).map(|_| ()),
            failed
        );
        // count との不一致
        assert_eq!(
            tracks_from_nul_buffer(b"T\0A\0", 2, false).map(|_| ()),
            failed
        );
        assert_eq!(tracks_from_nul_buffer(b"", 1, false).map(|_| ()), failed);
        // NUL 終端が無い
        assert_eq!(
            tracks_from_nul_buffer(b"T\0A", 1, false).map(|_| ()),
            failed
        );
    }
}
