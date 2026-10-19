//! Phase 11（端末機能の拡充ループ、親 #107）型サンプル「PDF 表示」（Issue #469）の effect ドライバ。
//! `fandhe_core::screens::pdf_first_page`（端末依存ゼロ）の要求を観測し、ワーカースレッドで
//! `fandhe_device::pdf_viewer::PdfPageRenderer` を呼んで `CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・期限・守る範囲）の SSOT は
//! `fandhe_device::pdf_viewer`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web は対象外。
//!
//! # OS 別の分担（dispatch は #469 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: [`super::apple_pdf_viewer::ApplePdfPageRenderer`]（#469）
//! - iOS: `ios_pdf_viewer`（b-ios。macOS と共用の `ApplePdfPageRenderer` を返す）
//! - Windows: `windows_pdf_viewer`（windows の子。`Windows.Data.Pdf`）
//! - Android: `android_pdf_viewer`（b-android。`PdfRenderer`）
//! - それ以外（Linux）: `UnsupportedPdfPageRenderer` 固定（ダミーの成功実装へは倒さない）
//!
//! # 期限（契約表 3）
//!
//! 期限は本ドライバだけが持つ。起点は要求を観測した時の `Instant`（単調時計）。毎フレーム、
//! 処理中でセッションの世代が一致するとき `elapsed >= deadline` なら `Timeout` を確定して
//! セッションを空にし、まだなら `request_repaint_after` で次の判定を予約する。確定後に
//! ワーカーが返す結果は画面側の世代・状態照合（`finish`）で捨てられる。ワーカー自身も
//! 期限超過を `Timeout` にし、期限後に届いた成功を成功として扱わない。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::pdf_first_page::{
    finish, mark_loading, take_request, PdfFailure, PdfFirstPageState, PdfPhase, PdfShown,
};
use fandhe_core::RgbaPixels;
use fandhe_device::pdf_viewer::{
    check_page, sample_pdf, validate_input, PdfPageRenderer, BROKEN, EMPTY, ENCRYPTED, MAX_SIDE,
    PDF_DEADLINE, TIMEOUT,
};
use fandhe_device::DeviceError;

use super::request_slot::RequestSlot;
use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `PdfPageRenderer` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn PdfPageRenderer> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::apple_pdf_viewer::ApplePdfPageRenderer)
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_pdf_viewer::implementation()
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_pdf_viewer::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_pdf_viewer::implementation()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        Arc::new(fandhe_device::pdf_viewer::UnsupportedPdfPageRenderer)
    }
}

/// ドライバの可変状態。テストが並列で干渉しないよう static から切り出して引数で渡す。
///
/// 要求 1 件の追跡（採番・二重発送防止・破棄）は共通部品 [`RequestSlot`] に任せる。
/// `poll_and_finish` は通さない: 結果を置く側が OS コールバックではなく本ドライバ自身の
/// ワーカーで、完了は `CompletionSender` へ直接 push できるため。期限管理（受付時刻と
/// 画面の世代）だけを `timing` に残す。
struct Session {
    slot: RequestSlot<()>,
    /// 進行中の要求の `(request_id, 画面の世代, 受付時刻)`。
    timing: Mutex<Option<(u64, u64, Instant)>>,
}

impl Session {
    const fn new() -> Self {
        Self {
            slot: RequestSlot::new(),
            timing: Mutex::new(None),
        }
    }

    fn reset(&self) {
        self.slot.reset();
        *self.timing.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

static SESSION: std::sync::OnceLock<Arc<Session>> = std::sync::OnceLock::new();

/// Activity 再生成時（`android_main` の再実行）に、ドライバの処理済み記録を破棄する。
/// 画面状態は `default()`（世代 0）に戻るため、完了後も残る記録を残すと次の「表示」が
/// 発送されず飲み込まれる。古いワーカーの結果は古いアプリのチャネルへ届くので新しい画面には
/// 適用されない。走っている OS 呼び出しは止めず走り切らせる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        s.reset();
    }
}

/// `DeviceError` を画面の失敗種別へ写す。内部識別子・OS コードは画面にもログにも出さない
/// （security.md A09）。
pub(crate) fn to_failure(e: DeviceError) -> PdfFailure {
    match e {
        DeviceError::Unsupported(_) => PdfFailure::Unsupported,
        DeviceError::Internal(TIMEOUT) => PdfFailure::Timeout,
        DeviceError::Internal(BROKEN) => PdfFailure::Broken,
        DeviceError::Internal(ENCRYPTED) => PdfFailure::Encrypted,
        DeviceError::Internal(EMPTY) => PdfFailure::Empty,
        _ => PdfFailure::Failed,
    }
}

/// ワーカーの本体: 入力の生成と検査 → 描画 → 期限の確認 → 出力の再検査。
fn run_worker(
    imp: &dyn PdfPageRenderer,
    started: Instant,
    deadline: Duration,
) -> Result<PdfShown, PdfFailure> {
    let pdf = sample_pdf();
    validate_input(&pdf).map_err(to_failure)?;
    let result = imp.render_first_page(&pdf, MAX_SIDE);
    // 期限後に届いた結果は成功にしない（中身は見ない）。
    if started.elapsed() > deadline {
        return Err(PdfFailure::Timeout);
    }
    let p = result.map_err(to_failure)?;
    let p = check_page(p.page_count, p.image.width, p.image.height, p.image.rgba)
        .map_err(to_failure)?;
    let pixels =
        RgbaPixels::new(p.image.width, p.image.height, p.image.rgba).ok_or(PdfFailure::Failed)?;
    Ok(PdfShown {
        pixels,
        page_count: p.page_count,
    })
}

/// 1 フレームぶんの処理。要求の発送と期限判定を行う。
fn drive(
    state: &PdfFirstPageState,
    sender: &CompletionSender<PdfFirstPageState>,
    imp: &Arc<dyn PdfPageRenderer>,
    session: &Arc<Session>,
    deadline: Duration,
) {
    // 要求の観測と記録は同じロックの内側で行い、二重発送を防ぐ。
    let new_request = {
        let mut g = session.timing.lock().unwrap_or_else(|e| e.into_inner());
        match take_request(state) {
            Some(generation) if session.slot.snapshot().is_none() => {
                let started = Instant::now();
                let request_id = session.slot.begin();
                *g = Some((request_id, generation, started));
                Some((request_id, generation, started))
            }
            _ => {
                // 期限判定: 処理中でセッションの世代が一致するときだけ。
                if let (Some((request_id, generation, started)), PdfPhase::Loading) =
                    (*g, state.phase)
                {
                    if generation == state.generation {
                        let elapsed = started.elapsed();
                        if elapsed >= deadline {
                            *g = None;
                            session.slot.clear(request_id);
                            let _ = sender.send(Box::new(move |s: &mut PdfFirstPageState| {
                                finish(s, generation, Err(PdfFailure::Timeout));
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
    let Some((request_id, generation, started)) = new_request else {
        return;
    };
    let _ = sender.send(Box::new(mark_loading));
    sender.request_repaint_after(deadline);

    let worker_sender = sender.clone();
    let worker_imp = imp.clone();
    let worker_session = session.clone();
    let spawned = std::thread::Builder::new()
        .name("pdf-viewer".into())
        .spawn(move || {
            let result = run_worker(&*worker_imp, started, deadline);
            let _ = worker_sender.send(Box::new(move |s: &mut PdfFirstPageState| {
                // 完了キューの適用が遅れた場合も期限を守る（UI スレッド停止後の適用）。
                let result = match result {
                    Ok(_) if started.elapsed() > deadline => Err(PdfFailure::Timeout),
                    r => r,
                };
                finish(s, generation, result);
                worker_session.slot.clear(request_id);
            }));
        });
    if spawned.is_err() {
        let session = session.clone();
        let _ = sender.send(Box::new(move |s: &mut PdfFirstPageState| {
            // `mark_loading` の後に適用されるよう同じキューへ積む。
            finish(s, generation, Err(PdfFailure::Failed));
            session.slot.clear(request_id);
        }));
    }
}

/// `registry.rs` の `effect` 区画から `pdf_first_page` 画面用に配線される effect ドライバ。
pub fn pdf_first_page_effect(
    state: &PdfFirstPageState,
    sender: &CompletionSender<PdfFirstPageState>,
) {
    static IMPL: std::sync::OnceLock<Arc<dyn PdfPageRenderer>> = std::sync::OnceLock::new();
    let imp = IMPL.get_or_init(platform);
    drive(
        state,
        sender,
        imp,
        SESSION.get_or_init(|| Arc::new(Session::default())),
        PDF_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::pdf_first_page::{error_text, request};
    use fandhe_device::pdf_viewer::{PdfFirstPage, UnsupportedPdfPageRenderer};
    use fandhe_device::Image;
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<PdfFirstPageState>>;

    fn channel() -> (CompletionSender<PdfFirstPageState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn page(pages: u32, w: u32, h: u32, len: usize) -> Result<PdfFirstPage, DeviceError> {
        Ok(PdfFirstPage {
            page_count: pages,
            image: Image {
                width: w,
                height: h,
                rgba: vec![255; len],
            },
        })
    }

    fn good() -> Result<PdfFirstPage, DeviceError> {
        page(3, 2, 3, 24)
    }

    /// 指定の結果を返し、`sleep` だけ待つフェイク。
    struct Fake {
        result: fn() -> Result<PdfFirstPage, DeviceError>,
        sleep: Duration,
    }
    impl PdfPageRenderer for Fake {
        fn render_first_page(&self, _: &[u8], _: u32) -> Result<PdfFirstPage, DeviceError> {
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<PdfFirstPage, DeviceError>) -> Arc<dyn PdfPageRenderer> {
        Arc::new(Fake {
            result,
            sleep: Duration::ZERO,
        })
    }

    /// 要求を立てて 1 フレーム回し、届いた完了を `n` 件適用した状態を返す。
    fn run(imp: Arc<dyn PdfPageRenderer>, n: usize) -> PdfFirstPageState {
        let (sender, rx) = channel();
        let session = Arc::new(Session::default());
        let mut state = PdfFirstPageState::default();
        request(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        for _ in 0..n {
            let c = rx.recv_timeout(Duration::from_secs(5)).expect("完了が届く");
            c(&mut state);
        }
        state
    }

    fn failure(s: &PdfFirstPageState) -> PdfFailure {
        match &s.outcome {
            Some(Err(f)) => *f,
            o => panic!("失敗のはず: {o:?}"),
        }
    }

    #[test]
    fn no_request_does_nothing() {
        let (sender, rx) = channel();
        drive(
            &PdfFirstPageState::default(),
            &sender,
            &fake(good),
            &Arc::new(Session::default()),
            Duration::from_secs(1),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_fills_image_and_page_count() {
        let s = run(fake(good), 2);
        match s.outcome {
            Some(Ok(shown)) => assert_eq!(shown.page_count, 3),
            o => panic!("成功のはず: {o:?}"),
        }
    }

    #[test]
    fn invalid_pages_never_succeed() {
        // 寸法 0・長さ不一致・上限超え・0 ページはすべて失敗になる
        let s = run(fake(|| page(3, 0, 3, 0)), 2);
        assert_eq!(failure(&s), PdfFailure::Failed);
        let s = run(fake(|| page(3, 2, 3, 23)), 2);
        assert_eq!(failure(&s), PdfFailure::Failed);
        let s = run(
            fake(|| page(3, MAX_SIDE + 1, 1, (MAX_SIDE as usize + 1) * 4)),
            2,
        );
        assert_eq!(failure(&s), PdfFailure::Failed);
        let s = run(fake(|| page(0, 2, 3, 24)), 2);
        assert_eq!(failure(&s), PdfFailure::Empty);
    }

    #[test]
    fn os_errors_map_to_their_failures() {
        let s = run(fake(|| Err(DeviceError::Internal(ENCRYPTED))), 2);
        assert_eq!(failure(&s), PdfFailure::Encrypted);
        let s = run(fake(|| Err(DeviceError::Internal(BROKEN))), 2);
        assert_eq!(failure(&s), PdfFailure::Broken);
        let s = run(Arc::new(UnsupportedPdfPageRenderer), 2);
        assert_eq!(failure(&s), PdfFailure::Unsupported);
    }

    #[test]
    fn internal_ids_never_reach_screen_text() {
        let s = run(fake(|| Err(DeviceError::Internal("secret os detail"))), 2);
        assert!(!error_text(&s).contains("secret"));
        assert_eq!(failure(&s), PdfFailure::Failed);
    }

    #[test]
    fn worker_past_deadline_is_timeout_even_with_good_output() {
        let imp: Arc<dyn PdfPageRenderer> = Arc::new(Fake {
            result: good,
            sleep: Duration::from_millis(60),
        });
        let (sender, rx) = channel();
        let mut state = PdfFirstPageState::default();
        request(&mut state);
        drive(
            &state,
            &sender,
            &imp,
            &Arc::new(Session::default()),
            Duration::from_millis(20),
        );
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        }
        assert_eq!(failure(&state), PdfFailure::Timeout);
    }

    #[test]
    fn late_apply_past_deadline_is_timeout() {
        let (sender, rx) = channel();
        let mut state = PdfFirstPageState::default();
        request(&mut state);
        drive(
            &state,
            &sender,
            &fake(good),
            &Arc::new(Session::default()),
            Duration::from_millis(100),
        );
        let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        first(&mut state);
        second(&mut state);
        assert_eq!(failure(&state), PdfFailure::Timeout);
    }

    #[test]
    fn frame_deadline_times_out_and_late_worker_result_is_ignored() {
        let imp: Arc<dyn PdfPageRenderer> = Arc::new(Fake {
            result: good,
            sleep: Duration::from_millis(300),
        });
        let (sender, rx) = channel();
        let session = Arc::new(Session::default());
        let mut state = PdfFirstPageState::default();
        request(&mut state);
        let deadline = Duration::from_millis(30);
        drive(&state, &sender, &imp, &session, deadline);
        // mark_loading を適用
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        std::thread::sleep(Duration::from_millis(60));
        drive(&state, &sender, &imp, &session, deadline);
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failure(&state), PdfFailure::Timeout);
        // 遅れて届いたワーカーの結果は無視される
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failure(&state), PdfFailure::Timeout);
    }

    #[test]
    fn stale_generation_completion_is_ignored() {
        let mut state = PdfFirstPageState::default();
        request(&mut state);
        mark_loading(&mut state);
        finish(&mut state, 99, Err(PdfFailure::Failed));
        assert_eq!(state.phase, PdfPhase::Loading);
        assert_eq!(state.outcome, None);
    }

    #[test]
    fn double_drive_before_mark_applied_runs_once() {
        let (sender, rx) = channel();
        let imp = fake(good);
        let session = Arc::new(Session::default());
        let mut state = PdfFirstPageState::default();
        request(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(200));
        // mark_loading 1 + finish 1
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 2);
    }

    #[test]
    fn reset_lets_a_fresh_screen_dispatch_again() {
        let (sender, rx) = channel();
        let imp = fake(good);
        let session = Arc::new(Session::default());
        session.slot.begin();
        let mut state = PdfFirstPageState::default();
        request(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.try_recv().is_err(), "記録が残ると飲み込まれる");
        session.reset();
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn reset_then_stale_clear_does_not_consume_new_request() {
        let (sender, _rx) = channel();
        let imp = fake(good);
        let session = Arc::new(Session::default());
        let old_id = session.slot.begin();
        session.reset();
        let mut state = PdfFirstPageState::default();
        request(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        // 古い要求の後始末が、reset 後の新しい要求の記録を消さない。
        session.slot.clear(old_id);
        assert!(session.slot.snapshot().is_some());
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
            platform().render_first_page(&[1], MAX_SIDE),
            Err(DeviceError::Unsupported(_))
        ));
    }

    /// 出力の長辺の上限が `RgbaPixels` の上限に収まる（崩れたらコンパイルが通らない）。
    const _: () = assert!(MAX_SIDE <= fandhe_core::RGBA_MAX_SIDE);
}
