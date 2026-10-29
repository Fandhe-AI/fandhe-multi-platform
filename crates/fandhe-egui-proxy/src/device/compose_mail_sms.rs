//! Phase 11（親 #1142）型サンプル「メール・SMS の作成」（Issue #1384）の effect ドライバ。
//! `fandhe_core::screens::compose_mail_sms`（端末依存ゼロ）の要求を観測し、値を
//! `fandhe_device::compose_mail_sms::ComposeRequest` で検査してから OS へ渡し、結果を
//! `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、
//! `device/app_badge.rs` と同型）。契約と守る範囲は `fandhe_device::compose_mail_sms` の
//! モジュール doc を参照。
//!
//! - iOS / Android / Windows: コールバック型（Windows の添付は modal で戻らない作業スレッド）のため
//!   pull 型 [`bridge`]（`RequestSlot` の facade）で毎フレーム `drive_callback` が結果を反映する。
//!   Windows は添付なしの要求も `begin()` を通し、画面に入り直した後の古い完了を id 照合で捨てる。
//!   **dispatch はここで最終形にしてあり、後続 issue は OS 別モジュールだけを埋める**
//! - macOS / それ以外: [`ComposeMailSms`] を UI スレッドで同期に呼ぶ（macOS の添付付きも
//!   `NSSharingService` を同期に呼び、完了は待たない）。Linux は `UnsupportedComposeMailSms` で
//!   「非対応」を表示する

use fandhe_core::screens::compose_mail_sms::{
    take_request, ComposeFailure, ComposeMailSmsState, ComposeRequestView, ComposeTarget,
    CoreOutcome,
};
use fandhe_device::compose_mail_sms::{self, ComposeAttachment, ComposeOutcome, ComposeRequest};
use fandhe_device::DeviceError;

#[cfg(not(any(target_os = "ios", target_os = "android", target_os = "windows")))]
use fandhe_core::screens::compose_mail_sms::{finish_compose, mark_composing};
#[cfg(not(any(target_os = "ios", target_os = "android", target_os = "windows")))]
use fandhe_device::compose_mail_sms::ComposeMailSms;

use crate::app::CompletionSender;

/// `DeviceError` を画面用の結果へ写像する。内部構造・入力値は出さず固定文言だけにする
/// （security.md A09）。
fn to_outcome(result: Result<ComposeOutcome, DeviceError>) -> Result<CoreOutcome, ComposeFailure> {
    match result {
        Ok(ComposeOutcome::Queued) => Ok(CoreOutcome::Queued),
        Ok(ComposeOutcome::Saved) => Ok(CoreOutcome::Saved),
        Ok(ComposeOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Ok(ComposeOutcome::HandedOff) => Ok(CoreOutcome::HandedOff),
        Ok(ComposeOutcome::NoHandler) => Ok(CoreOutcome::NoHandler),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(DeviceError::InvalidInput(kind)) => Err(ComposeFailure::Invalid(
            match kind {
                "compose to" => "宛先",
                "compose cc" => "CC",
                "compose bcc" => "BCC",
                "compose subject" => "件名",
                "compose body" => "本文",
                "compose attachment" => "添付",
                _ => "入力",
            }
            .to_string(),
        )),
        Err(DeviceError::Internal(label)) => Err(ComposeFailure::Failed(
            match label {
                l if l == compose_mail_sms::ATTACHMENT_FILE => {
                    "添付の一時ファイルを用意できませんでした"
                }
                l if l == compose_mail_sms::CLEANUP => {
                    "前回の添付の一時ファイルを消せなかったため、作成画面を開きませんでした"
                }
                l if l == compose_mail_sms::BUSY => "前の作成画面がまだ開いています",
                l if l == compose_mail_sms::MAPI => {
                    "既定のメールアプリで添付付きの作成を開けませんでした（新しい Outlook は非対応）"
                }
                _ => "OS の作成画面を開けませんでした",
            }
            .to_string(),
        )),
        Err(_) => Err(ComposeFailure::Failed(
            "OS の作成画面を開けませんでした".to_string(),
        )),
    }
}

/// iOS の結果コード表（SSOT。`.m` の `RESULT_*` と一致させる）。未知のコードは失敗に倒す。
#[allow(dead_code)] // b-ios が結果受け口から使う
pub(crate) fn outcome_from_ios_code(code: i32) -> Result<ComposeOutcome, DeviceError> {
    match code {
        0 => Ok(ComposeOutcome::Queued),
        1 => Ok(ComposeOutcome::Saved),
        2 => Ok(ComposeOutcome::Cancelled),
        3 => Err(DeviceError::Internal("compose failed")),
        4 => Ok(ComposeOutcome::NoHandler),
        5 => Err(DeviceError::Internal("compose present")),
        _ => Err(DeviceError::Internal("compose outcome")),
    }
}

/// Android の結果コード表（SSOT。Java 側 `ComposeHelper.RESULT_*` と一致させる）。
#[allow(dead_code)] // b-android が結果受け口から使う
pub(crate) fn outcome_from_android_code(code: i32) -> Result<ComposeOutcome, DeviceError> {
    match code {
        0 => Ok(ComposeOutcome::HandedOff),
        1 => Ok(ComposeOutcome::NoHandler),
        2 => Err(DeviceError::Internal("compose background")),
        3 => Err(DeviceError::Internal("compose present")),
        _ => Err(DeviceError::Internal("compose outcome")),
    }
}

/// 画面の値から検査済みの要求を作る。OS へ渡す唯一の入口（検査の SSOT は `ComposeRequest`）。
fn build_request(view: &ComposeRequestView) -> Result<ComposeRequest, DeviceError> {
    match view.target {
        ComposeTarget::Mail => {
            let req =
                ComposeRequest::mail(&view.to, &view.cc, &view.bcc, &view.subject, &view.body)?;
            match &view.attachment {
                Some(a) => req.with_attachment(ComposeAttachment::new(
                    &a.file_name,
                    &a.mime_type,
                    &a.data,
                )?),
                None => Ok(req),
            }
        }
        ComposeTarget::Sms if view.to.len() > 1 => Err(DeviceError::InvalidInput("compose to")),
        ComposeTarget::Sms => {
            ComposeRequest::sms(view.to.first().map_or("", String::as_str), &view.body)
        }
    }
}

/// OS 側コールバックとの橋渡し（iOS / Android / Windows、pull 型、`RequestSlot` の facade）。
/// 他機能とは共有しない本機能専用の [`SLOT`](bridge::SLOT)。host テストでも使えるよう
/// `cfg(test)` でもコンパイルする。
#[cfg(any(target_os = "ios", target_os = "android", target_os = "windows", test))]
#[cfg_attr(
    not(any(target_os = "ios", target_os = "android", target_os = "windows")),
    allow(dead_code)
)]
pub(crate) mod bridge {
    use fandhe_core::screens::compose_mail_sms::{ComposeFailure, CoreOutcome};
    use fandhe_device::compose_mail_sms::ComposeOutcome;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<CoreOutcome, ComposeFailure>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    #[allow(dead_code)] // 後続 issue（b-ios / b-android）の受け口が使う
    pub(crate) fn deliver(request_id: u64, result: Result<ComposeOutcome, DeviceError>) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }

    /// Android: Activity 再生成直後に `android_main` から呼ぶ。保留中の要求は復元せず破棄し、
    /// 以後届く旧 `request_id` の [`deliver`] を無視させる（`app_badge::bridge::reset` と同じ）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    bridge::reset();
}

/// OS へ作成画面の表示を依頼する。`Ok` は受理済みで結果は後から [`bridge::deliver`] で届く。
/// `Err` は依頼自体の失敗（呼び出し元がその場で `deliver` する）。
#[cfg(target_os = "ios")]
fn begin_compose(request_id: u64, req: &ComposeRequest) -> Result<(), DeviceError> {
    super::ios_compose_mail_sms::begin_compose(request_id, req)
}

#[cfg(target_os = "android")]
fn begin_compose(request_id: u64, req: &ComposeRequest) -> Result<(), DeviceError> {
    super::android_compose_mail_sms::begin_compose(request_id, req)
}

/// Windows: 添付付きは作業スレッド（#1043）へ渡し、添付なしは `ShellExecuteW` を同期に呼んで結果を
/// すぐ `deliver` する（全要求で `begin()` を通すため、古い id の完了は `deliver` で捨てられる）。
#[cfg(target_os = "windows")]
fn begin_compose(request_id: u64, req: &ComposeRequest) -> Result<(), DeviceError> {
    if req.attachment().is_some() {
        return super::windows_compose_mail_sms::begin_compose_attachment(request_id, req);
    }
    let result = super::windows_compose_mail_sms::implementation().compose(req)?;
    bridge::deliver(request_id, Ok(result));
    Ok(())
}

/// iOS / Android / Windows 版の effect ドライバ本体。
#[cfg(any(target_os = "ios", target_os = "android", target_os = "windows"))]
fn drive_callback(state: &ComposeMailSmsState, sender: &CompletionSender<ComposeMailSmsState>) {
    use fandhe_core::screens::compose_mail_sms::{finish_compose, mark_composing, ComposePhase};

    if let Some(view) = take_request(state) {
        let _ = sender.send(Box::new(mark_composing));
        let request_id = bridge::begin();
        // 入力不正は OS へ渡さずその場で画面へ返す。
        let accepted = build_request(&view).and_then(|r| begin_compose(request_id, &r));
        if let Err(e) = accepted {
            bridge::deliver(request_id, Err(e));
        }
        return;
    }

    if state.phase != ComposePhase::Composing {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut ComposeMailSmsState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != ComposePhase::Idle {
                finish_compose(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// `registry.rs` から `compose_mail_sms` 画面用に配線される effect ドライバ（iOS / Android / Windows）。
#[cfg(any(target_os = "ios", target_os = "android", target_os = "windows"))]
pub fn compose_mail_sms_effect(
    state: &ComposeMailSmsState,
    sender: &CompletionSender<ComposeMailSmsState>,
) {
    drive_callback(state, sender);
}

/// 同期経路（macOS・Linux）: 検査してから [`ComposeMailSms`] を呼ぶ。
#[cfg(not(any(target_os = "ios", target_os = "android", target_os = "windows")))]
fn drive(
    state: &ComposeMailSmsState,
    sender: &CompletionSender<ComposeMailSmsState>,
    composer: &dyn ComposeMailSms,
) {
    let Some(view) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_composing));
    let result = build_request(&view).and_then(|r| composer.compose(&r));
    let outcome = to_outcome(result);
    let generation = view.generation;
    let _ = sender.send(Box::new(move |s: &mut ComposeMailSmsState| {
        finish_compose(s, generation, outcome);
    }));
}

/// OS 別の [`ComposeMailSms`] 実装を返す（macOS / それ以外）。
#[cfg(not(any(target_os = "ios", target_os = "android", target_os = "windows")))]
fn platform_compose() -> Box<dyn ComposeMailSms> {
    #[cfg(target_os = "macos")]
    return super::macos_compose_mail_sms::implementation();
    #[cfg(not(target_os = "macos"))]
    return Box::new(fandhe_device::compose_mail_sms::UnsupportedComposeMailSms);
}

/// `registry.rs` から配線される effect ドライバ（macOS・Linux）。
///
/// ponytail: 作成画面を開くのは OS への URL 1 回の軽い呼び出しなので UI スレッドで同期に呼ぶ。
/// 戻らない呼び出しが見つかったら、作業スレッド + bridge へ移す。
#[cfg(not(any(target_os = "ios", target_os = "android", target_os = "windows")))]
pub fn compose_mail_sms_effect(
    state: &ComposeMailSmsState,
    sender: &CompletionSender<ComposeMailSmsState>,
) {
    if take_request(state).is_none() {
        return;
    }
    drive(state, sender, platform_compose().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::compose_mail_sms as screen;

    #[test]
    fn to_outcome_covers_all_arms() {
        assert_eq!(
            to_outcome(Ok(ComposeOutcome::Queued)),
            Ok(CoreOutcome::Queued)
        );
        assert_eq!(
            to_outcome(Ok(ComposeOutcome::Saved)),
            Ok(CoreOutcome::Saved)
        );
        assert_eq!(
            to_outcome(Ok(ComposeOutcome::Cancelled)),
            Ok(CoreOutcome::Cancelled)
        );
        assert_eq!(
            to_outcome(Ok(ComposeOutcome::HandedOff)),
            Ok(CoreOutcome::HandedOff)
        );
        assert_eq!(
            to_outcome(Ok(ComposeOutcome::NoHandler)),
            Ok(CoreOutcome::NoHandler)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("compose"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert!(matches!(
            to_outcome(Err(DeviceError::InvalidInput("compose to"))),
            Err(ComposeFailure::Invalid(m)) if m == "宛先"
        ));
        assert!(matches!(
            to_outcome(Err(DeviceError::Internal("compose present"))),
            Err(ComposeFailure::Failed(_))
        ));
    }

    #[test]
    fn to_outcome_maps_attachment_labels_to_fixed_text() {
        for l in [
            compose_mail_sms::ATTACHMENT_FILE,
            compose_mail_sms::CLEANUP,
            compose_mail_sms::BUSY,
            compose_mail_sms::MAPI,
        ] {
            let Err(ComposeFailure::Failed(m)) = to_outcome(Err(DeviceError::Internal(l))) else {
                panic!("{l}");
            };
            assert!(!m.contains("compose") && !m.is_empty());
            assert_ne!(m, "OS の作成画面を開けませんでした");
        }
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("compose cc"))),
            Ok(CoreOutcome::Unsupported)
        );
    }

    #[test]
    fn code_tables_never_turn_unknown_codes_into_success() {
        for c in [-1, 6, 99, i32::MAX] {
            assert!(outcome_from_ios_code(c).is_err());
        }
        for c in [-1, 4, 99, i32::MAX] {
            assert!(outcome_from_android_code(c).is_err());
        }
        assert_eq!(outcome_from_ios_code(0).unwrap(), ComposeOutcome::Queued);
        assert_eq!(outcome_from_ios_code(1).unwrap(), ComposeOutcome::Saved);
        assert_eq!(outcome_from_ios_code(2).unwrap(), ComposeOutcome::Cancelled);
        assert_eq!(outcome_from_ios_code(4).unwrap(), ComposeOutcome::NoHandler);
        assert!(outcome_from_ios_code(3).is_err() && outcome_from_ios_code(5).is_err());
        assert_eq!(
            outcome_from_android_code(0).unwrap(),
            ComposeOutcome::HandedOff
        );
        assert_eq!(
            outcome_from_android_code(1).unwrap(),
            ComposeOutcome::NoHandler
        );
        assert!(outcome_from_android_code(2).is_err() && outcome_from_android_code(3).is_err());
    }

    #[test]
    fn build_request_validates_fixed_values_and_rejects_injection() {
        for target in [ComposeTarget::Mail, ComposeTarget::Sms] {
            let mut s = ComposeMailSmsState::default();
            match target {
                ComposeTarget::Mail => screen::compose_mail(&mut s),
                ComposeTarget::Sms => screen::compose_sms(&mut s),
            }
            assert!(build_request(&take_request(&s).unwrap()).is_ok());
        }
        let bad = ComposeRequestView {
            generation: 1,
            target: ComposeTarget::Mail,
            to: vec!["a@b.c&bcc=x@y.z".into()],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body: String::new(),
            attachment: None,
        };
        assert!(matches!(
            build_request(&bad),
            Err(DeviceError::InvalidInput("compose to"))
        ));
        let bad_cc = ComposeRequestView {
            to: vec!["a@b.c".into()],
            cc: vec!["x@y.z&bcc=q@r.s".into()],
            ..bad
        };
        let err = build_request(&bad_cc).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("compose cc")));
        assert_eq!(
            to_outcome(Err(err)),
            Err(ComposeFailure::Invalid("CC".into()))
        );
    }

    #[cfg(not(any(target_os = "ios", target_os = "android", target_os = "windows")))]
    mod sync_path {
        use super::*;
        use fandhe_device::compose_mail_sms::UnsupportedComposeMailSms;
        use std::sync::atomic::{AtomicU32, Ordering};

        /// 呼び出し回数を数え、決めた結果を返す偽実装。
        struct Fake {
            calls: AtomicU32,
            result: fn() -> Result<ComposeOutcome, DeviceError>,
        }

        impl ComposeMailSms for Fake {
            fn compose(&self, _r: &ComposeRequest) -> Result<ComposeOutcome, DeviceError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                (self.result)()
            }
        }

        fn fake(result: fn() -> Result<ComposeOutcome, DeviceError>) -> Fake {
            Fake {
                calls: AtomicU32::new(0),
                result,
            }
        }

        /// 押下して 1 回ドライバを回し、反映後の状態を返す。
        fn run(mail: bool, composer: &dyn ComposeMailSms) -> ComposeMailSmsState {
            let mut s = ComposeMailSmsState::default();
            if mail {
                screen::compose_mail(&mut s);
            } else {
                screen::compose_sms(&mut s);
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(&s, &sender, composer);
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            s
        }

        /// 実際の `to_url` を通す（URL 経路の OS 実装と同じ。添付は `Unsupported` になる）。
        struct UrlOnly;

        impl ComposeMailSms for UrlOnly {
            fn compose(&self, r: &ComposeRequest) -> Result<ComposeOutcome, DeviceError> {
                r.to_url().map(|_| ComposeOutcome::HandedOff)
            }
        }

        #[test]
        fn attachment_over_url_path_shows_unsupported() {
            let mut s = ComposeMailSmsState::default();
            screen::compose_mail_with_attachment(&mut s);
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(&s, &sender, &UrlOnly);
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
            assert!(s.error.is_empty());
        }

        #[test]
        fn no_request_never_calls_os() {
            let f = fake(|| Ok(ComposeOutcome::HandedOff));
            let s = ComposeMailSmsState::default();
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(&s, &sender, &f);
            assert!(rx.try_recv().is_err());
            assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn handed_off_and_no_handler_are_not_queued() {
            let f = fake(|| Ok(ComposeOutcome::HandedOff));
            let s = run(true, &f);
            assert_eq!(s.status, screen::HANDED_OFF_STATUS);
            assert_ne!(s.status, screen::QUEUED_STATUS);
            let f = fake(|| Ok(ComposeOutcome::NoHandler));
            let s = run(false, &f);
            assert_eq!(s.status, screen::NO_HANDLER_STATUS);
            assert!(s.error.is_empty());
        }

        #[test]
        fn unsupported_shows_unsupported_not_success() {
            let s = run(false, &UnsupportedComposeMailSms);
            assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
            assert!(s.error.is_empty());
            assert_ne!(s.status, screen::QUEUED_STATUS);
            assert_ne!(s.status, screen::HANDED_OFF_STATUS);
        }

        #[test]
        fn failure_goes_to_error_with_fixed_label() {
            let f = fake(|| Err(DeviceError::Internal("compose present")));
            let s = run(true, &f);
            assert!(s.status.is_empty());
            assert!(s.error.starts_with(screen::FAILED_PREFIX));
            assert!(!s.error.contains("compose"));
        }

        #[test]
        fn stale_generation_completion_is_dropped() {
            let mut s = ComposeMailSmsState::default();
            screen::compose_mail(&mut s);
            mark_composing(&mut s);
            let stale = s.generation.wrapping_sub(1);
            finish_compose(&mut s, stale, Ok(CoreOutcome::HandedOff));
            assert!(s.status.is_empty());
        }
    }
}

#[cfg(test)]
mod bridge_tests {
    use super::bridge;
    use super::*;

    #[test]
    fn stale_request_id_is_dropped_and_second_deliver_ignored() {
        let id = bridge::begin();
        assert!(!bridge::deliver(id + 1000, Ok(ComposeOutcome::Queued)));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(id, Ok(ComposeOutcome::Cancelled)));
        assert!(!bridge::deliver(id, Ok(ComposeOutcome::Queued)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(ready, Some(Ok(CoreOutcome::Cancelled)));
        bridge::SLOT.clear(id);
    }
}
