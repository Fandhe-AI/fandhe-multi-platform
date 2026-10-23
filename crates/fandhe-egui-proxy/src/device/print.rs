//! Phase 11（親 #94）型サンプル「印刷」（Issue #325）の effect ドライバ。
//! `fandhe_core::screens::print_page`（端末依存ゼロ）の要求を観測し、値を
//! `fandhe_device::print::PrintRequest` で検査してから OS へ渡し、結果を `CompletionSender` 経由で
//! 返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、`device/compose_mail_sms.rs` と同型）。
//! 契約と守る範囲は `fandhe_device::print` のモジュール doc を参照。
//!
//! - iOS / Android: コールバック型のため pull 型 [`bridge`]（`RequestSlot` の facade）で毎フレーム
//!   `drive_callback` が結果を反映する。Android（#328）・iOS（#327）とも実装済み。
//!   **dispatch はここで最終形にしてあり、後続 issue は OS 別
//!   モジュールだけを埋める**
//! - macOS / Windows / それ以外: [`Print`] を UI スレッドで同期に呼ぶ（同期モーダル）。Linux は
//!   `UnsupportedPrint` で「非対応」を表示する

use fandhe_core::screens::print_page::{
    take_request, CoreOutcome, PrintFailure, PrintPageState, PrintRequestView,
};
use fandhe_device::print::{PrintOutcome, PrintRequest};
use fandhe_device::DeviceError;

#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_core::screens::print_page::{finish_print, mark_printing};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_device::print::Print;

use crate::app::CompletionSender;

/// `DeviceError` を画面用の結果へ写像する。内部構造・入力値は出さず固定文言だけにする
/// （security.md A09）。
fn to_outcome(result: Result<PrintOutcome, DeviceError>) -> Result<CoreOutcome, PrintFailure> {
    match result {
        Ok(PrintOutcome::Submitted) => Ok(CoreOutcome::Submitted),
        Ok(PrintOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Ok(PrintOutcome::Unconfirmed) => Ok(CoreOutcome::Unconfirmed),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(DeviceError::InvalidInput(kind)) => Err(PrintFailure::Invalid(
            match kind {
                "print title" => "見出し",
                "print body" => "本文",
                _ => "入力",
            }
            .to_string(),
        )),
        Err(_) => Err(PrintFailure::Failed(
            "OS の印刷を開始できませんでした".to_string(),
        )),
    }
}

/// iOS の結果コード表（SSOT。`ios/fandhe_ios_print.m` の結果コードと一致させる）。未知のコードは失敗に倒す。
/// 0 = completed かつ error なし / 1 = completed NO かつ error なし（取り消し）/ 2 = error あり /
/// 3 = 提示不可（main 外）/ 4 = `isPrintingAvailable == NO`。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))] // iOS の結果受け口（ios_print.rs）が使う
pub(crate) fn outcome_from_ios_code(code: i32) -> Result<PrintOutcome, DeviceError> {
    match code {
        0 => Ok(PrintOutcome::Submitted),
        1 => Ok(PrintOutcome::Cancelled),
        2 => Err(DeviceError::Internal("print failed")),
        3 => Err(DeviceError::Internal("print present")),
        4 => Err(DeviceError::Unsupported("print")),
        _ => Err(DeviceError::Internal("print outcome")),
    }
}

/// Android の結果コード表（SSOT。Java `PrintHelper.RESULT_*` と一致させる）。
/// 0 = QUEUED / STARTED / BLOCKED / COMPLETED / 1 = CANCELED / 2 = 2 秒後も CREATED /
/// 3 = FAILED・onWrite 失敗 / 4 = `print()` 例外・Activity 無し。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn outcome_from_android_code(code: i32) -> Result<PrintOutcome, DeviceError> {
    match code {
        0 => Ok(PrintOutcome::Submitted),
        1 => Ok(PrintOutcome::Cancelled),
        2 => Ok(PrintOutcome::Unconfirmed),
        3 => Err(DeviceError::Internal("print failed")),
        4 => Err(DeviceError::Internal("print present")),
        _ => Err(DeviceError::Internal("print outcome")),
    }
}

/// 画面の値から検査済みの要求を作る。OS へ渡す唯一の入口（検査の SSOT は `PrintRequest`）。
fn build_request(view: &PrintRequestView) -> Result<PrintRequest, DeviceError> {
    PrintRequest::new(&view.title, &view.body)
}

/// OS 側コールバックとの橋渡し（iOS / Android、pull 型、`RequestSlot` の facade）。
/// 他機能とは共有しない本機能専用の [`SLOT`](bridge::SLOT)。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use fandhe_core::screens::print_page::{CoreOutcome, PrintFailure};
    use fandhe_device::print::PrintOutcome;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<CoreOutcome, PrintFailure>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    #[allow(dead_code)] // 後続 issue（b-ios / b-android）の受け口が使う
    pub(crate) fn deliver(request_id: u64, result: Result<PrintOutcome, DeviceError>) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }

    /// Android: Activity 再生成直後に `android_main` から呼ぶ。保留中の要求は復元せず破棄し、
    /// 以後届く旧 `request_id` の [`deliver`] を無視させる。
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

/// OS へ印刷ダイアログの表示を依頼する。`Ok` は受理済みで結果は後から [`bridge::deliver`] で届く。
/// `Err` は依頼自体の失敗（呼び出し元がその場で `deliver` する）。
#[cfg(target_os = "ios")]
fn begin_print(request_id: u64, req: &PrintRequest) -> Result<(), DeviceError> {
    super::ios_print::begin_print(request_id, req)
}

#[cfg(target_os = "android")]
fn begin_print(request_id: u64, req: &PrintRequest) -> Result<(), DeviceError> {
    super::android_print::begin_print(request_id, req)
}

/// iOS / Android 版の effect ドライバ本体。保留中（`Printing`）は新規要求を受けない
/// （画面の phase が二重押しを止める。iOS の共有印刷コントローラの再 present 対策）。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn drive_callback(state: &PrintPageState, sender: &CompletionSender<PrintPageState>) {
    use fandhe_core::screens::print_page::{finish_print, mark_printing, PrintPhase};

    if let Some(view) = take_request(state) {
        let _ = sender.send(Box::new(mark_printing));
        let request_id = bridge::begin();
        // 入力不正は OS へ渡さずその場で画面へ返す。
        let accepted = build_request(&view).and_then(|r| begin_print(request_id, &r));
        if let Err(e) = accepted {
            bridge::deliver(request_id, Err(e));
        }
        return;
    }

    if state.phase != PrintPhase::Printing {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut PrintPageState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != PrintPhase::Idle {
                finish_print(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// `registry.rs` から `print_page` 画面用に配線される effect ドライバ（iOS / Android）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn print_page_effect(state: &PrintPageState, sender: &CompletionSender<PrintPageState>) {
    drive_callback(state, sender);
}

/// 同期経路（macOS・Windows・Linux）: 検査してから [`Print`] を呼ぶ。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn drive(state: &PrintPageState, sender: &CompletionSender<PrintPageState>, printer: &dyn Print) {
    let Some(view) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_printing));
    let result = build_request(&view).and_then(|r| printer.print(&r));
    let outcome = to_outcome(result);
    let generation = view.generation;
    let _ = sender.send(Box::new(move |s: &mut PrintPageState| {
        finish_print(s, generation, outcome);
    }));
}

/// OS 別の [`Print`] 実装を返す（macOS / Windows / それ以外）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn platform_print() -> Box<dyn Print> {
    #[cfg(target_os = "macos")]
    return super::macos_print::implementation();
    #[cfg(target_os = "windows")]
    return super::windows_print::implementation();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(fandhe_device::print::UnsupportedPrint);
}

/// `registry.rs` から配線される effect ドライバ（macOS・Windows・Linux）。
///
/// ponytail: 印刷ダイアログは同期モーダルなので UI スレッドで直接呼ぶ（`file_picker` が macOS で
/// rfd の同期ダイアログを呼ぶのと同じ判断）。戻らない呼び出しが見つかったら、作業スレッド +
/// bridge へ移す。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn print_page_effect(state: &PrintPageState, sender: &CompletionSender<PrintPageState>) {
    if take_request(state).is_none() {
        return;
    }
    drive(state, sender, platform_print().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::print_page as screen;

    // 印刷に権限は要らないため、権限拒否の失敗系は該当なし。

    #[test]
    fn to_outcome_covers_all_arms() {
        assert_eq!(
            to_outcome(Ok(PrintOutcome::Submitted)),
            Ok(CoreOutcome::Submitted)
        );
        assert_eq!(
            to_outcome(Ok(PrintOutcome::Cancelled)),
            Ok(CoreOutcome::Cancelled)
        );
        assert_eq!(
            to_outcome(Ok(PrintOutcome::Unconfirmed)),
            Ok(CoreOutcome::Unconfirmed)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("print"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::InvalidInput("print title"))),
            Err(PrintFailure::Invalid("見出し".into()))
        );
        assert_eq!(
            to_outcome(Err(DeviceError::InvalidInput("print body"))),
            Err(PrintFailure::Invalid("本文".into()))
        );
        assert_eq!(
            to_outcome(Err(DeviceError::InvalidInput("other"))),
            Err(PrintFailure::Invalid("入力".into()))
        );
        assert!(matches!(
            to_outcome(Err(DeviceError::Internal("print failed"))),
            Err(PrintFailure::Failed(_))
        ));
    }

    #[test]
    fn code_tables_never_turn_unknown_codes_into_success() {
        for c in [-1, 5, 99, i32::MAX] {
            assert!(outcome_from_ios_code(c).is_err());
        }
        for c in [-1, 5, 99, i32::MAX] {
            assert!(outcome_from_android_code(c).is_err());
        }
        assert_eq!(outcome_from_ios_code(0).unwrap(), PrintOutcome::Submitted);
        assert_eq!(outcome_from_ios_code(1).unwrap(), PrintOutcome::Cancelled);
        assert!(matches!(
            outcome_from_ios_code(4),
            Err(DeviceError::Unsupported("print"))
        ));
        assert!(outcome_from_ios_code(2).is_err() && outcome_from_ios_code(3).is_err());
        assert_eq!(
            outcome_from_android_code(0).unwrap(),
            PrintOutcome::Submitted
        );
        assert_eq!(
            outcome_from_android_code(1).unwrap(),
            PrintOutcome::Cancelled
        );
        assert_eq!(
            outcome_from_android_code(2).unwrap(),
            PrintOutcome::Unconfirmed
        );
        assert!(outcome_from_android_code(3).is_err() && outcome_from_android_code(4).is_err());
    }

    #[test]
    fn build_request_accepts_fixed_values_and_rejects_control_chars() {
        let mut s = PrintPageState::default();
        screen::print(&mut s);
        assert!(build_request(&take_request(&s).unwrap()).is_ok());

        let bad_title = PrintRequestView {
            generation: 1,
            title: "a\u{7}b".into(),
            body: "x".into(),
        };
        let err = build_request(&bad_title).unwrap_err();
        assert_eq!(
            to_outcome(Err(err)),
            Err(PrintFailure::Invalid("見出し".into()))
        );
        let bad_body = PrintRequestView {
            title: "t".into(),
            body: "a\tb".into(),
            ..bad_title
        };
        let err = build_request(&bad_body).unwrap_err();
        assert_eq!(
            to_outcome(Err(err)),
            Err(PrintFailure::Invalid("本文".into()))
        );
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    mod sync_path {
        use super::*;
        use fandhe_device::print::UnsupportedPrint;
        use std::sync::atomic::{AtomicU32, Ordering};

        /// 呼び出し回数を数え、決めた結果を返す偽実装。
        struct Fake {
            calls: AtomicU32,
            result: fn() -> Result<PrintOutcome, DeviceError>,
        }

        impl Print for Fake {
            fn print(&self, _r: &PrintRequest) -> Result<PrintOutcome, DeviceError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                (self.result)()
            }
        }

        fn fake(result: fn() -> Result<PrintOutcome, DeviceError>) -> Fake {
            Fake {
                calls: AtomicU32::new(0),
                result,
            }
        }

        /// 押下して 1 回ドライバを回し、反映後の状態を返す。
        fn run(printer: &dyn Print) -> PrintPageState {
            let mut s = PrintPageState::default();
            screen::print(&mut s);
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(&s, &sender, printer);
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            s
        }

        #[test]
        fn no_request_never_calls_os() {
            let f = fake(|| Ok(PrintOutcome::Submitted));
            let s = PrintPageState::default();
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(&s, &sender, &f);
            assert!(rx.try_recv().is_err());
            assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn submitted_calls_os_once() {
            let f = fake(|| Ok(PrintOutcome::Submitted));
            let s = run(&f);
            assert_eq!(s.status, screen::SUBMITTED_STATUS);
            assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn cancelled_is_neither_error_nor_submitted() {
            let f = fake(|| Ok(PrintOutcome::Cancelled));
            let s = run(&f);
            assert_eq!(s.status, screen::CANCELLED_STATUS);
            assert!(s.error.is_empty());
            assert_ne!(s.status, screen::SUBMITTED_STATUS);
        }

        #[test]
        fn unconfirmed_is_not_shown_as_submitted() {
            let f = fake(|| Ok(PrintOutcome::Unconfirmed));
            let s = run(&f);
            assert_eq!(s.status, screen::UNCONFIRMED_STATUS);
            assert_ne!(s.status, screen::SUBMITTED_STATUS);
            assert!(s.error.is_empty());
        }

        #[test]
        fn unsupported_shows_unsupported_not_success() {
            let s = run(&UnsupportedPrint);
            assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
            assert!(s.error.is_empty());
            assert_ne!(s.status, screen::SUBMITTED_STATUS);
        }

        #[test]
        fn failure_goes_to_error_with_fixed_label() {
            let f = fake(|| Err(DeviceError::Internal("print failed")));
            let s = run(&f);
            assert!(s.status.is_empty());
            assert!(s.error.starts_with(screen::FAILED_PREFIX));
            assert!(!s.error.contains("print"));
        }

        #[test]
        fn stale_generation_completion_is_dropped() {
            let mut s = PrintPageState::default();
            screen::print(&mut s);
            mark_printing(&mut s);
            let stale = s.generation.wrapping_sub(1);
            finish_print(&mut s, stale, Ok(CoreOutcome::Submitted));
            assert!(s.status.is_empty());
        }
    }
}

#[cfg(all(test, any(target_os = "ios", target_os = "android")))]
mod bridge_tests {
    use super::bridge;
    use super::*;

    #[test]
    fn stale_request_id_is_dropped_and_second_deliver_ignored() {
        let id = bridge::begin();
        assert!(!bridge::deliver(id + 1000, Ok(PrintOutcome::Submitted)));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(id, Ok(PrintOutcome::Cancelled)));
        assert!(!bridge::deliver(id, Ok(PrintOutcome::Submitted)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(ready, Some(Ok(CoreOutcome::Cancelled)));
        bridge::SLOT.clear(id);
    }
}
