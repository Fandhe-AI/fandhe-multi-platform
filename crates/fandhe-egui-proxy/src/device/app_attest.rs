//! Phase 11（親 #132）型サンプル「端末・アプリの真正性証明」（Issue #677）の effect ドライバ。
//! `fandhe_core::screens::app_attest_show`（端末依存ゼロ）の要求を観測し、ワーカースレッドで
//! `fandhe_device::app_attest::AppAttest` を呼んで `CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・期限・守る範囲）の SSOT は
//! `fandhe_device::app_attest`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # OS 別の分担（dispatch は #677 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - Android: `android_app_attest`（#678。`AndroidKeyStore` の Key Attestation）
//! - iOS: 有料の開発者登録が切れているため deferred（対応表で管理。再開時は別 issue で分岐を足す）
//! - macOS・Windows・Linux・Web: 対応する公式 API が無い（no）ため `UnsupportedAppAttest` 固定
//!   （ダミーの成功実装へは倒さない）
//!
//! # 期限（契約表 3）
//!
//! 期限は本ドライバだけが持つ。起点は要求を観測した時の `Instant`（単調時計）。毎フレーム、
//! 処理中で結果が未着のとき `elapsed >= deadline` なら `Timeout` を確定してスロットを空にし、
//! まだなら `poll_and_finish` の再描画予約（`POLL_INTERVAL`）で次の判定に回る。要求の追跡は
//! `request_slot::RequestSlot`（pull 型）。確定後にワーカーが返す結果は画面側の世代・状態照合
//! （`finish_attest`）で捨てられる。ワーカー自身も期限超過を `Timeout` にし、期限後に届いた成功を
//! 成功として扱わない。
//!
//! # 秘密値（契約表 8）
//!
//! スロットに置くのは件数と合計バイト数だけで、challenge・証明データ本体は保持せずその場で捨てる。
//! 内部識別子や OS の文言は画面にもログにも出さない。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::app_attest_show::{
    finish_attest, mark_running, take_request, AppAttestState, AttestFailure, AttestPhase,
};
use fandhe_device::app_attest::{
    check_result, validate_challenge, AppAttest, ATTEST_DEADLINE, CHALLENGE_LEN, EMPTY, KEYSTORE,
    TIMEOUT, TOO_LARGE,
};
use fandhe_device::DeviceError;

use super::request_slot::{poll_and_finish, RequestSlot, POLL_INTERVAL};
use crate::app::CompletionSender;

const FAILED_MESSAGE: &str = "証明データを取得できませんでした";

/// 取得結果の要約（件数・合計バイト数）。証明データ本体は持たない。
type Summary = (usize, usize);

/// 現在のプラットフォームに応じた `AppAttest` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn AppAttest> {
    #[cfg(target_os = "android")]
    {
        super::android_app_attest::implementation()
    }
    #[cfg(not(target_os = "android"))]
    {
        Arc::new(fandhe_device::app_attest::UnsupportedAppAttest)
    }
}

/// ドライバの可変状態。要求の追跡（二重発送防止・完了照合・再生成時の破棄）は
/// [`RequestSlot`] に任せ、ここは期限の起点だけを持つ。テストが並列で干渉しないよう static から
/// 切り出して引数で渡す（`poll_and_finish` が `'static` を要るためスロットは leak）。
struct Session {
    slot: &'static RequestSlot<Result<Summary, AttestFailure>>,
    /// 進行中の要求を観測した時刻（単調時計）。
    started: Mutex<Option<Instant>>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            slot: Box::leak(Box::new(RequestSlot::new())),
            started: Mutex::new(None),
        }
    }
}

static SESSION: std::sync::OnceLock<Session> = std::sync::OnceLock::new();

/// Activity 再生成時（`android_main` の再実行）に、保留中の要求を破棄する。
/// 画面状態は `default()`（世代 0）に戻るため、保留が残ると次の取得が発送されず飲み込まれる。
/// 古いワーカーの結果は照合で捨てられる。走っている OS 呼び出しは止めず走り切らせる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        s.slot.reset();
        *s.started.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// `DeviceError` を画面の失敗種別へ写す。内部識別子・OS コード・OS の文言は画面にもログにも出さず、
/// 上記以外は固定文言にする（security.md A09）。
pub(crate) fn to_failure(e: DeviceError) -> AttestFailure {
    match e {
        DeviceError::Unsupported(_) => AttestFailure::Unsupported,
        DeviceError::InvalidInput(_) => AttestFailure::InvalidInput,
        DeviceError::Internal(KEYSTORE) => AttestFailure::KeystoreFailed,
        DeviceError::Internal(EMPTY) => AttestFailure::Empty,
        DeviceError::Internal(TOO_LARGE) => AttestFailure::TooLarge,
        DeviceError::Internal(TIMEOUT) => AttestFailure::Timeout,
        _ => AttestFailure::Failed(FAILED_MESSAGE.to_string()),
    }
}

/// ワーカーの本体: challenge 生成 → 検査 → 取得 → 期限・結果の照合。
fn run_worker(
    imp: &dyn AppAttest,
    started: Instant,
    deadline: Duration,
) -> Result<Summary, AttestFailure> {
    let mut buf = [0u8; CHALLENGE_LEN];
    imp.fill_random(&mut buf).map_err(to_failure)?;
    let challenge = validate_challenge(&buf).map_err(to_failure)?;
    let result = imp.attest(&challenge, deadline.saturating_sub(started.elapsed()));
    // 期限後に届いた結果は成功にしない。
    if started.elapsed() > deadline {
        return Err(AttestFailure::Timeout);
    }
    let a = check_result(result.map_err(to_failure)?).map_err(to_failure)?;
    // 証明データ本体はここで捨て、件数とサイズだけを返す。
    Ok((a.count(), a.total_bytes()))
}

/// 1 フレームぶんの処理。要求の発送・期限判定・完了の画面反映を行う。
fn drive(
    state: &AppAttestState,
    sender: &CompletionSender<AppAttestState>,
    imp: &Arc<dyn AppAttest>,
    session: &Session,
    deadline: Duration,
) {
    let slot = session.slot;
    if take_request(state).is_some() {
        // 保留ありは発送済み（`mark_running` の適用待ち）。二重に発送しない。
        if slot.snapshot().is_some() {
            return;
        }
        let request_id = slot.begin();
        let started = Instant::now();
        *session.started.lock().unwrap_or_else(|e| e.into_inner()) = Some(started);
        let _ = sender.send(Box::new(mark_running));
        sender.request_repaint_after(POLL_INTERVAL);

        let worker_imp = imp.clone();
        let spawned = std::thread::Builder::new()
            .name("app-attest".into())
            .spawn(move || {
                let result = run_worker(&*worker_imp, started, deadline);
                slot.deliver(request_id, result);
            });
        if spawned.is_err() {
            slot.deliver(
                request_id,
                Err(AttestFailure::Failed(FAILED_MESSAGE.to_string())),
            );
        }
        return;
    }
    if state.phase != AttestPhase::Running {
        return;
    }
    let generation = state.generation;
    // 期限判定: 単調時計。確定後に届く結果は照合で捨てられる。
    let started = *session.started.lock().unwrap_or_else(|e| e.into_inner());
    if let (Some(started), Some((request_id, None))) = (started, slot.snapshot()) {
        // 判定と除去を原子的に行う。false は deliver が先に着地済み（成功を消さず後続に任せる）。
        if started.elapsed() >= deadline && slot.clear_if_pending(request_id) {
            let _ = sender.send(Box::new(move |s: &mut AppAttestState| {
                finish_attest(s, generation, Err(AttestFailure::Timeout));
            }));
            return;
        }
    }
    poll_and_finish(
        slot,
        sender,
        |_| {},
        move |s: &mut AppAttestState, result| {
            finish_attest(s, generation, result);
            true
        },
    );
}

/// `registry.rs` の `effect` 区画から `app_attest_show` 画面用に配線される effect ドライバ。
pub fn app_attest_show_effect(state: &AppAttestState, sender: &CompletionSender<AppAttestState>) {
    static IMPL: std::sync::OnceLock<Arc<dyn AppAttest>> = std::sync::OnceLock::new();
    let imp = IMPL.get_or_init(platform);
    drive(
        state,
        sender,
        imp,
        SESSION.get_or_init(Session::default),
        ATTEST_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::app_attest_show::request_attest;
    use fandhe_device::app_attest::{Attestation, Challenge, UnsupportedAppAttest};
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<AppAttestState>>;

    fn channel() -> (CompletionSender<AppAttestState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    type AttestResult = fn() -> Result<Attestation, DeviceError>;

    /// 指定の結果を返し、`sleep` だけ待つフェイク。`fill_random` は固定の印の値で埋め、
    /// `attest` が受け取った challenge を記録する。
    struct Fake {
        result: AttestResult,
        random: fn() -> Result<(), DeviceError>,
        sleep: Duration,
        seen: Mutex<Vec<Vec<u8>>>,
    }
    impl AppAttest for Fake {
        fn fill_random(&self, buf: &mut [u8]) -> Result<(), DeviceError> {
            (self.random)()?;
            buf.fill(0x5A);
            Ok(())
        }
        fn attest(&self, c: &Challenge, _: Duration) -> Result<Attestation, DeviceError> {
            self.seen.lock().unwrap().push(c.as_bytes().to_vec());
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn chain() -> Result<Attestation, DeviceError> {
        Ok(Attestation {
            certs: vec![vec![1; 700], vec![2; 500]],
        })
    }

    fn fake_with(result: AttestResult, sleep: Duration) -> Arc<Fake> {
        Arc::new(Fake {
            result,
            random: || Ok(()),
            sleep,
            seen: Mutex::new(Vec::new()),
        })
    }

    fn fake(result: AttestResult) -> Arc<dyn AppAttest> {
        fake_with(result, Duration::ZERO)
    }

    /// フレームを回して届いた完了を適用する。`Running` を抜けるか 5 秒で打ち切る。
    fn pump(
        state: &mut AppAttestState,
        sender: &CompletionSender<AppAttestState>,
        rx: &Rx,
        imp: &Arc<dyn AppAttest>,
        session: &Session,
        deadline: Duration,
    ) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            drive(state, sender, imp, session, deadline);
            while let Ok(c) = rx.try_recv() {
                c(state);
            }
            if !matches!(state.phase, AttestPhase::Requested | AttestPhase::Running)
                || Instant::now() > end
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 要求を立ててフレームを回し、完了まで適用した状態を返す。
    fn run(imp: Arc<dyn AppAttest>) -> AppAttestState {
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = AppAttestState::default();
        request_attest(&mut state);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &session,
            Duration::from_secs(30),
        );
        state
    }

    fn failed(s: &AppAttestState) -> &AttestFailure {
        match &s.phase {
            AttestPhase::Failed(f) => f,
            p => panic!("失敗のはず: {p:?}"),
        }
    }

    #[test]
    fn no_request_does_nothing() {
        let (sender, rx) = channel();
        let imp = fake(chain);
        drive(
            &AppAttestState::default(),
            &sender,
            &imp,
            &Session::default(),
            Duration::from_secs(1),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_shows_count_and_bytes() {
        let s = run(fake(chain));
        assert_eq!(
            s.phase,
            AttestPhase::Done {
                count: 2,
                total_bytes: 1200
            }
        );
    }

    #[test]
    fn challenge_comes_from_fill_random_with_expected_length() {
        let f = fake_with(chain, Duration::ZERO);
        let imp: Arc<dyn AppAttest> = f.clone();
        let (sender, rx) = channel();
        let mut state = AppAttestState::default();
        request_attest(&mut state);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &Session::default(),
            Duration::from_secs(30),
        );
        assert_eq!(*f.seen.lock().unwrap(), vec![vec![0x5A; CHALLENGE_LEN]]);
    }

    #[test]
    fn fill_random_failure_is_failure_not_success() {
        let imp: Arc<dyn AppAttest> = Arc::new(Fake {
            result: chain,
            random: || Err(DeviceError::Internal("app_attest")),
            sleep: Duration::ZERO,
            seen: Mutex::new(Vec::new()),
        });
        let s = run(imp);
        assert_eq!(failed(&s), &AttestFailure::Failed(FAILED_MESSAGE.into()));
    }

    #[test]
    fn errors_map_to_failures() {
        let cases: [(AttestResult, AttestFailure); 8] = [
            (
                || Err(DeviceError::Unsupported("app_attest")),
                AttestFailure::Unsupported,
            ),
            (
                || Err(DeviceError::InvalidInput("app_attest challenge")),
                AttestFailure::InvalidInput,
            ),
            (
                || Err(DeviceError::Internal(KEYSTORE)),
                AttestFailure::KeystoreFailed,
            ),
            (|| Err(DeviceError::Internal(EMPTY)), AttestFailure::Empty),
            (
                || Err(DeviceError::Internal(TOO_LARGE)),
                AttestFailure::TooLarge,
            ),
            (
                || Err(DeviceError::Internal(TIMEOUT)),
                AttestFailure::Timeout,
            ),
            (
                || Err(DeviceError::PermissionDenied("app_attest")),
                AttestFailure::Failed(FAILED_MESSAGE.into()),
            ),
            (
                || Err(DeviceError::Internal("app_attest")),
                AttestFailure::Failed(FAILED_MESSAGE.into()),
            ),
        ];
        for (f, expect) in cases {
            let s = run(fake(f));
            assert_eq!(failed(&s), &expect);
        }
    }

    #[test]
    fn empty_or_oversized_chain_is_not_success() {
        let cases: [(AttestResult, AttestFailure); 3] = [
            (|| Ok(Attestation { certs: vec![] }), AttestFailure::Empty),
            (
                || {
                    Ok(Attestation {
                        certs: vec![vec![1], vec![]],
                    })
                },
                AttestFailure::Empty,
            ),
            (
                || {
                    Ok(Attestation {
                        certs: vec![vec![1; 70_000]],
                    })
                },
                AttestFailure::TooLarge,
            ),
        ];
        for (f, expect) in cases {
            let s = run(fake(f));
            assert_eq!(failed(&s), &expect);
        }
    }

    #[test]
    fn internal_ids_and_cert_bytes_never_reach_screen_text() {
        let s = run(fake(|| Err(DeviceError::Internal("secret os detail"))));
        let text = fandhe_core::screens::app_attest_show::error_text(&s);
        assert!(
            !text.contains("secret") && !text.contains("app_attest"),
            "{text}"
        );
        let s = run(fake(chain));
        let text = fandhe_core::screens::app_attest_show::result_text(&s);
        assert!(!text.contains("[1") && !text.contains("0x"), "{text}");
    }

    #[test]
    fn worker_past_deadline_is_timeout() {
        let imp: Arc<dyn AppAttest> = fake_with(chain, Duration::from_millis(60));
        let (sender, rx) = channel();
        let mut state = AppAttestState::default();
        request_attest(&mut state);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &Session::default(),
            Duration::from_millis(20),
        );
        assert_eq!(failed(&state), &AttestFailure::Timeout);
    }

    #[test]
    fn frame_deadline_times_out_and_late_worker_result_is_ignored() {
        let imp: Arc<dyn AppAttest> = fake_with(chain, Duration::from_millis(300));
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = AppAttestState::default();
        request_attest(&mut state);
        let deadline = Duration::from_millis(30);
        pump(&mut state, &sender, &rx, &imp, &session, deadline);
        assert_eq!(failed(&state), &AttestFailure::Timeout);
        // 遅れて届いたワーカーの結果は無視される（スロットは空・追加の完了なし）
        std::thread::sleep(Duration::from_millis(350));
        drive(&state, &sender, &imp, &session, deadline);
        assert!(rx.try_recv().is_err());
        assert_eq!(failed(&state), &AttestFailure::Timeout);
    }

    #[test]
    fn stale_generation_completion_is_ignored() {
        let mut state = AppAttestState::default();
        request_attest(&mut state);
        mark_running(&mut state);
        finish_attest(&mut state, 99, Ok((1, 1)));
        assert_eq!(state.phase, AttestPhase::Running);
    }

    #[test]
    fn double_drive_before_mark_applied_runs_once() {
        let (sender, rx) = channel();
        let imp = fake(chain);
        let session = Session::default();
        let mut state = AppAttestState::default();
        request_attest(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        // mark_running 1 件のみ（2 回目は発送されない）
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 1);
    }

    #[test]
    fn reset_lets_a_fresh_screen_dispatch_again() {
        let (sender, rx) = channel();
        let imp = fake(chain);
        let session = Session::default();
        session.slot.begin();
        let mut state = AppAttestState::default();
        request_attest(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.try_recv().is_err(), "記録が残ると飲み込まれる");
        session.slot.reset();
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn unsupported_impl_is_unsupported() {
        let s = run(Arc::new(UnsupportedAppAttest));
        assert_eq!(failed(&s), &AttestFailure::Unsupported);
    }

    #[test]
    fn fallback_platform_is_unsupported_off_android() {
        #[cfg(not(target_os = "android"))]
        {
            let imp = platform();
            let c = validate_challenge(&[1; CHALLENGE_LEN]).unwrap();
            assert!(matches!(
                imp.attest(&c, Duration::ZERO),
                Err(DeviceError::Unsupported(_))
            ));
        }
    }
}
