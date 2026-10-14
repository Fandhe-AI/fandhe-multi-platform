//! Phase 11（親 #1118）型サンプル「予定の追加」（Issue #1224 macOS・#1225 iOS）の
//! effect ドライバ。
//! `fandhe_core::screens::calendar_add`（端末依存ゼロ）の要求を観測し、検査
//! （`fandhe_device::calendar::EventDraft::parse`）を通ったものだけを OS へ渡して、
//! 結果を `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から
//! 呼ばれる契約、`device/biometric_auth.rs` と同型）。契約と守る範囲は
//! `fandhe_device::calendar` のモジュール doc を参照。
//!
//! - macOS / iOS: EventKit（iOS は EventKitUI の編集画面）はコールバック型のため
//!   `CalendarEvents` を通らず、pull 型 [`bridge`]（`RequestSlot` の facade）で
//!   毎フレーム [`drive_callback`] が結果を反映する
//! - Android（#1226）: `Intent.ACTION_INSERT` でカレンダーアプリの編集画面を開く（権限なし）。
//!   同じ pull 型 [`bridge`]。編集画面へ渡せたら `HandedToOs`（「保存した」とは見せない）、
//!   アプリ無し・非前景・起動失敗は失敗
//! - それ以外（Linux・Windows）: `UnsupportedCalendar` を同期に呼び「非対応」を表示する

use fandhe_core::screens::calendar_add::{
    finish_add, mark_adding, take_request, AddFailure, AddOutcome as CoreOutcome, CalendarAddState,
};
use fandhe_device::calendar::{AddEventOutcome, EventDraft};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `DeviceError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする
/// （security.md A09）。
fn to_outcome(result: Result<AddEventOutcome, DeviceError>) -> Result<CoreOutcome, AddFailure> {
    match result {
        Ok(AddEventOutcome::Saved) => Ok(CoreOutcome::Saved),
        Ok(AddEventOutcome::HandedToOs) => Ok(CoreOutcome::HandedToOs),
        Ok(AddEventOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Err(DeviceError::PermissionDenied(_)) => Ok(CoreOutcome::Denied),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(DeviceError::InvalidInput(kind)) => Err(AddFailure::Invalid(
            match kind {
                "calendar title" => "件名は 1〜200 文字で入力してください",
                "calendar time" => "日時は YYYY-MM-DD HH:MM で入力してください",
                "calendar range" => "終了は開始以降にしてください",
                _ => "入力が不正です",
            }
            .to_string(),
        )),
        Err(other) => Err(AddFailure::Failed(other.to_string())),
    }
}

/// `.m` 側の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 0 | 保存した | `Saved` |
/// | 1 | 権限拒否 | `PermissionDenied` |
/// | 2 | macOS 14 / iOS 17 未満 | `Unsupported` |
/// | 3 | 保存失敗 | `Internal("calendar save")` |
/// | 4 | 権限要求がエラー | `Internal("calendar access")` |
/// | 5 | 既定カレンダーなし | `Internal("calendar default")` |
/// | 6 | `NSDate` へ変換できない | `Internal("calendar date")` |
/// | 7 | 利用者が編集画面を閉じた（iOS の Canceled / Deleted） | `Cancelled` |
/// | 8 | 編集画面を出せなかった（iOS） | `Internal("calendar present")` |
/// | 他 | 未知 | `Internal("calendar outcome")` |
///
/// ホストでもテストできるよう cfg は割かない（呼び出し元は iOS / macOS のみ）。
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(crate) fn outcome_from_code(code: i32) -> Result<AddEventOutcome, DeviceError> {
    match code {
        0 => Ok(AddEventOutcome::Saved),
        1 => Err(DeviceError::PermissionDenied("calendar")),
        2 => Err(DeviceError::Unsupported("calendar")),
        3 => Err(DeviceError::Internal("calendar save")),
        4 => Err(DeviceError::Internal("calendar access")),
        5 => Err(DeviceError::Internal("calendar default")),
        6 => Err(DeviceError::Internal("calendar date")),
        7 => Ok(AddEventOutcome::Cancelled),
        8 => Err(DeviceError::Internal("calendar present")),
        _ => Err(DeviceError::Internal("calendar outcome")),
    }
}

/// Android: `.java` 側の結果コード → 結果の写像（SSOT は `MainActivity.java` の
/// `CALENDAR_RESULT_*`。未知のコードは失敗に倒し成功を作らない）。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 0 | 編集画面へ渡した | `HandedToOs` |
/// | 1 | カレンダーアプリ無し | `Internal("calendar app")` |
/// | 2 | 非前景・破棄中で起動しなかった | `Internal("calendar background")` |
/// | 3 | その他の起動失敗 | `Internal("calendar launch")` |
/// | 他 | 未知 | `Internal("calendar outcome")` |
///
/// macOS 用 [`outcome_from_code`] とはコード体系が違うため別関数にする。
/// ホストでもテストできるよう cfg は割かない。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn outcome_from_android_code(code: i32) -> Result<AddEventOutcome, DeviceError> {
    match code {
        0 => Ok(AddEventOutcome::HandedToOs),
        1 => Err(DeviceError::Internal("calendar app")),
        2 => Err(DeviceError::Internal("calendar background")),
        3 => Err(DeviceError::Internal("calendar launch")),
        _ => Err(DeviceError::Internal("calendar outcome")),
    }
}

/// OS 側コールバックとの橋渡し（macOS / iOS / Android、pull 型、`RequestSlot` の facade）。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use fandhe_core::screens::calendar_add::{AddFailure, AddOutcome};
    use fandhe_device::calendar::AddEventOutcome;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<AddOutcome, AddFailure>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 提示直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果あり
    /// は捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(request_id: u64, result: Result<AddEventOutcome, DeviceError>) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }

    /// Android: Activity 再生成直後に `android_main` から呼ぶ。[`SLOT`] はプロセス static の
    /// ため再生成をまたいで残る。保留中の要求は復元せず破棄し、以後届く旧 `request_id` の
    /// [`deliver`] を無視させる（`notification::bridge::reset` と同じ位置づけ）。
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

/// OS へ編集画面（または保存）を依頼する。`false` は依頼自体の失敗（呼び出し元が
/// `Err(Internal("calendar present"))` として畳む）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn present(request_id: u64, draft: &EventDraft) -> bool {
    super::apple_calendar::present(request_id, draft)
}

#[cfg(target_os = "android")]
fn present(request_id: u64, draft: &EventDraft) -> bool {
    super::android::request_calendar_insert(request_id, draft)
}

/// macOS / iOS / Android 版の effect ドライバ本体。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
fn drive_callback(state: &CalendarAddState, sender: &CompletionSender<CalendarAddState>) {
    use fandhe_core::screens::calendar_add::AddPhase;

    if let Some(request) = take_request(state) {
        let _ = sender.send(Box::new(mark_adding));
        let request_id = bridge::begin();
        match EventDraft::parse(&request.title, &request.start, &request.end) {
            Ok(draft) => {
                if !present(request_id, &draft) {
                    // 依頼自体が同期的に失敗した（コールバックは二度と来ない）。
                    bridge::deliver(request_id, Err(DeviceError::Internal("calendar present")));
                }
            }
            // 入力不正は OS を呼ばずに画面へ返す。
            Err(e) => {
                bridge::deliver(request_id, Err(e));
            }
        }
        return;
    }

    if state.phase != AddPhase::Adding {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut CalendarAddState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != AddPhase::Idle {
                finish_add(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// `registry.rs` から `calendar_add` 画面用に配線される effect ドライバ（macOS / iOS / Android）。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
pub fn calendar_add_effect(state: &CalendarAddState, sender: &CompletionSender<CalendarAddState>) {
    drive_callback(state, sender);
}

/// 非対応 OS: 検査してから `UnsupportedCalendar` を同期に呼ぶ（入力不正は先に入力不正
/// として返す）。
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
fn drive(
    state: &CalendarAddState,
    sender: &CompletionSender<CalendarAddState>,
    calendar: &dyn fandhe_device::calendar::CalendarEvents,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_adding));
    let result = EventDraft::parse(&request.title, &request.start, &request.end)
        .and_then(|draft| calendar.add_event(&draft));
    let outcome = to_outcome(result);
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut CalendarAddState| {
        finish_add(s, generation, outcome);
    }));
}

/// `registry.rs` から配線される effect ドライバ（非対応 OS。常に非対応を表示する）。
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
pub fn calendar_add_effect(state: &CalendarAddState, sender: &CompletionSender<CalendarAddState>) {
    drive(state, sender, &fandhe_device::calendar::UnsupportedCalendar);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_outcome_covers_all_paths() {
        assert_eq!(
            to_outcome(Ok(AddEventOutcome::Saved)),
            Ok(CoreOutcome::Saved)
        );
        assert_eq!(
            to_outcome(Ok(AddEventOutcome::HandedToOs)),
            Ok(CoreOutcome::HandedToOs)
        );
        assert_eq!(
            to_outcome(Ok(AddEventOutcome::Cancelled)),
            Ok(CoreOutcome::Cancelled)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::PermissionDenied("calendar"))),
            Ok(CoreOutcome::Denied)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("calendar"))),
            Ok(CoreOutcome::Unsupported)
        );
        for kind in ["calendar title", "calendar time", "calendar range"] {
            assert!(matches!(
                to_outcome(Err(DeviceError::InvalidInput(kind))),
                Err(AddFailure::Invalid(_))
            ));
        }
        assert!(matches!(
            to_outcome(Err(DeviceError::Internal("calendar save"))),
            Err(AddFailure::Failed(_))
        ));
    }

    #[test]
    fn outcome_from_code_table() {
        assert!(matches!(outcome_from_code(0), Ok(AddEventOutcome::Saved)));
        assert!(matches!(
            outcome_from_code(1),
            Err(DeviceError::PermissionDenied("calendar"))
        ));
        assert!(matches!(
            outcome_from_code(2),
            Err(DeviceError::Unsupported("calendar"))
        ));
        assert!(matches!(
            outcome_from_code(7),
            Ok(AddEventOutcome::Cancelled)
        ));
        for code in [3, 4, 5, 6, 8, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_code(code),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[test]
    fn outcome_from_android_code_table() {
        assert!(matches!(
            outcome_from_android_code(0),
            Ok(AddEventOutcome::HandedToOs)
        ));
        for code in [1, 2, 3, 4, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_android_code(code),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    mod non_macos {
        use super::*;
        use fandhe_core::screens::calendar_add::{add, AddPhase};

        fn run(state: &mut CalendarAddState) {
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(
                state,
                &sender,
                &fandhe_device::calendar::UnsupportedCalendar,
            );
            while let Ok(apply) = rx.try_recv() {
                apply(state);
            }
        }

        #[test]
        fn shows_unsupported_never_added() {
            let mut s = CalendarAddState {
                title: "会議".into(),
                ..Default::default()
            };
            add(&mut s);
            run(&mut s);
            assert_eq!(s.phase, AddPhase::Idle);
            assert!(!s.status.contains("追加しました"));
            assert!(!s.status.is_empty());
        }

        #[test]
        fn invalid_input_is_reported_as_invalid() {
            let mut s = CalendarAddState::default(); // 件名が空
            add(&mut s);
            run(&mut s);
            assert!(s.error.starts_with("入力を確認してください"));
        }
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
mod bridge_tests {
    use super::bridge;
    use super::*;

    #[test]
    fn stale_request_id_is_dropped_and_second_deliver_ignored() {
        let id = bridge::begin();
        assert!(!bridge::deliver(id + 1000, Ok(AddEventOutcome::Saved)));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(id, Ok(AddEventOutcome::Cancelled)));
        assert!(!bridge::deliver(id, Ok(AddEventOutcome::Saved)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(ready, Some(Ok(CoreOutcome::Cancelled)));
        bridge::SLOT.clear(id);
    }
}
