//! Phase 11 の拡充ループ（親 #139）型サンプル「クイック設定タイル・コントロール」（Issue #719）の
//! effect ドライバ。`fandhe_core::screens::quick_settings_tile_show`（端末依存ゼロ）へ、
//! `fandhe_device::quick_settings_tile::QuickSettingsTile` が読んだ回数と追加方法の案内を注入する
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`device/share_receive.rs::drive` と同型）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # pull 型のポーリングと別スレッド読み出し
//!
//! 押下は OS 側の拡張が書くため、画面へは本ドライバが [`POLL_INTERVAL`] ごとに回数を読んで届ける。
//! macOS 15 以降は App Group コンテナへの初回アクセスで OS の確認が出て読み出しがブロックしうる
//! ため、読み出しは UI スレッドではなくワーカースレッドで行う。毎フレーム呼ばれるため、
//! 読み出しは [`Poller`] が高々 1 件に絞る（判定と状態更新は同じロックの内側）。期限は単調時計
//! （`Instant`）で測る。期限前の分岐でも次回のポーリングは必ず予約する
//! （`device-bridge-contracts.md`「4. ポーリング・再描画」）。完了は先に次回の期限と空きを確定して
//! から画面へ送る。失敗の後も読み出しを続け、復帰したら回数を出す。非対応 OS は結果が変わらない
//! ため 1 回だけ送り、以後は読み出しも再描画の予約もしない。
//!
//! # 実装のある OS
//!
//! macOS（[`apple_quick_settings_tile`](super::apple_quick_settings_tile)、Issue #719）と
//! Android（[`android_quick_settings_tile`](super::android_quick_settings_tile)、Issue #720）。iOS は保留、Windows は
//! 対象外で `UnsupportedQuickSettingsTile` 固定のため、画面には「未対応」と出る。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::quick_settings_tile_show::{
    apply_unsupported, finish_read, set_guide, QuickSettingsTileShowState, ReadOutcome, TileGuide,
};
use fandhe_device::quick_settings_tile::QuickSettingsTile;
#[cfg(not(target_os = "android"))]
use fandhe_device::quick_settings_tile::UnsupportedQuickSettingsTile;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 回数を読み直す間隔。表示の遅れはこの間隔ぶん許容する（trait doc の守らない範囲）。
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 読み出しの二重起動防止・世代・次回期限をまとめて持つ。
pub(super) struct Poller {
    inner: Mutex<PollerInner>,
}

struct PollerInner {
    /// 次に読んでよい時刻。`None` は未開始（すぐ読む）。
    next_due: Option<Instant>,
    /// 発行済みの最新世代。
    generation: u64,
    /// 読み出しスレッドが走っているか。
    in_flight: bool,
}

impl Poller {
    pub(super) const fn new() -> Self {
        Self {
            inner: Mutex::new(PollerInner {
                next_due: None,
                generation: 0,
                in_flight: false,
            }),
        }
    }
}

/// 現在の OS の実装（macOS は App Group の回数ファイル、Android は `android_quick_settings_tile`、
/// 他と初期化不能は `Unsupported` 固定）。
fn platform_tile() -> &'static Arc<dyn QuickSettingsTile> {
    static TILE: OnceLock<Arc<dyn QuickSettingsTile>> = OnceLock::new();
    TILE.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            match super::apple_quick_settings_tile::macos_default() {
                Some(t) => Arc::new(t),
                None => Arc::new(UnsupportedQuickSettingsTile),
            }
        }
        #[cfg(target_os = "android")]
        {
            super::android_quick_settings_tile::implementation()
        }
        #[cfg(not(any(target_os = "macos", target_os = "android")))]
        {
            Arc::new(UnsupportedQuickSettingsTile)
        }
    })
}

/// 現在の OS の追加方法の案内（`platform_tile` と同じ cfg 集合）。
fn platform_guide() -> TileGuide {
    #[cfg(target_os = "macos")]
    {
        TileGuide::MacControlCenter
    }
    #[cfg(target_os = "android")]
    {
        TileGuide::AndroidQuickSettings
    }
    #[cfg(not(any(target_os = "macos", target_os = "android")))]
    {
        TileGuide::None
    }
}

/// 毎フレーム呼ばれ、期限が来ていて読み出し中でなければ読み出しを 1 件起動する。
fn drive(
    state: &QuickSettingsTileShowState,
    sender: &CompletionSender<QuickSettingsTileShowState>,
    tile: &Arc<dyn QuickSettingsTile>,
    poller: &'static Poller,
    guide: TileGuide,
    now: Instant,
) {
    // 非対応は確定済み。読み出しも再描画の予約もしない。
    if state.is_unsupported() {
        return;
    }
    // 受信側（ProxyApp）がドロップ済みの送信失敗は無視してよい
    // （coding-rust.md: panic を API 契約にしない）。
    if state.guide() != guide {
        let _ = sender.send(Box::new(move |s: &mut QuickSettingsTileShowState| {
            set_guide(s, guide);
        }));
    }

    let generation = {
        let mut inner = match poller.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // 読み出し中は何もしない。完了の send が再描画を起こす（二重起動の防止）。
        if inner.in_flight {
            return;
        }
        if let Some(due) = inner.next_due {
            if now < due {
                // 送信を省く分岐でも次回を予約する。
                sender.request_repaint_after(due - now);
                return;
            }
        }
        inner.generation += 1;
        inner.in_flight = true;
        inner.generation
    };

    let tile = Arc::clone(tile);
    let tx = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("fandhe-quick-settings-tile".into())
        .spawn({
            let tx = tx.clone();
            move || {
                let result = tile.snapshot();
                finish(poller, &tx, generation, result);
            }
        });
    if spawned.is_err() {
        // 起動できなければ失敗として確定する（成功を装わない）。
        finish(
            poller,
            &tx,
            generation,
            Err(DeviceError::Internal("quick_settings_tile")),
        );
    }
}

/// 読み出しの完了。先に次回の期限と空きを確定してから画面へ送る。
fn finish(
    poller: &Poller,
    sender: &CompletionSender<QuickSettingsTileShowState>,
    generation: u64,
    result: Result<fandhe_device::quick_settings_tile::TileSnapshot, DeviceError>,
) {
    {
        let mut inner = match poller.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        inner.next_due = Some(Instant::now() + POLL_INTERVAL);
        inner.in_flight = false;
    }
    match result {
        Ok(snap) => {
            let outcome = ReadOutcome::Count {
                count: snap.count,
                failed_presses: snap.failed_presses,
            };
            let _ = sender.send(Box::new(move |s: &mut QuickSettingsTileShowState| {
                finish_read(s, generation, outcome);
            }));
            sender.request_repaint_after(POLL_INTERVAL);
        }
        // 非対応は結果が変わらないため 1 回だけ送る。再描画は予約しない。
        Err(DeviceError::Unsupported(_)) => {
            let _ = sender.send(Box::new(apply_unsupported));
        }
        // 理由（パス・OS の文言）は画面へ渡さない。次回も読み、復帰したら回数へ戻る。
        Err(_) => {
            let _ = sender.send(Box::new(move |s: &mut QuickSettingsTileShowState| {
                finish_read(s, generation, ReadOutcome::Failed);
            }));
            sender.request_repaint_after(POLL_INTERVAL);
        }
    }
}

/// `registry.rs` の `effect` 区画から `quick_settings_tile_show` 画面用に配線される effect ドライバ。
pub fn quick_settings_tile_show_effect(
    state: &QuickSettingsTileShowState,
    sender: &CompletionSender<QuickSettingsTileShowState>,
) {
    static POLLER: Poller = Poller::new();
    drive(
        state,
        sender,
        platform_tile(),
        &POLLER,
        platform_guide(),
        Instant::now(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Completion;
    use fandhe_core::screens::quick_settings_tile_show::status_text;
    use fandhe_device::quick_settings_tile::{
        TileSnapshot, UnsupportedQuickSettingsTile as Unsupported,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};

    type St = QuickSettingsTileShowState;

    fn sender_with_channel() -> (CompletionSender<St>, Receiver<Completion<St>>) {
        let (tx, rx) = channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 既定の待ち時間内に届いた完了をすべて状態へ適用し、適用数を返す。
    fn drain_wait(rx: &Receiver<Completion<St>>, state: &mut St, want: usize) -> usize {
        let mut n = 0;
        while n < want {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(c) => {
                    c(state);
                    n += 1;
                }
                Err(_) => break,
            }
        }
        n
    }

    fn drain_now(rx: &Receiver<Completion<St>>, state: &mut St) -> usize {
        let mut n = 0;
        while let Ok(c) = rx.try_recv() {
            c(state);
            n += 1;
        }
        n
    }

    /// 呼び出し回数を数え、毎回同じ結果を返す実装。
    struct Scripted {
        calls: Arc<AtomicUsize>,
        results: Mutex<Vec<Result<TileSnapshot, DeviceError>>>,
    }
    impl QuickSettingsTile for Scripted {
        fn snapshot(&self) -> Result<TileSnapshot, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut r = self.results.lock().unwrap();
            if r.len() > 1 {
                r.remove(0)
            } else {
                match &r[0] {
                    Ok(s) => Ok(*s),
                    Err(_) => Err(DeviceError::Internal("quick_settings_tile")),
                }
            }
        }
    }

    fn scripted(
        results: Vec<Result<TileSnapshot, DeviceError>>,
    ) -> (Arc<dyn QuickSettingsTile>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Scripted {
                calls: calls.clone(),
                results: Mutex::new(results),
            }),
            calls,
        )
    }

    fn leak_poller() -> &'static Poller {
        Box::leak(Box::new(Poller::new()))
    }

    fn snap(count: u64) -> Result<TileSnapshot, DeviceError> {
        Ok(TileSnapshot {
            count,
            failed_presses: None,
        })
    }

    fn fail() -> Result<TileSnapshot, DeviceError> {
        Err(DeviceError::Internal("quick_settings_tile"))
    }

    #[test]
    fn unsupported_is_sent_once_then_silent() {
        let (tx, rx) = sender_with_channel();
        let tile: Arc<dyn QuickSettingsTile> = Arc::new(Unsupported);
        let poller = leak_poller();
        let mut state = St::default();

        drive(&state, &tx, &tile, poller, TileGuide::None, Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert!(state.is_unsupported());

        // 確定後は読み出しも送信もしない。
        drive(
            &state,
            &tx,
            &tile,
            poller,
            TileGuide::None,
            Instant::now() + Duration::from_secs(60),
        );
        assert_eq!(drain_now(&rx, &mut state), 0);
    }

    #[test]
    fn reads_count_and_shows_it() {
        let (tx, rx) = sender_with_channel();
        let (tile, _) = scripted(vec![snap(3)]);
        let mut state = St::default();
        drive(
            &state,
            &tx,
            &tile,
            leak_poller(),
            TileGuide::None,
            Instant::now(),
        );
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert_eq!(status_text(&state), "押された回数: 3");
    }

    #[test]
    fn failure_is_not_zero_and_recovers() {
        let (tx, rx) = sender_with_channel();
        let (tile, _) = scripted(vec![fail(), snap(4)]);
        let poller = leak_poller();
        let mut state = St::default();

        drive(&state, &tx, &tile, poller, TileGuide::None, Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert_eq!(status_text(&state), "回数を読み出せません");

        drive(
            &state,
            &tx,
            &tile,
            poller,
            TileGuide::None,
            Instant::now() + POLL_INTERVAL * 2,
        );
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert_eq!(status_text(&state), "押された回数: 4");
    }

    #[test]
    fn before_due_does_not_read_or_send() {
        let (tx, rx) = sender_with_channel();
        let (tile, calls) = scripted(vec![snap(1)]);
        let poller = leak_poller();
        let mut state = St::default();

        drive(&state, &tx, &tile, poller, TileGuide::None, Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        drive(&state, &tx, &tile, poller, TileGuide::None, Instant::now());
        assert_eq!(drain_now(&rx, &mut state), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// 開始を待たせて、読み出し中の再呼び出しが二重に起動しないことを確かめる。
    struct Blocking {
        calls: Arc<AtomicUsize>,
        gate: Mutex<Receiver<()>>,
    }
    impl QuickSettingsTile for Blocking {
        fn snapshot(&self) -> Result<TileSnapshot, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let _ = self
                .gate
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
            snap(9)
        }
    }

    #[test]
    fn in_flight_read_is_not_started_twice() {
        let (tx, rx) = sender_with_channel();
        let (open, gate): (Sender<()>, Receiver<()>) = channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let tile: Arc<dyn QuickSettingsTile> = Arc::new(Blocking {
            calls: calls.clone(),
            gate: Mutex::new(gate),
        });
        let poller = leak_poller();
        let mut state = St::default();

        let far = Instant::now() + Duration::from_secs(100);
        drive(&state, &tx, &tile, poller, TileGuide::None, far);
        drive(&state, &tx, &tile, poller, TileGuide::None, far);
        drive(&state, &tx, &tile, poller, TileGuide::None, far);
        open.send(()).unwrap();
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(status_text(&state), "押された回数: 9");
    }

    #[test]
    fn guide_is_sent_once() {
        let (tx, rx) = sender_with_channel();
        let (tile, _) = scripted(vec![snap(0)]);
        let poller = leak_poller();
        let mut state = St::default();

        drive(
            &state,
            &tx,
            &tile,
            poller,
            TileGuide::MacControlCenter,
            Instant::now(),
        );
        // guide + 読み出し結果
        assert_eq!(drain_wait(&rx, &mut state, 2), 2);
        assert_eq!(state.guide(), TileGuide::MacControlCenter);

        drive(
            &state,
            &tx,
            &tile,
            poller,
            TileGuide::MacControlCenter,
            Instant::now(),
        );
        assert_eq!(drain_now(&rx, &mut state), 0);
    }

    #[test]
    fn stale_generation_does_not_override_newer_failure() {
        // 古い世代の成功が、より新しい世代の失敗の後に届いても失敗表示のまま。
        let mut state = St::default();
        finish_read(&mut state, 2, ReadOutcome::Failed);
        finish_read(
            &mut state,
            1,
            ReadOutcome::Count {
                count: 5,
                failed_presses: None,
            },
        );
        assert_eq!(status_text(&state), "回数を読み出せません");
    }
}
