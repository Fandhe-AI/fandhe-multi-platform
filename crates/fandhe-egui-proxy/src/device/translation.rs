//! Phase 11（親 #115）型サンプル「端末内翻訳」（Issue #535）の effect ドライバ。
//! `fandhe_core::screens::translation`（端末依存ゼロ）の要求を観測し、ワーカースレッドで
//! `fandhe_device::translation::TextTranslator` を呼んで `CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・期限・守る範囲）の SSOT は
//! `fandhe_device::translation`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web の実体は
//! `fandhe-web-dom` の非同期 effect（#539）が持つ。
//!
//! # OS 別の分担（dispatch は #535 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: [`super::apple_translation::AppleTextTranslator`]（#535）
//! - iOS: `ios_translation`（#536。macOS と共用の `AppleTextTranslator` へ差し替える）
//! - Android: `android_translation`（#537。`android.view.translation`）
//! - Web: `fandhe-web-dom`（#539。Translator API）
//! - Windows・Linux: `UnsupportedTextTranslator` 固定（ダミーの成功実装へは倒さない）
//!
//! # 期限（契約表 3）
//!
//! 期限は本ドライバだけが持つ。起点は要求を観測した時の `Instant`（単調時計）。毎フレーム、
//! 処理中でセッションの世代が一致するとき `elapsed >= deadline` なら `Timeout` を確定して
//! セッションを空にし、まだなら `request_repaint_after` で次の判定を予約する。確定後に
//! ワーカーが返す結果は画面側の世代・状態照合（`finish_translate`）で捨てられる。ワーカー自身も
//! 期限超過を `Timeout` にし、期限後に届いた成功を成功として扱わない。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::translation::{
    finish_translate, mark_running, take_request, TranslateFailure, TranslatePhase,
    TranslationState, SOURCE_LANG, SOURCE_TEXT,
};
use fandhe_device::translation::{
    check_result, validate_request, TextTranslator, EMPTY, LANGUAGE_UNSUPPORTED, NOT_INSTALLED,
    TIMEOUT, TRANSLATE_DEADLINE,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

const FAILED_MESSAGE: &str = "翻訳できませんでした";

/// 現在のプラットフォームに応じた `TextTranslator` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn TextTranslator> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::apple_translation::AppleTextTranslator)
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_translation::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_translation::implementation()
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    {
        Arc::new(fandhe_device::translation::UnsupportedTextTranslator)
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
/// 「英語に訳す」が発送されず飲み込まれる。古いワーカーの結果は古いアプリのチャネルへ届くので
/// 新しい画面には適用されない。走っている OS 呼び出しは止めず走り切らせる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        *s.inflight.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// `DeviceError` を画面の失敗種別へ写す。内部識別子・OS コード・OS の文言は画面にもログにも出さず、
/// 上記以外は固定文言にする（security.md A09）。
pub(crate) fn to_failure(e: DeviceError) -> TranslateFailure {
    match e {
        DeviceError::Unsupported(_) => TranslateFailure::Unsupported,
        DeviceError::PermissionDenied(_) => TranslateFailure::PermissionDenied,
        DeviceError::Internal(LANGUAGE_UNSUPPORTED) => TranslateFailure::LanguageUnsupported,
        DeviceError::Internal(NOT_INSTALLED) => TranslateFailure::NotInstalled,
        DeviceError::Internal(TIMEOUT) => TranslateFailure::Timeout,
        DeviceError::Internal(EMPTY) => TranslateFailure::Empty,
        _ => TranslateFailure::Failed(FAILED_MESSAGE.to_string()),
    }
}

/// ワーカーの本体: 入力検査 → 翻訳 → 期限・空訳文の照合。
fn run_worker(
    imp: &dyn TextTranslator,
    target: &str,
    started: Instant,
    deadline: Duration,
) -> Result<String, TranslateFailure> {
    validate_request(SOURCE_TEXT, SOURCE_LANG, target).map_err(to_failure)?;
    let result = imp.translate(
        SOURCE_TEXT,
        SOURCE_LANG,
        target,
        deadline.saturating_sub(started.elapsed()),
    );
    // 期限後に届いた結果は成功にしない。
    if started.elapsed() > deadline {
        return Err(TranslateFailure::Timeout);
    }
    check_result(result.map_err(to_failure)?).map_err(to_failure)
}

/// 1 フレームぶんの処理。要求の発送と期限判定を行う。
fn drive(
    state: &TranslationState,
    sender: &CompletionSender<TranslationState>,
    imp: &Arc<dyn TextTranslator>,
    session: &Session,
    deadline: Duration,
) {
    // 要求の観測と記録は同じロックの内側で行い、二重発送を防ぐ。
    let new_request = {
        let mut g = session.inflight.lock().unwrap_or_else(|e| e.into_inner());
        match take_request(state) {
            Some((generation, target)) if g.map(|(gen, _)| gen) != Some(generation) => {
                let started = Instant::now();
                *g = Some((generation, started));
                Some((generation, target, started))
            }
            _ => {
                // 期限判定: 処理中でセッションの世代が一致するときだけ。
                if let (Some((gen, started)), TranslatePhase::Running) = (*g, &state.phase) {
                    if gen == state.generation {
                        let elapsed = started.elapsed();
                        if elapsed >= deadline {
                            *g = None;
                            let _ = sender.send(Box::new(move |s: &mut TranslationState| {
                                finish_translate(s, gen, Err(TranslateFailure::Timeout));
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
    let Some((generation, target, started)) = new_request else {
        return;
    };
    let _ = sender.send(Box::new(mark_running));
    sender.request_repaint_after(deadline);

    let worker_sender = sender.clone();
    let worker_imp = imp.clone();
    let spawned = std::thread::Builder::new()
        .name("translation".into())
        .spawn(move || {
            let result = run_worker(&*worker_imp, target, started, deadline);
            let _ = worker_sender.send(Box::new(move |s: &mut TranslationState| {
                finish_translate(s, generation, result);
            }));
        });
    if spawned.is_err() {
        let _ = sender.send(Box::new(move |s: &mut TranslationState| {
            // `mark_running` の後に適用されるよう同じキューへ積む。
            finish_translate(
                s,
                generation,
                Err(TranslateFailure::Failed(FAILED_MESSAGE.to_string())),
            );
        }));
    }
}

/// `registry.rs` の `effect` 区画から `translation` 画面用に配線される effect ドライバ。
pub fn translation_effect(state: &TranslationState, sender: &CompletionSender<TranslationState>) {
    static IMPL: std::sync::OnceLock<Arc<dyn TextTranslator>> = std::sync::OnceLock::new();
    let imp = IMPL.get_or_init(platform);
    drive(
        state,
        sender,
        imp,
        SESSION.get_or_init(Session::default),
        TRANSLATE_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::translation::{request_translate, TargetLang};
    use fandhe_device::translation::UnsupportedTextTranslator;
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<TranslationState>>;

    fn channel() -> (CompletionSender<TranslationState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 指定の結果を返し、`sleep` だけ待つフェイク。受け取った訳先を記録する。
    struct Fake {
        result: fn() -> Result<String, DeviceError>,
        sleep: Duration,
        seen: Mutex<Vec<String>>,
    }
    impl TextTranslator for Fake {
        fn translate(
            &self,
            _: &str,
            _: &str,
            target: &str,
            _: Duration,
        ) -> Result<String, DeviceError> {
            self.seen.lock().unwrap().push(target.to_string());
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn fake_with(result: fn() -> Result<String, DeviceError>, sleep: Duration) -> Arc<Fake> {
        Arc::new(Fake {
            result,
            sleep,
            seen: Mutex::new(Vec::new()),
        })
    }

    fn fake(result: fn() -> Result<String, DeviceError>) -> Arc<dyn TextTranslator> {
        fake_with(result, Duration::ZERO)
    }

    /// 要求を立てて 1 フレーム回し、届いた完了を `n` 件適用した状態を返す。
    fn run(imp: Arc<dyn TextTranslator>, n: usize) -> TranslationState {
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = TranslationState::default();
        request_translate(&mut state, TargetLang::English);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        for _ in 0..n {
            let c = rx.recv_timeout(Duration::from_secs(5)).expect("完了が届く");
            c(&mut state);
        }
        state
    }

    fn failed(s: &TranslationState) -> &TranslateFailure {
        match &s.phase {
            TranslatePhase::Failed(f) => f,
            p => panic!("失敗のはず: {p:?}"),
        }
    }

    #[test]
    fn no_request_does_nothing() {
        let (sender, rx) = channel();
        let imp = fake(|| Ok("Hi".into()));
        drive(
            &TranslationState::default(),
            &sender,
            &imp,
            &Session::default(),
            Duration::from_secs(1),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_shows_translation() {
        let s = run(fake(|| Ok("Nice weather today.".into())), 2);
        assert_eq!(
            s.phase,
            TranslatePhase::Done {
                text: "Nice weather today.".into()
            }
        );
    }

    #[test]
    fn chosen_target_reaches_implementation() {
        let f = fake_with(|| Ok("x".into()), Duration::ZERO);
        let imp: Arc<dyn TextTranslator> = f.clone();
        let (sender, rx) = channel();
        let mut state = TranslationState::default();
        request_translate(&mut state, TargetLang::Unsupported);
        drive(
            &state,
            &sender,
            &imp,
            &Session::default(),
            Duration::from_secs(30),
        );
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        }
        assert_eq!(*f.seen.lock().unwrap(), vec!["zz".to_string()]);
    }

    #[test]
    fn errors_map_to_failures() {
        type R = fn() -> Result<String, DeviceError>;
        let cases: [(R, TranslateFailure); 8] = [
            (
                || Err(DeviceError::Unsupported("translation")),
                TranslateFailure::Unsupported,
            ),
            (
                || Err(DeviceError::Internal(LANGUAGE_UNSUPPORTED)),
                TranslateFailure::LanguageUnsupported,
            ),
            (
                || Err(DeviceError::Internal(NOT_INSTALLED)),
                TranslateFailure::NotInstalled,
            ),
            (
                || Err(DeviceError::Internal(TIMEOUT)),
                TranslateFailure::Timeout,
            ),
            (
                || Err(DeviceError::Internal(EMPTY)),
                TranslateFailure::Empty,
            ),
            (
                || Err(DeviceError::PermissionDenied("translation")),
                TranslateFailure::PermissionDenied,
            ),
            (
                || Err(DeviceError::InvalidInput("translation lang")),
                TranslateFailure::Failed(FAILED_MESSAGE.into()),
            ),
            (
                || Err(DeviceError::Internal("translation")),
                TranslateFailure::Failed(FAILED_MESSAGE.into()),
            ),
        ];
        for (f, expect) in cases {
            let s = run(fake(f), 2);
            assert_eq!(failed(&s), &expect);
        }
    }

    #[test]
    fn blank_translation_is_empty_not_success() {
        for f in [
            (|| Ok(String::new())) as fn() -> Result<String, DeviceError>,
            || Ok("  \n".into()),
        ] {
            let s = run(fake(f), 2);
            assert_eq!(failed(&s), &TranslateFailure::Empty);
        }
    }

    #[test]
    fn internal_ids_never_reach_screen_text() {
        let s = run(fake(|| Err(DeviceError::Internal("secret os detail"))), 2);
        let text = fandhe_core::screens::translation::error_text(&s);
        assert!(
            !text.contains("secret") && !text.contains("translation "),
            "{text}"
        );
    }

    #[test]
    fn worker_past_deadline_is_timeout() {
        let imp: Arc<dyn TextTranslator> = fake_with(|| Ok("Hi".into()), Duration::from_millis(60));
        let (sender, rx) = channel();
        let mut state = TranslationState::default();
        request_translate(&mut state, TargetLang::English);
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
        assert_eq!(failed(&state), &TranslateFailure::Timeout);
    }

    #[test]
    fn frame_deadline_times_out_and_late_worker_result_is_ignored() {
        let imp: Arc<dyn TextTranslator> =
            fake_with(|| Ok("Hi".into()), Duration::from_millis(300));
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = TranslationState::default();
        request_translate(&mut state, TargetLang::English);
        let deadline = Duration::from_millis(30);
        drive(&state, &sender, &imp, &session, deadline);
        // mark_running を適用
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        std::thread::sleep(Duration::from_millis(60));
        drive(&state, &sender, &imp, &session, deadline);
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failed(&state), &TranslateFailure::Timeout);
        // 遅れて届いたワーカーの結果は無視される
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failed(&state), &TranslateFailure::Timeout);
    }

    #[test]
    fn stale_generation_completion_is_ignored() {
        let mut state = TranslationState::default();
        request_translate(&mut state, TargetLang::English);
        mark_running(&mut state);
        finish_translate(&mut state, 99, Ok("Hi".into()));
        assert_eq!(state.phase, TranslatePhase::Running);
    }

    #[test]
    fn double_drive_before_mark_applied_runs_once() {
        let (sender, rx) = channel();
        let imp = fake(|| Ok("Hi".into()));
        let session = Session::default();
        let mut state = TranslationState::default();
        request_translate(&mut state, TargetLang::English);
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
        let imp = fake(|| Ok("Hi".into()));
        let session = Session::default();
        *session.inflight.lock().unwrap() = Some((1, Instant::now()));
        let mut state = TranslationState::default();
        request_translate(&mut state, TargetLang::English);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.try_recv().is_err(), "記録が残ると飲み込まれる");
        *session.inflight.lock().unwrap() = None;
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn unsupported_impl_is_unsupported() {
        let s = run(Arc::new(UnsupportedTextTranslator), 2);
        assert_eq!(failed(&s), &TranslateFailure::Unsupported);
    }

    #[test]
    fn fallback_platform_is_unsupported_on_linux() {
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
        assert!(matches!(
            platform().translate("x", "ja", "en", Duration::ZERO),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
