//! 「一定時間後に状態を変える」書き方（`CompletionSender::after`）が `ProxyApp` で動くことの検証（Issue #1291）。
//!
//! effect は毎フレーム「状態が copied なら 2 秒後に戻す」と書くだけで、スレッドも sleep も要らない。
//! ここでは待ちを短くし（150 ms）、実時間で確かめる。時計を差し替えた厳密な期限の判定は
//! `fandhe-runtime` の `timer` のユニットテストが持つ。

use egui_kittest::Harness;
use fandhe_core::Screen;
use fandhe_render_egui::{CompletionSender, EguiApp, ProxyApp};
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
use std::time::{Duration, Instant};

// `ProxyApp` は状態を外へ出さないため、毎フレーム呼ばれる画面の構築関数で状態を観測する。
static COPIED: AtomicU32 = AtomicU32::new(0);
static RESETS: AtomicU32 = AtomicU32::new(0);
static TICKS: AtomicU32 = AtomicU32::new(0);

#[derive(Default)]
struct St {
    copied: bool,
    resets: u32,
    ticks: u32,
}

const DELAY: Duration = Duration::from_millis(150);

fn effect(s: &St, sender: &CompletionSender<St>) {
    if s.copied {
        sender
            .after("reset", DELAY, |s: &mut St| {
                s.copied = false;
                s.resets += 1;
            })
            .expect("積める");
    } else {
        sender.cancel_timer("reset");
    }
    // 定期更新: 期限が来て適用されるたびに、次のフレームの effect が同じキーで積み直す。
    sender
        .after("tick", Duration::from_millis(60), |s: &mut St| s.ticks += 1)
        .expect("積める");
}

fn screen(s: &St) -> Screen<St> {
    COPIED.store(s.copied as u32, SeqCst);
    RESETS.store(s.resets, SeqCst);
    TICKS.store(s.ticks, SeqCst);
    Screen::new("t").text("x")
}

#[test]
fn state_reverts_after_delay_and_periodic_update_repeats() {
    let ctx = egui::Context::default();
    let app = ProxyApp::with_screen_fn(
        &ctx,
        screen,
        St {
            copied: true,
            ..Default::default()
        },
    )
    .with_effect(effect);
    let mut h = Harness::new_state(|ctx, app: &mut ProxyApp<St>| EguiApp::update(app, ctx), app);

    let started = Instant::now();
    h.step();
    h.step();
    assert!(COPIED.load(SeqCst) == 1, "期限前は戻らない");

    while COPIED.load(SeqCst) == 1 && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
        h.step();
    }
    let at = started.elapsed();
    assert!(COPIED.load(SeqCst) == 0, "期限後に状態が戻る");
    assert!(at >= DELAY, "期限より早く戻してはいけない: {at:?}");
    assert_eq!(RESETS.load(SeqCst), 1);

    // 戻した後は二度と適用されない（effect が取り下げ側へ回る）。定期更新は繰り返し進む。
    let ticks_before = TICKS.load(SeqCst);
    let until = Instant::now() + Duration::from_millis(400);
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
        h.step();
    }
    assert_eq!(RESETS.load(SeqCst), 1);
    assert!(
        TICKS.load(SeqCst) >= ticks_before + 3,
        "定期更新が繰り返す: {}",
        TICKS.load(SeqCst)
    );
}

/// アプリを破棄すれば、予約のクロージャが抱えた sender の複製と資源も解放される（`Timers → クロージャ → sender → Timers` の
/// 循環にならない）。外のワーカーが sender を持ち続けていても、破棄後の `after` は断られる。
#[test]
fn dropping_the_app_frees_timers_even_if_they_hold_the_sender() {
    let ctx = egui::Context::default();
    let app = ProxyApp::with_screen_fn(&ctx, screen, St::default());
    let sender = app.completion_sender();
    let resource = std::sync::Arc::new(());

    let (captured_sender, captured_res) = (sender.clone(), resource.clone());
    sender
        .after("k", Duration::from_secs(60), move |_: &mut St| {
            let _ = (&captured_sender, &captured_res);
        })
        .expect("積める");
    assert_eq!(std::sync::Arc::strong_count(&resource), 2);

    drop(app);
    assert_eq!(
        std::sync::Arc::strong_count(&resource),
        1,
        "アプリの破棄で予約と捕まえた資源が解放される"
    );
    assert_eq!(
        sender.after("again", Duration::from_secs(1), |_: &mut St| {}),
        Err(fandhe_runtime::TimerRefused::Closed)
    );
}
