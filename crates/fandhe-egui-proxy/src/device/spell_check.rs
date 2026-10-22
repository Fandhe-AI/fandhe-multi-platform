//! Phase 11（親 #119）型サンプル「スペルチェック」（Issue #578）の effect ドライバ。
//! `fandhe_core::screens::spell_check`（端末依存ゼロ）の要求を観測し、ワーカースレッドで
//! `fandhe_device::spell_check::SpellChecker` を呼んで `CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・期限・守る範囲）の SSOT は
//! `fandhe_device::spell_check`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web は呼べる API が無く対象外。
//!
//! # OS 別の分担（dispatch は #578 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: [`super::macos_spell_check`]（#578。`NSSpellChecker`）
//! - iOS: [`super::ios_spell_check`]（#579。`UITextChecker`。バッファ解釈 [`parse_apple_issue_buffer`]・
//!   [`languages_from_nul_buffer`] は macOS と共用）
//! - Android: [`super::android_spell_check`]（#580。`TextServicesManager`）
//! - Windows: [`super::windows_spell_check`]（#581。`ISpellChecker`）
//! - Linux ほか: `UnsupportedSpellChecker` 固定（ダミーの成功実装へは倒さない）
//!
//! # 期限（契約表 3）
//!
//! 期限は本ドライバだけが持つ。起点は要求を観測した時の `Instant`（単調時計）。毎フレーム、処理中で
//! 結果が未着のとき `elapsed >= deadline` なら `Timeout` を確定してスロットを空にし、まだなら
//! `poll_and_finish` の再描画予約で次の判定に回る。要求の追跡は `request_slot::RequestSlot`
//! （pull 型）。確定後にワーカーが返す結果は画面側の世代・状態照合（`finish_check`）で捨てられる。
//! ワーカー自身も期限超過を `Timeout` にし、期限後に届いた成功を成功として扱わない。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::spell_check::{
    finish_check, mark_running, take_request, SpellCheckState, SpellFailure, SpellIssueView,
    SpellPhase, SpellResultView,
};
use fandhe_device::spell_check::{
    validate_request, SpellChecker, SpellReport, LANGUAGE_UNAVAILABLE, SPELL_CHECK_DEADLINE,
    TIMEOUT,
};
use fandhe_device::DeviceError;

use super::request_slot::{poll_and_finish, RequestSlot, POLL_INTERVAL};
use crate::app::CompletionSender;

const FAILED_MESSAGE: &str = "検査できませんでした";

/// 現在のプラットフォームに応じた `SpellChecker` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn SpellChecker> {
    #[cfg(target_os = "macos")]
    {
        super::macos_spell_check::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_spell_check::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_spell_check::implementation()
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_spell_check::implementation()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    {
        Arc::new(fandhe_device::spell_check::UnsupportedSpellChecker)
    }
}

/// Apple（macOS・iOS）の `.m` が返す誤りの記録を解釈する。形式は、誤り 1 件ごとに
/// `u32 utf16_start, u32 utf16_len, u32 n`（ネイティブエンディアン）に続けて、候補 `n` 個を
/// NUL 終端の UTF-8 で並べる。ちょうど `count` 件で過不足なく終わらない・途中で切れている・
/// 不正な UTF-8 を含む場合は `None`（嘘の成功にしない）。
#[cfg(any(target_os = "macos", target_os = "ios", test))]
// macOS（#578）と iOS（#579）が使う。ホストのテストでは呼び出し元が無く未使用になる。
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(super) fn parse_apple_issue_buffer(
    buf: &[u8],
    count: usize,
) -> Option<Vec<fandhe_device::spell_check::RawIssue>> {
    fn u32_at(buf: &[u8], pos: &mut usize) -> Option<usize> {
        let b = buf.get(*pos..pos.checked_add(4)?)?;
        *pos += 4;
        Some(u32::from_ne_bytes(b.try_into().ok()?) as usize)
    }
    let mut pos = 0usize;
    let mut out = Vec::new();
    for _ in 0..count {
        let utf16_start = u32_at(buf, &mut pos)?;
        let utf16_len = u32_at(buf, &mut pos)?;
        let n = u32_at(buf, &mut pos)?;
        let mut suggestions = Vec::new();
        for _ in 0..n {
            let rest = buf.get(pos..)?;
            let end = rest.iter().position(|&b| b == 0)?;
            suggestions.push(std::str::from_utf8(&rest[..end]).ok()?.to_owned());
            pos += end + 1;
        }
        out.push(fandhe_device::spell_check::RawIssue {
            utf16_start,
            utf16_len,
            suggestions,
        });
    }
    (pos == buf.len()).then_some(out)
}

/// Apple（macOS・iOS）の `.m` が返す利用可能な言語の一覧（NUL 区切りの UTF-8）を解釈する。
/// 件数が `count` と合わない・不正な UTF-8 を含む場合は `None`。
#[cfg(any(target_os = "macos", target_os = "ios", test))]
// macOS（#578）と iOS（#579）が使う。ホストのテストでは呼び出し元が無く未使用になる。
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(super) fn languages_from_nul_buffer(buf: &[u8], count: usize) -> Option<Vec<String>> {
    let names: Vec<String> = buf
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| std::str::from_utf8(s).ok().map(str::to_owned))
        .collect::<Option<_>>()?;
    (names.len() == count).then_some(names)
}

/// ドライバの可変状態。要求の追跡（二重発送防止・完了照合・再生成時の破棄）は
/// [`RequestSlot`] に任せ、ここは期限の起点だけを持つ。テストが並列で干渉しないよう static から
/// 切り出して引数で渡す（`poll_and_finish` が `'static` を要るためスロットは leak）。
struct Session {
    slot: &'static RequestSlot<Result<SpellResultView, SpellFailure>>,
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
/// 画面状態は `default()`（世代 0）に戻るため、保留が残ると次の検査が発送されず飲み込まれる。
/// 古いワーカーの結果は照合で捨てられる。走っている OS 呼び出しは止めず走り切らせる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        s.slot.reset();
        *s.started.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// `DeviceError` を画面の失敗種別へ写す。入力・OS の文言・内部識別子は画面にもログにも出さず、
/// 上記以外（`PermissionDenied`・`Cancelled` を含む）は固定文言にする（security.md A09）。
pub(crate) fn to_failure(e: DeviceError) -> SpellFailure {
    match e {
        DeviceError::Unsupported(_) | DeviceError::Disabled(_) => SpellFailure::Unsupported,
        DeviceError::InvalidInput(_) => SpellFailure::InvalidInput,
        DeviceError::Internal(LANGUAGE_UNAVAILABLE) => SpellFailure::LanguageUnavailable,
        DeviceError::Internal(TIMEOUT) => SpellFailure::Timeout,
        _ => SpellFailure::Failed(FAILED_MESSAGE.to_string()),
    }
}

/// 検査結果を画面の表示用データへ写す。
fn to_view(r: SpellReport) -> SpellResultView {
    SpellResultView {
        language: r.language,
        issues: r
            .issues
            .into_iter()
            .map(|i| SpellIssueView {
                start: i.start,
                word: i.word,
                suggestions: i.suggestions,
            })
            .collect(),
        truncated: r.truncated,
    }
}

/// ワーカーの本体: 入力検査 → 検査 → 期限の照合。入力不正は実装に届かない。
fn run_worker(
    imp: &dyn SpellChecker,
    text: &str,
    language: &str,
    started: Instant,
    deadline: Duration,
) -> Result<SpellResultView, SpellFailure> {
    validate_request(text, language).map_err(to_failure)?;
    let result = imp.check(text, language, deadline.saturating_sub(started.elapsed()));
    // 期限後に届いた結果は成功にしない。
    if started.elapsed() > deadline {
        return Err(SpellFailure::Timeout);
    }
    result.map(to_view).map_err(to_failure)
}

/// 1 フレームぶんの処理。要求の発送・期限判定・完了の画面反映を行う。
fn drive(
    state: &SpellCheckState,
    sender: &CompletionSender<SpellCheckState>,
    imp: &Arc<dyn SpellChecker>,
    session: &Session,
    deadline: Duration,
) {
    let slot = session.slot;
    if let Some((_, text, language)) = take_request(state) {
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
            .name("spell_check".into())
            .spawn(move || {
                let result = run_worker(&*worker_imp, &text, language, started, deadline);
                slot.deliver(request_id, result);
            });
        if spawned.is_err() {
            slot.deliver(
                request_id,
                Err(SpellFailure::Failed(FAILED_MESSAGE.to_string())),
            );
        }
        return;
    }
    if state.phase != SpellPhase::Running {
        return;
    }
    let generation = state.generation;
    // 期限判定: 単調時計。確定後に届く結果は照合で捨てられる。
    let started = *session.started.lock().unwrap_or_else(|e| e.into_inner());
    if let (Some(started), Some((request_id, None))) = (started, slot.snapshot()) {
        // 判定と除去を原子的に行う。false は deliver が先に着地済み（成功を消さず後続に任せる）。
        if started.elapsed() >= deadline && slot.clear_if_pending(request_id) {
            let _ = sender.send(Box::new(move |s: &mut SpellCheckState| {
                finish_check(s, generation, Err(SpellFailure::Timeout));
            }));
            return;
        }
    }
    poll_and_finish(
        slot,
        sender,
        |_| {},
        move |s: &mut SpellCheckState, result| {
            finish_check(s, generation, result);
            true
        },
    );
}

/// `registry.rs` の `effect` 区画から `spell_check` 画面用に配線される effect ドライバ。
pub fn spell_check_effect(state: &SpellCheckState, sender: &CompletionSender<SpellCheckState>) {
    static IMPL: std::sync::OnceLock<Arc<dyn SpellChecker>> = std::sync::OnceLock::new();
    let imp = IMPL.get_or_init(platform);
    drive(
        state,
        sender,
        imp,
        SESSION.get_or_init(Session::default),
        SPELL_CHECK_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::spell_check::{request_check, set_input, LanguageChoice};
    use fandhe_device::spell_check::UnsupportedSpellChecker;
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<SpellCheckState>>;
    type Outcome = Result<SpellReport, DeviceError>;

    fn channel() -> (CompletionSender<SpellCheckState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 指定の結果を返し、`sleep` だけ待つフェイク。受け取った入力を記録する。
    struct Fake {
        result: fn() -> Outcome,
        sleep: Duration,
        seen: Mutex<Vec<(String, String)>>,
    }
    impl SpellChecker for Fake {
        fn check(&self, text: &str, language: &str, _: Duration) -> Outcome {
            self.seen
                .lock()
                .unwrap()
                .push((text.to_string(), language.to_string()));
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn report() -> Outcome {
        Ok(SpellReport {
            language: "en_US".into(),
            issues: vec![fandhe_device::spell_check::SpellIssue {
                start: 5,
                len: 8,
                word: "sentense".into(),
                suggestions: vec!["sentence".into()],
            }],
            truncated: false,
        })
    }

    fn fake_with(result: fn() -> Outcome, sleep: Duration) -> Arc<Fake> {
        Arc::new(Fake {
            result,
            sleep,
            seen: Mutex::new(Vec::new()),
        })
    }

    fn fake(result: fn() -> Outcome) -> Arc<dyn SpellChecker> {
        fake_with(result, Duration::ZERO)
    }

    /// フレームを回して届いた完了を適用する。`Running` を抜けるか 5 秒で打ち切る。
    fn pump(
        state: &mut SpellCheckState,
        sender: &CompletionSender<SpellCheckState>,
        rx: &Rx,
        imp: &Arc<dyn SpellChecker>,
        session: &Session,
        deadline: Duration,
    ) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            drive(state, sender, imp, session, deadline);
            while let Ok(c) = rx.try_recv() {
                c(state);
            }
            if !matches!(state.phase, SpellPhase::Requested | SpellPhase::Running)
                || Instant::now() > end
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn run_with(imp: Arc<dyn SpellChecker>, state: SpellCheckState) -> SpellCheckState {
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = state;
        request_check(&mut state, LanguageChoice::English);
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

    fn run(imp: Arc<dyn SpellChecker>) -> SpellCheckState {
        run_with(imp, SpellCheckState::default())
    }

    fn failed(s: &SpellCheckState) -> &SpellFailure {
        match &s.phase {
            SpellPhase::Failed(f) => f,
            p => panic!("失敗のはず: {p:?}"),
        }
    }

    #[test]
    fn no_request_does_nothing() {
        let (sender, rx) = channel();
        let imp = fake(report);
        drive(
            &SpellCheckState::default(),
            &sender,
            &imp,
            &Session::default(),
            Duration::from_secs(1),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_shows_issues() {
        let s = run(fake(report));
        let SpellPhase::Done(v) = &s.phase else {
            panic!("成功のはず: {:?}", s.phase);
        };
        assert_eq!(v.issues.len(), 1);
        assert_eq!(v.issues[0].word, "sentense");
        assert_eq!(v.issues[0].suggestions, ["sentence"]);
    }

    #[test]
    fn zero_issues_is_a_success() {
        let s = run(fake(|| {
            Ok(SpellReport {
                language: "en".into(),
                issues: vec![],
                truncated: false,
            })
        }));
        assert!(matches!(&s.phase, SpellPhase::Done(v) if v.issues.is_empty()));
    }

    #[test]
    fn input_and_language_reach_implementation() {
        let f = fake_with(report, Duration::ZERO);
        let imp: Arc<dyn SpellChecker> = f.clone();
        let (sender, rx) = channel();
        let mut state = SpellCheckState::default();
        set_input(&mut state, "helo wrold".into());
        request_check(&mut state, LanguageChoice::Unavailable);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &Session::default(),
            Duration::from_secs(30),
        );
        assert_eq!(
            *f.seen.lock().unwrap(),
            vec![("helo wrold".to_string(), "zz".to_string())]
        );
    }

    #[test]
    fn errors_map_to_failures() {
        type R = fn() -> Outcome;
        let fixed = SpellFailure::Failed(FAILED_MESSAGE.into());
        let cases: [(R, SpellFailure); 8] = [
            (
                || Err(DeviceError::Unsupported("spell_check")),
                SpellFailure::Unsupported,
            ),
            (
                || Err(DeviceError::Disabled("spell_check")),
                SpellFailure::Unsupported,
            ),
            (
                || Err(DeviceError::Internal(LANGUAGE_UNAVAILABLE)),
                SpellFailure::LanguageUnavailable,
            ),
            (
                || Err(DeviceError::Internal(TIMEOUT)),
                SpellFailure::Timeout,
            ),
            (
                || Err(DeviceError::InvalidInput("spell_check text")),
                SpellFailure::InvalidInput,
            ),
            (
                || Err(DeviceError::PermissionDenied("spell_check")),
                fixed.clone(),
            ),
            (|| Err(DeviceError::Cancelled("spell_check")), fixed.clone()),
            (|| Err(DeviceError::Internal("spell_check range")), fixed),
        ];
        for (f, expect) in cases {
            let s = run(fake(f));
            assert_eq!(failed(&s), &expect);
        }
    }

    #[test]
    fn invalid_input_never_reaches_implementation() {
        for bad in ["", "  \n", "a\0b"] {
            let f = fake_with(report, Duration::ZERO);
            let imp: Arc<dyn SpellChecker> = f.clone();
            let mut st = SpellCheckState::default();
            set_input(&mut st, bad.to_string());
            let s = run_with(imp, st);
            assert_eq!(failed(&s), &SpellFailure::InvalidInput, "{bad:?}");
            assert!(f.seen.lock().unwrap().is_empty());
        }
        let f = fake_with(report, Duration::ZERO);
        let imp: Arc<dyn SpellChecker> = f.clone();
        let mut st = SpellCheckState::default();
        set_input(&mut st, "a".repeat(1001));
        let s = run_with(imp, st);
        assert_eq!(failed(&s), &SpellFailure::InvalidInput);
        assert!(f.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn internal_ids_never_reach_screen_text() {
        let s = run(fake(|| Err(DeviceError::Internal("secret os detail"))));
        let text = fandhe_core::screens::spell_check::error_text(&s);
        assert!(
            !text.contains("secret") && !text.contains("spell_check "),
            "{text}"
        );
    }

    #[test]
    fn worker_past_deadline_is_timeout() {
        let imp: Arc<dyn SpellChecker> = fake_with(report, Duration::from_millis(60));
        let (sender, rx) = channel();
        let mut state = SpellCheckState::default();
        request_check(&mut state, LanguageChoice::English);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &Session::default(),
            Duration::from_millis(20),
        );
        assert_eq!(failed(&state), &SpellFailure::Timeout);
    }

    #[test]
    fn frame_deadline_times_out_and_late_worker_result_is_ignored() {
        let imp: Arc<dyn SpellChecker> = fake_with(report, Duration::from_millis(300));
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = SpellCheckState::default();
        request_check(&mut state, LanguageChoice::English);
        let deadline = Duration::from_millis(30);
        pump(&mut state, &sender, &rx, &imp, &session, deadline);
        assert_eq!(failed(&state), &SpellFailure::Timeout);
        // 遅れて届いたワーカーの結果は無視される（スロットは空・追加の完了なし）
        std::thread::sleep(Duration::from_millis(350));
        drive(&state, &sender, &imp, &session, deadline);
        assert!(rx.try_recv().is_err());
        assert_eq!(failed(&state), &SpellFailure::Timeout);
    }

    #[test]
    fn stale_generation_completion_is_ignored() {
        let mut state = SpellCheckState::default();
        request_check(&mut state, LanguageChoice::English);
        mark_running(&mut state);
        finish_check(
            &mut state,
            99,
            Ok(SpellResultView {
                language: "en".into(),
                issues: vec![],
                truncated: false,
            }),
        );
        assert_eq!(state.phase, SpellPhase::Running);
    }

    #[test]
    fn double_drive_before_mark_applied_runs_once() {
        let (sender, rx) = channel();
        let imp = fake(report);
        let session = Session::default();
        let mut state = SpellCheckState::default();
        request_check(&mut state, LanguageChoice::English);
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
        let imp = fake(report);
        let session = Session::default();
        session.slot.begin();
        let mut state = SpellCheckState::default();
        request_check(&mut state, LanguageChoice::English);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.try_recv().is_err(), "記録が残ると飲み込まれる");
        session.slot.reset();
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn unsupported_impl_is_unsupported() {
        let s = run(Arc::new(UnsupportedSpellChecker));
        assert_eq!(failed(&s), &SpellFailure::Unsupported);
    }

    #[test]
    fn fallback_platform_is_unsupported_on_linux() {
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "android",
            target_os = "windows"
        )))]
        assert!(matches!(
            platform().check("x", "en", Duration::ZERO),
            Err(DeviceError::Unsupported(_))
        ));
    }

    fn record(start: u32, len: u32, sugg: &[&str]) -> Vec<u8> {
        let mut b = Vec::new();
        for v in [start, len, sugg.len() as u32] {
            b.extend_from_slice(&v.to_ne_bytes());
        }
        for s in sugg {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        b
    }

    #[test]
    fn apple_buffer_parses_records() {
        let mut buf = record(5, 8, &["sentence", "sentences"]);
        buf.extend(record(20, 7, &[]));
        let v = parse_apple_issue_buffer(&buf, 2).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].utf16_start, v[0].utf16_len), (5, 8));
        assert_eq!(v[0].suggestions, ["sentence", "sentences"]);
        assert!(v[1].suggestions.is_empty());
        assert_eq!(parse_apple_issue_buffer(b"", 0).unwrap().len(), 0);
    }

    #[test]
    fn apple_buffer_rejects_malformed() {
        let ok = record(1, 2, &["ab"]);
        // 途中で切れている
        assert!(parse_apple_issue_buffer(&ok[..ok.len() - 1], 1).is_none());
        assert!(parse_apple_issue_buffer(&ok[..6], 1).is_none());
        // 件数の不一致（不足・余り）
        assert!(parse_apple_issue_buffer(&ok, 2).is_none());
        assert!(parse_apple_issue_buffer(&ok, 0).is_none());
        // 不正な UTF-8
        let mut bad = record(1, 2, &[]);
        bad[8..12].copy_from_slice(&1u32.to_ne_bytes());
        bad.extend_from_slice(&[0xff, 0xfe, 0]);
        assert!(parse_apple_issue_buffer(&bad, 1).is_none());
        // 巨大な候補数でも落ちない
        let mut huge = record(1, 2, &[]);
        huge[8..12].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(parse_apple_issue_buffer(&huge, 1).is_none());
    }

    #[test]
    fn languages_buffer_parses_and_checks_count() {
        assert_eq!(
            languages_from_nul_buffer(b"en_US\0fr\0", 2).unwrap(),
            ["en_US", "fr"]
        );
        assert!(languages_from_nul_buffer(b"en_US\0", 2).is_none());
        assert!(languages_from_nul_buffer(b"\xff\0", 1).is_none());
        assert_eq!(languages_from_nul_buffer(b"", 0).unwrap().len(), 0);
    }
}
