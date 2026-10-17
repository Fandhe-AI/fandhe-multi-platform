//! Phase 11（親 #93）型サンプル「端末内 LLM」（Issue #345）の effect ドライバ。
//! `fandhe_core::screens::on_device_llm`（端末依存ゼロ）の要求を観測し、ワーカースレッドで
//! `fandhe_device::on_device_llm::OnDeviceLlm` を呼んで `CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・期限・守る範囲）の SSOT は
//! `fandhe_device::on_device_llm`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web の実体は
//! `fandhe-web-dom` の非同期 effect（c の子 issue）が持つ。
//!
//! # OS 別の分担（dispatch は #345 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: [`super::apple_on_device_llm::AppleOnDeviceLlm`]（#345）
//! - iOS: `ios_on_device_llm`（b-ios。FoundationModels）
//! - Web: `fandhe-web-dom`（c。Prompt API）
//! - Android・Windows: 保留・申請待ちのため `UnsupportedOnDeviceLlm` 固定
//!   （ダミーの成功実装へは倒さない）
//!
//! # 期限（契約表 3）
//!
//! 期限は本ドライバだけが持つ。起点は要求を観測した時の `Instant`（単調時計）。毎フレーム、
//! 処理中でセッションの世代が一致するとき `elapsed >= deadline` なら `Timeout` を確定して
//! セッションを空にし、まだなら `request_repaint_after` で次の判定を予約する。確定後に
//! ワーカーが返す結果は画面側の世代・状態照合（`finish_respond`）で捨てられる。ワーカー自身も
//! 期限超過を `Timeout` にし、期限後に届いた成功を成功として扱わない。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::on_device_llm::{
    finish_respond, mark_running, take_request, LlmFailure, LlmPhase, OnDeviceLlmState, PROMPT,
};
use fandhe_device::on_device_llm::{
    check_response, validate_prompt, OnDeviceLlm, BUSY, EMPTY, MODEL_NOT_READY, REFUSED,
    RESPOND_DEADLINE, TIMEOUT,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `OnDeviceLlm` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn OnDeviceLlm> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::apple_on_device_llm::AppleOnDeviceLlm)
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_on_device_llm::implementation()
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        Arc::new(fandhe_device::on_device_llm::UnsupportedOnDeviceLlm)
    }
}

/// ドライバの可変状態。テストが並列で干渉しないよう static から切り出して引数で渡す。
/// 進行中の要求の `(世代, 受付時刻)`。
#[derive(Default)]
struct Session {
    inflight: Mutex<Option<(u64, Instant)>>,
}

static SESSION: std::sync::OnceLock<Session> = std::sync::OnceLock::new();

/// Activity 再生成時（`android_main` の再実行）に、ドライバの処理済み記録を破棄する。
/// 画面状態は `default()`（世代 0）に戻るため、完了後も残る記録（世代 1）を残すと次の
/// 「質問する」が発送されず飲み込まれる。古いワーカーの結果は古いアプリのチャネルへ届くので
/// 新しい画面には適用されない。走っている OS 呼び出しは止めず走り切らせる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        *s.inflight.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// `DeviceError` を画面の失敗種別へ写す。内部識別子・OS コード・OS の文言は画面にもログにも出さず、
/// 上記以外は固定文言にする（security.md A09）。
pub(crate) fn to_failure(e: DeviceError) -> LlmFailure {
    match e {
        DeviceError::Unsupported(_) => LlmFailure::Unsupported,
        DeviceError::Disabled(_) => LlmFailure::Disabled,
        DeviceError::PermissionDenied(_) => LlmFailure::PermissionDenied,
        DeviceError::Internal(MODEL_NOT_READY) => LlmFailure::ModelNotReady,
        DeviceError::Internal(REFUSED) => LlmFailure::Refused,
        DeviceError::Internal(BUSY) => LlmFailure::Busy,
        DeviceError::Internal(TIMEOUT) => LlmFailure::Timeout,
        DeviceError::Internal(EMPTY) => LlmFailure::Empty,
        _ => LlmFailure::Failed("応答を得られませんでした".to_string()),
    }
}

/// ワーカーの本体: 入力検査 → 応答 → 期限・空応答の照合。
fn run_worker(
    imp: &dyn OnDeviceLlm,
    started: Instant,
    deadline: Duration,
) -> Result<String, LlmFailure> {
    validate_prompt(PROMPT).map_err(to_failure)?;
    let result = imp.respond(PROMPT, deadline.saturating_sub(started.elapsed()));
    // 期限後に届いた結果は成功にしない。
    if started.elapsed() > deadline {
        return Err(LlmFailure::Timeout);
    }
    check_response(result.map_err(to_failure)?).map_err(to_failure)
}

/// 1 フレームぶんの処理。要求の発送と期限判定を行う。
fn drive(
    state: &OnDeviceLlmState,
    sender: &CompletionSender<OnDeviceLlmState>,
    imp: &Arc<dyn OnDeviceLlm>,
    session: &Session,
    deadline: Duration,
) {
    // 要求の観測と記録は同じロックの内側で行い、二重発送を防ぐ。
    let new_request = {
        let mut g = session.inflight.lock().unwrap_or_else(|e| e.into_inner());
        match take_request(state) {
            Some(generation) if g.map(|(gen, _)| gen) != Some(generation) => {
                let started = Instant::now();
                *g = Some((generation, started));
                Some((generation, started))
            }
            _ => {
                // 期限判定: 処理中でセッションの世代が一致するときだけ。
                if let (Some((gen, started)), LlmPhase::Running) = (*g, &state.phase) {
                    if gen == state.generation {
                        let elapsed = started.elapsed();
                        if elapsed >= deadline {
                            *g = None;
                            let _ = sender.send(Box::new(move |s: &mut OnDeviceLlmState| {
                                finish_respond(s, gen, Err(LlmFailure::Timeout));
                            }));
                        } else {
                            sender.request_repaint_after(deadline - elapsed);
                        }
                    }
                }
                None
            }
        }
    };
    let Some((generation, started)) = new_request else {
        return;
    };
    let _ = sender.send(Box::new(mark_running));
    sender.request_repaint_after(deadline);

    let worker_sender = sender.clone();
    let worker_imp = imp.clone();
    let spawned = std::thread::Builder::new()
        .name("on-device-llm".into())
        .spawn(move || {
            let result = run_worker(&*worker_imp, started, deadline);
            let _ = worker_sender.send(Box::new(move |s: &mut OnDeviceLlmState| {
                finish_respond(s, generation, result);
            }));
        });
    if spawned.is_err() {
        let _ = sender.send(Box::new(move |s: &mut OnDeviceLlmState| {
            // `mark_running` の後に適用されるよう同じキューへ積む。
            finish_respond(
                s,
                generation,
                Err(LlmFailure::Failed("応答を得られませんでした".to_string())),
            );
        }));
    }
}

/// `registry.rs` の `effect` 区画から `on_device_llm` 画面用に配線される effect ドライバ。
pub fn on_device_llm_effect(state: &OnDeviceLlmState, sender: &CompletionSender<OnDeviceLlmState>) {
    static IMPL: std::sync::OnceLock<Arc<dyn OnDeviceLlm>> = std::sync::OnceLock::new();
    let imp = IMPL.get_or_init(platform);
    drive(
        state,
        sender,
        imp,
        SESSION.get_or_init(Session::default),
        RESPOND_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::on_device_llm::request_respond;
    use fandhe_device::on_device_llm::UnsupportedOnDeviceLlm;
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<OnDeviceLlmState>>;

    fn channel() -> (CompletionSender<OnDeviceLlmState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 指定の結果を返し、`sleep` だけ待つフェイク。
    struct Fake {
        result: fn() -> Result<String, DeviceError>,
        sleep: Duration,
    }
    impl OnDeviceLlm for Fake {
        fn respond(&self, _: &str, _: Duration) -> Result<String, DeviceError> {
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<String, DeviceError>) -> Arc<dyn OnDeviceLlm> {
        Arc::new(Fake {
            result,
            sleep: Duration::ZERO,
        })
    }

    /// 要求を立てて 1 フレーム回し、届いた完了を `n` 件適用した状態を返す。
    fn run(imp: Arc<dyn OnDeviceLlm>, n: usize) -> OnDeviceLlmState {
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = OnDeviceLlmState::default();
        request_respond(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        for _ in 0..n {
            let c = rx.recv_timeout(Duration::from_secs(5)).expect("完了が届く");
            c(&mut state);
        }
        state
    }

    fn failed(s: &OnDeviceLlmState) -> &LlmFailure {
        match &s.phase {
            LlmPhase::Failed(f) => f,
            p => panic!("失敗のはず: {p:?}"),
        }
    }

    #[test]
    fn no_request_does_nothing() {
        let (sender, rx) = channel();
        let imp = fake(|| Ok("春".into()));
        drive(
            &OnDeviceLlmState::default(),
            &sender,
            &imp,
            &Session::default(),
            Duration::from_secs(1),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_shows_response() {
        let s = run(fake(|| Ok("春夏秋冬".into())), 2);
        assert_eq!(
            s.phase,
            LlmPhase::Done {
                text: "春夏秋冬".into()
            }
        );
    }

    #[test]
    fn errors_map_to_failures() {
        type R = fn() -> Result<String, DeviceError>;
        let cases: [(R, LlmFailure); 7] = [
            (
                || Err(DeviceError::Unsupported("on_device_llm")),
                LlmFailure::Unsupported,
            ),
            (
                || Err(DeviceError::Disabled("on_device_llm")),
                LlmFailure::Disabled,
            ),
            (
                || Err(DeviceError::Internal(MODEL_NOT_READY)),
                LlmFailure::ModelNotReady,
            ),
            (|| Err(DeviceError::Internal(REFUSED)), LlmFailure::Refused),
            (|| Err(DeviceError::Internal(BUSY)), LlmFailure::Busy),
            (
                || Err(DeviceError::PermissionDenied("on_device_llm")),
                LlmFailure::PermissionDenied,
            ),
            (
                || Err(DeviceError::InvalidInput("on_device_llm prompt")),
                LlmFailure::Failed("応答を得られませんでした".into()),
            ),
        ];
        for (f, expect) in cases {
            let s = run(fake(f), 2);
            assert_eq!(failed(&s), &expect);
        }
    }

    #[test]
    fn blank_response_is_empty_not_success() {
        for f in [
            (|| Ok(String::new())) as fn() -> Result<String, DeviceError>,
            || Ok("  \n".into()),
        ] {
            let s = run(fake(f), 2);
            assert_eq!(failed(&s), &LlmFailure::Empty);
        }
    }

    #[test]
    fn internal_ids_never_reach_screen_text() {
        let s = run(fake(|| Err(DeviceError::Internal("secret os detail"))), 2);
        let text = fandhe_core::screens::on_device_llm::error_text(&s);
        assert!(
            !text.contains("secret") && !text.contains("on_device_llm"),
            "{text}"
        );
    }

    #[test]
    fn worker_past_deadline_is_timeout() {
        let imp: Arc<dyn OnDeviceLlm> = Arc::new(Fake {
            result: || Ok("春".into()),
            sleep: Duration::from_millis(60),
        });
        let (sender, rx) = channel();
        let mut state = OnDeviceLlmState::default();
        request_respond(&mut state);
        drive(
            &state,
            &sender,
            &imp,
            &Session::default(),
            Duration::from_millis(20),
        );
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        }
        assert_eq!(failed(&state), &LlmFailure::Timeout);
    }

    #[test]
    fn frame_deadline_times_out_and_late_worker_result_is_ignored() {
        let imp: Arc<dyn OnDeviceLlm> = Arc::new(Fake {
            result: || Ok("春".into()),
            sleep: Duration::from_millis(300),
        });
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = OnDeviceLlmState::default();
        request_respond(&mut state);
        let deadline = Duration::from_millis(30);
        drive(&state, &sender, &imp, &session, deadline);
        // mark_running を適用
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        std::thread::sleep(Duration::from_millis(60));
        drive(&state, &sender, &imp, &session, deadline);
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failed(&state), &LlmFailure::Timeout);
        // 遅れて届いたワーカーの結果は無視される
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failed(&state), &LlmFailure::Timeout);
    }

    #[test]
    fn stale_generation_completion_is_ignored() {
        let mut state = OnDeviceLlmState::default();
        request_respond(&mut state);
        mark_running(&mut state);
        finish_respond(&mut state, 99, Ok("春".into()));
        assert_eq!(state.phase, LlmPhase::Running);
    }

    #[test]
    fn double_drive_before_mark_applied_runs_once() {
        let (sender, rx) = channel();
        let imp = fake(|| Ok("春".into()));
        let session = Session::default();
        let mut state = OnDeviceLlmState::default();
        request_respond(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(100));
        // mark_running 1 + finish 1
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 2);
    }

    #[test]
    fn reset_lets_a_fresh_screen_dispatch_again() {
        let (sender, rx) = channel();
        let imp = fake(|| Ok("春".into()));
        let session = Session::default();
        *session.inflight.lock().unwrap() = Some((1, Instant::now()));
        let mut state = OnDeviceLlmState::default();
        request_respond(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.try_recv().is_err(), "記録が残ると飲み込まれる");
        *session.inflight.lock().unwrap() = None;
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn unsupported_impl_is_unsupported() {
        let s = run(Arc::new(UnsupportedOnDeviceLlm), 2);
        assert_eq!(failed(&s), &LlmFailure::Unsupported);
    }

    #[test]
    fn fallback_platform_is_unsupported_on_linux() {
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        assert!(matches!(
            platform().respond("x", Duration::ZERO),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
