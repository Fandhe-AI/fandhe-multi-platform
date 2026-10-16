//! Phase 11（親 #89）型サンプル「機械学習モデル推論」（Issue #289）の effect ドライバ。
//! `fandhe_core::screens::ml_infer`（端末依存ゼロ）の要求を観測し、ワーカースレッドで
//! `fandhe_device::ml_inference::MlInference` を呼んで `CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・期限・守る範囲）の SSOT は
//! `fandhe_device::ml_inference`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web は対象外。
//!
//! # OS 別の分担（dispatch は #289 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: [`super::apple_ml_inference::AppleMlInference`]（#289）
//! - iOS: `ios_ml_inference`（b-ios）
//! - Windows: `windows_ml_inference`（windows の子。WinML）
//! - Android: `android_ml_inference`（b-android。NNAPI）
//! - それ以外（Linux）: `UnsupportedMlInference` 固定（ダミーの成功実装へは倒さない）
//!
//! # 期限（契約表 3）
//!
//! 期限は本ドライバだけが持つ。起点は要求を観測した時の `Instant`（単調時計）。毎フレーム、
//! 処理中でセッションの世代が一致するとき `elapsed >= deadline` なら `Timeout` を確定して
//! セッションを空にし、まだなら `request_repaint_after` で次の判定を予約する。確定後に
//! ワーカーが返す結果は画面側の世代・状態照合（`finish_infer`）で捨てられる。ワーカー自身も
//! 期限超過を `Timeout` にし、期限後に届いた成功を成功として扱わない。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::ml_infer::{
    finish_infer, mark_running, take_request, InferFailure, MlInferPhase, MlInferState,
    MlModelChoice,
};
use fandhe_device::ml_inference::{
    check_output, expected, validate_input, DemoModel, MlInference, DEMO_INPUT, INFER_DEADLINE,
    INVALID_MODEL, MISMATCH, TIMEOUT,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `MlInference` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn MlInference> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::apple_ml_inference::AppleMlInference)
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_ml_inference::implementation()
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_ml_inference::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_ml_inference::implementation()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        Arc::new(fandhe_device::ml_inference::UnsupportedMlInference)
    }
}

/// ドライバの可変状態。テストが並列で干渉しないよう static から切り出して引数で渡す。
/// 進行中の要求の `(世代, 受付時刻)`。
#[derive(Default)]
struct Session {
    inflight: Mutex<Option<(u64, Instant)>>,
}

static SESSION: std::sync::OnceLock<Session> = std::sync::OnceLock::new();

/// `DeviceError` を画面の失敗種別へ写す。内部識別子・OS コードは画面にもログにも出さず、
/// 上記以外は固定文言にする（security.md A09）。
pub(crate) fn to_failure(e: DeviceError) -> InferFailure {
    match e {
        DeviceError::Unsupported(_) => InferFailure::Unsupported,
        DeviceError::Internal(INVALID_MODEL) => InferFailure::InvalidModel,
        DeviceError::Internal(TIMEOUT) => InferFailure::Timeout,
        DeviceError::Internal(MISMATCH) => InferFailure::Mismatch,
        _ => InferFailure::Failed("推論できませんでした".to_string()),
    }
}

/// ワーカーの本体: 入力検査 → 推論 → 期限・壊れたモデル・出力の照合。
fn run_worker(
    imp: &dyn MlInference,
    choice: MlModelChoice,
    started: Instant,
    deadline: Duration,
) -> Result<(f64, f64, Duration), InferFailure> {
    let model = match choice {
        MlModelChoice::Valid => DemoModel::Valid,
        MlModelChoice::Corrupt => DemoModel::Corrupt,
    };
    validate_input(DEMO_INPUT).map_err(to_failure)?;
    let t0 = Instant::now();
    let result = imp.infer(
        model,
        DEMO_INPUT,
        deadline.saturating_sub(started.elapsed()),
    );
    let elapsed = t0.elapsed();
    // 期限後に届いた結果は成功にしない。
    if started.elapsed() > deadline {
        return Err(InferFailure::Timeout);
    }
    let y = result.map_err(to_failure)?;
    if model == DemoModel::Corrupt {
        // 壊れたモデルを OS が受け付けたのは契約違反。成功に見せない。
        return Err(InferFailure::Failed(
            "壊れたモデルが受け付けられました".to_string(),
        ));
    }
    let exp = expected(DEMO_INPUT);
    let y = check_output(y, exp).map_err(to_failure)?;
    Ok((y, exp, elapsed))
}

/// 1 フレームぶんの処理。要求の発送と期限判定を行う。
fn drive(
    state: &MlInferState,
    sender: &CompletionSender<MlInferState>,
    imp: &Arc<dyn MlInference>,
    session: &Session,
    deadline: Duration,
) {
    // 要求の観測と記録は同じロックの内側で行い、二重発送を防ぐ。
    let new_request = {
        let mut g = session.inflight.lock().unwrap_or_else(|e| e.into_inner());
        match take_request(state) {
            Some((generation, choice)) if g.map(|(gen, _)| gen) != Some(generation) => {
                let started = Instant::now();
                *g = Some((generation, started));
                Some((generation, choice, started))
            }
            _ => {
                // 期限判定: 処理中でセッションの世代が一致するときだけ。
                if let (Some((gen, started)), MlInferPhase::Running) = (*g, &state.phase) {
                    if gen == state.generation {
                        let elapsed = started.elapsed();
                        if elapsed >= deadline {
                            *g = None;
                            let _ = sender.send(Box::new(move |s: &mut MlInferState| {
                                finish_infer(s, gen, Err(InferFailure::Timeout));
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
    let Some((generation, choice, started)) = new_request else {
        return;
    };
    let _ = sender.send(Box::new(mark_running));
    sender.request_repaint_after(deadline);

    let worker_sender = sender.clone();
    let worker_imp = imp.clone();
    let spawned = std::thread::Builder::new()
        .name("ml-inference".into())
        .spawn(move || {
            let result = run_worker(&*worker_imp, choice, started, deadline);
            let _ = worker_sender.send(Box::new(move |s: &mut MlInferState| {
                finish_infer(s, generation, result);
            }));
        });
    if spawned.is_err() {
        let _ = sender.send(Box::new(move |s: &mut MlInferState| {
            // `mark_running` の後に適用されるよう同じキューへ積む。
            finish_infer(
                s,
                generation,
                Err(InferFailure::Failed("推論できませんでした".to_string())),
            );
        }));
    }
}

/// `registry.rs` の `effect` 区画から `ml_infer` 画面用に配線される effect ドライバ。
pub fn ml_infer_effect(state: &MlInferState, sender: &CompletionSender<MlInferState>) {
    static IMPL: std::sync::OnceLock<Arc<dyn MlInference>> = std::sync::OnceLock::new();
    let imp = IMPL.get_or_init(platform);
    drive(
        state,
        sender,
        imp,
        SESSION.get_or_init(Session::default),
        INFER_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::ml_infer::{request_infer, MlInferPhase};
    use fandhe_device::ml_inference::UnsupportedMlInference;
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<MlInferState>>;

    fn channel() -> (CompletionSender<MlInferState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 指定の結果を返し、`sleep` だけ待つフェイク。
    struct Fake {
        result: fn() -> Result<f64, DeviceError>,
        sleep: Duration,
    }
    impl MlInference for Fake {
        fn infer(&self, _: DemoModel, _: [f32; 2], _: Duration) -> Result<f64, DeviceError> {
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<f64, DeviceError>) -> Arc<dyn MlInference> {
        Arc::new(Fake {
            result,
            sleep: Duration::ZERO,
        })
    }

    /// 要求を立てて 1 フレーム回し、届いた完了を `n` 件適用した状態を返す。
    fn run(imp: Arc<dyn MlInference>, choice: MlModelChoice, n: usize) -> MlInferState {
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = MlInferState::default();
        request_infer(&mut state, choice);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        for _ in 0..n {
            let c = rx.recv_timeout(Duration::from_secs(5)).expect("完了が届く");
            c(&mut state);
        }
        state
    }

    fn failed(s: &MlInferState) -> &InferFailure {
        match &s.phase {
            MlInferPhase::Failed(f) => f,
            p => panic!("失敗のはず: {p:?}"),
        }
    }

    #[test]
    fn no_request_does_nothing() {
        let (sender, rx) = channel();
        let imp = fake(|| Ok(24.0));
        drive(
            &MlInferState::default(),
            &sender,
            &imp,
            &Session::default(),
            Duration::from_secs(1),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_fills_output_expected_elapsed() {
        let s = run(fake(|| Ok(24.0)), MlModelChoice::Valid, 2);
        match s.phase {
            MlInferPhase::Done { y, expected, .. } => {
                assert_eq!((y, expected), (24.0, 24.0));
            }
            p => panic!("成功のはず: {p:?}"),
        }
    }

    #[test]
    fn unsupported_is_unsupported() {
        let s = run(Arc::new(UnsupportedMlInference), MlModelChoice::Valid, 2);
        assert_eq!(failed(&s), &InferFailure::Unsupported);
    }

    #[test]
    fn invalid_model_error_maps() {
        let s = run(
            fake(|| Err(DeviceError::Internal(INVALID_MODEL))),
            MlModelChoice::Corrupt,
            2,
        );
        assert_eq!(failed(&s), &InferFailure::InvalidModel);
    }

    #[test]
    fn wrong_output_is_mismatch_not_success() {
        for f in [
            (|| Ok(0.0)) as fn() -> Result<f64, DeviceError>,
            || Ok(f64::NAN),
            || Ok(f64::INFINITY),
        ] {
            let s = run(fake(f), MlModelChoice::Valid, 2);
            assert_eq!(failed(&s), &InferFailure::Mismatch);
        }
    }

    #[test]
    fn corrupt_model_accepted_is_failure() {
        let s = run(fake(|| Ok(24.0)), MlModelChoice::Corrupt, 2);
        assert!(matches!(failed(&s), InferFailure::Failed(_)));
    }

    #[test]
    fn internal_ids_never_reach_screen_text() {
        let s = run(
            fake(|| Err(DeviceError::Internal("secret os detail"))),
            MlModelChoice::Valid,
            2,
        );
        let text = fandhe_core::screens::ml_infer::error_text(&s);
        assert!(!text.contains("secret"), "{text}");
    }

    #[test]
    fn worker_past_deadline_is_timeout() {
        let imp: Arc<dyn MlInference> = Arc::new(Fake {
            result: || Ok(24.0),
            sleep: Duration::from_millis(60),
        });
        let (sender, rx) = channel();
        let mut state = MlInferState::default();
        request_infer(&mut state, MlModelChoice::Valid);
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
        assert_eq!(failed(&state), &InferFailure::Timeout);
    }

    #[test]
    fn frame_deadline_times_out_and_late_worker_result_is_ignored() {
        let imp: Arc<dyn MlInference> = Arc::new(Fake {
            result: || Ok(24.0),
            sleep: Duration::from_millis(300),
        });
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = MlInferState::default();
        request_infer(&mut state, MlModelChoice::Valid);
        let deadline = Duration::from_millis(30);
        drive(&state, &sender, &imp, &session, deadline);
        // mark_running を適用
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        std::thread::sleep(Duration::from_millis(60));
        drive(&state, &sender, &imp, &session, deadline);
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failed(&state), &InferFailure::Timeout);
        // 遅れて届いたワーカーの結果は無視される
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failed(&state), &InferFailure::Timeout);
    }

    #[test]
    fn stale_generation_completion_is_ignored() {
        let mut state = MlInferState::default();
        request_infer(&mut state, MlModelChoice::Valid);
        mark_running(&mut state);
        finish_infer(&mut state, 99, Ok((24.0, 24.0, Duration::ZERO)));
        assert_eq!(state.phase, MlInferPhase::Running);
    }

    #[test]
    fn double_drive_before_mark_applied_runs_once() {
        let (sender, rx) = channel();
        let imp = fake(|| Ok(24.0));
        let session = Session::default();
        let mut state = MlInferState::default();
        request_infer(&mut state, MlModelChoice::Valid);
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
    fn fallback_platform_is_unsupported_on_linux() {
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "windows",
            target_os = "android"
        )))]
        assert!(matches!(
            platform().infer(DemoModel::Valid, DEMO_INPUT, Duration::ZERO),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
