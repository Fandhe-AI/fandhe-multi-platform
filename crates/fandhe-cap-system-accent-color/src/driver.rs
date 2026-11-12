//! Phase 11 の拡充ループ（親 #103、Issue #421）「システムのアクセント色」の effect ドライバ（機能 crate へ移行、Issue #1189）。
//! `crate::contract::AccentColorSource` を一定間隔で読み、結果を
//! `fandhe_core::screens::system_accent_color_show`（端末依存ゼロ）へ注入する
//! （`registry.rs` の `effect` 区画から毎フレーム同期に呼ばれる）。
//!
//! **間隔つき pull**（`accessibility_prefs.rs` と同型でアニメーション部分なし）: 最終読取から
//! `READ_INTERVAL` 未満なら OS を読まない。状態が変わらなければ送信を省くが、次の読み取りまでの
//! 再描画を必ず予約する（入力が無いと egui が描画せずポーリングが止まる、契約表 4）。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。macOS は `macos_system_accent_color.rs`、
//! Windows は `windows_system_accent_color.rs`、Android は `android_system_accent_color.rs`
//! （JNI の同期 pull）。Web は `fandhe-web-dom` 側（#423）で、純粋な定数 `READ_INTERVAL` だけを本 crate から使う。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
use crate::contract::UnsupportedAccentColor;
use crate::contract::{self as dev, AccentColorSource, READ_INTERVAL};
use crate::error::SystemAccentColorError;
use fandhe_core::screens::system_accent_color_show::{
    ReadFailure, Rgb, SystemAccentColorShowState, apply_read,
};

use fandhe_runtime::CompletionSender;

/// 現在のプラットフォームに応じた実装を返す。フレームワークの外のアプリも直接使える入口。
pub fn platform_system_accent_color() -> Arc<dyn AccentColorSource> {
    #[cfg(target_os = "windows")]
    {
        Arc::new(crate::windows::WindowsAccentColor)
    }
    #[cfg(target_os = "macos")]
    {
        Arc::new(crate::macos::MacosAccentColor)
    }
    #[cfg(target_os = "android")]
    {
        crate::android::implementation()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
    {
        Arc::new(UnsupportedAccentColor)
    }
}

/// 契約の結果を画面の入力へ写す。失敗は固定文言へ畳み、OS 固有値を画面へ漏らさない
/// （security.md A09）。
fn to_result(r: Result<dev::Rgb, SystemAccentColorError>) -> Result<Rgb, ReadFailure> {
    match r {
        Ok(c) => Ok(Rgb {
            r: c.r,
            g: c.g,
            b: c.b,
        }),
        Err(SystemAccentColorError::Unsupported) => Err(ReadFailure::Unsupported),
        Err(_) => Err(ReadFailure::Failed("アクセント色の取得".to_string())),
    }
}

/// 最終読取の時刻（プロセス全体で 1 つ）。
#[derive(Default)]
struct Timing {
    last_read: Option<Instant>,
}

static TIMING: Mutex<Timing> = Mutex::new(Timing { last_read: None });

/// 間隔判定・読取・送信省略を行う本体（テストから時刻と実装を差し込めるよう切り出す）。
fn drive(
    state: &SystemAccentColorShowState,
    sender: &CompletionSender<SystemAccentColorShowState>,
    src: &dyn AccentColorSource,
    now: Instant,
    timing: &mut Timing,
) {
    let due = timing
        .last_read
        .is_none_or(|prev| now.saturating_duration_since(prev) >= READ_INTERVAL);
    let result = if due {
        timing.last_read = Some(now);
        Some(to_result(src.read()))
    } else {
        None
    };
    // 送信を省く分岐でも次回の読み取りを予約する（契約表 4）。
    let since_read = timing
        .last_read
        .map_or(Duration::ZERO, |p| now.saturating_duration_since(p));
    sender.request_repaint_after(READ_INTERVAL.saturating_sub(since_read));

    let Some(result) = result else { return };
    let mut next = state.clone();
    apply_read(&mut next, result.clone());
    // 失敗状態のまま毎フレーム送らない。
    if next == *state {
        return;
    }
    let _ = sender.send(Box::new(move |s: &mut SystemAccentColorShowState| {
        apply_read(s, result.clone())
    }));
}

/// `registry.rs` の `effect` 区画から `system_accent_color_show` 画面用に配線される effect ドライバ。
pub fn system_accent_color_show_effect(
    state: &SystemAccentColorShowState,
    sender: &CompletionSender<SystemAccentColorShowState>,
) {
    let mut timing = TIMING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    drive(
        state,
        sender,
        platform_system_accent_color().as_ref(),
        Instant::now(),
        &mut timing,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::UnsupportedAccentColor as Unsupported;
    use fandhe_core::screens::system_accent_color_show::AccentView;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    type Rx = std::sync::mpsc::Receiver<fandhe_runtime::Completion<SystemAccentColorShowState>>;

    fn sender_with_channel() -> (CompletionSender<SystemAccentColorShowState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
    }

    struct Fake {
        result: Result<dev::Rgb, SystemAccentColorError>,
        reads: AtomicUsize,
    }

    impl Fake {
        fn new(result: Result<dev::Rgb, SystemAccentColorError>) -> Self {
            Self {
                result,
                reads: AtomicUsize::new(0),
            }
        }
        fn color(r: u8, g: u8, b: u8) -> Self {
            Self::new(Ok(dev::Rgb { r, g, b }))
        }
    }

    impl AccentColorSource for Fake {
        fn read(&self) -> Result<dev::Rgb, SystemAccentColorError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            match &self.result {
                Ok(c) => Ok(*c),
                Err(SystemAccentColorError::Unsupported) => {
                    Err(SystemAccentColorError::Unsupported)
                }
                Err(_) => Err(SystemAccentColorError::Internal),
            }
        }
    }

    fn apply_all(rx: &Rx, s: &mut SystemAccentColorShowState) -> usize {
        let mut n = 0;
        while let Ok(c) = rx.try_recv() {
            c(s);
            n += 1;
        }
        n
    }

    #[test]
    fn skips_os_read_within_interval() {
        let (tx, _rx) = sender_with_channel();
        let fake = Fake::color(1, 2, 3);
        let state = SystemAccentColorShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(&state, &tx, &fake, t0, &mut timing);
        drive(
            &state,
            &tx,
            &fake,
            t0 + Duration::from_millis(500),
            &mut timing,
        );
        assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
        drive(&state, &tx, &fake, t0 + READ_INTERVAL, &mut timing);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn same_value_is_not_sent_but_change_is() {
        let (tx, rx) = sender_with_channel();
        let mut state = SystemAccentColorShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(&state, &tx, &Fake::color(1, 2, 3), t0, &mut timing);
        assert_eq!(apply_all(&rx, &mut state), 1);
        drive(
            &state,
            &tx,
            &Fake::color(1, 2, 3),
            t0 + READ_INTERVAL,
            &mut timing,
        );
        assert!(rx.try_recv().is_err());
        drive(
            &state,
            &tx,
            &Fake::color(9, 9, 9),
            t0 + READ_INTERVAL * 2,
            &mut timing,
        );
        assert_eq!(apply_all(&rx, &mut state), 1);
        assert_eq!(state.view, AccentView::Read(Rgb { r: 9, g: 9, b: 9 }));
    }

    #[test]
    fn failures_never_become_read() {
        for r in [
            Err(SystemAccentColorError::Unsupported),
            Err(SystemAccentColorError::Internal),
        ] {
            let (tx, rx) = sender_with_channel();
            let mut state = SystemAccentColorShowState::default();
            drive(
                &state,
                &tx,
                &Fake::new(r),
                Instant::now(),
                &mut Timing::default(),
            );
            apply_all(&rx, &mut state);
            assert!(matches!(
                state.view,
                AccentView::Unsupported | AccentView::Failed(_)
            ));
        }
    }

    #[test]
    fn previous_value_is_not_kept_after_failure() {
        let (tx, rx) = sender_with_channel();
        let mut state = SystemAccentColorShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(&state, &tx, &Fake::color(1, 2, 3), t0, &mut timing);
        apply_all(&rx, &mut state);
        let failing = Fake::new(Err(SystemAccentColorError::Internal));
        drive(&state, &tx, &failing, t0 + READ_INTERVAL, &mut timing);
        apply_all(&rx, &mut state);
        assert!(!matches!(state.view, AccentView::Read(_)));
    }

    #[test]
    fn unsupported_impl_reports_unsupported() {
        let (tx, rx) = sender_with_channel();
        let mut state = SystemAccentColorShowState::default();
        drive(
            &state,
            &tx,
            &Unsupported,
            Instant::now(),
            &mut Timing::default(),
        );
        apply_all(&rx, &mut state);
        assert_eq!(state.view, AccentView::Unsupported);
    }
}
