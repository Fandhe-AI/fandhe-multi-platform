//! Phase 11（親 #1118）型サンプル「予定の追加」（Issue #1224 macOS・#1225 iOS）の
//! effect ドライバ。
//! `fandhe_core::screens::calendar_add`（端末依存ゼロ）の要求を観測し、検査
//! （`crate::contract::EventDraft::parse`）を通ったものだけを OS へ渡して、
//! 結果を `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から
//! 呼ばれる契約、`fandhe-cap-biometric-auth` の `driver.rs` と同型）。契約と守る範囲は
//! `crate::contract` のモジュール doc を参照。
//!
//! - macOS / iOS: EventKit（iOS は EventKitUI の編集画面）はコールバック型のため
//!   `CalendarEvents` を通らず、pull 型 [`bridge`]（`RequestSlot` の facade）で
//!   毎フレーム [`drive_callback`] が結果を反映する
//! - Android（#1226）: `Intent.ACTION_INSERT` でカレンダーアプリの編集画面を開く（権限なし）。
//!   同じ pull 型 [`bridge`]。編集画面へ渡せたら `HandedToOs`（「保存した」とは見せない）、
//!   アプリ無し・非前景・起動失敗は失敗
//! - それ以外（Linux・Windows）: `UnsupportedCalendar` を同期に呼び「非対応」を表示する

use crate::contract::{AddEventOutcome, EventDraft};
use crate::error::CalendarError;
use fandhe_core::screens::calendar_add::{
    AddFailure, AddOutcome as CoreOutcome, CalendarAddState, finish_add, mark_adding, take_request,
};

use fandhe_runtime::CompletionSender;

/// `CalendarError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする
/// （security.md A09）。
fn to_outcome(result: Result<AddEventOutcome, CalendarError>) -> Result<CoreOutcome, AddFailure> {
    match result {
        Ok(AddEventOutcome::Saved) => Ok(CoreOutcome::Saved),
        Ok(AddEventOutcome::HandedToOs) => Ok(CoreOutcome::HandedToOs),
        Ok(AddEventOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Err(CalendarError::PermissionDenied) => Ok(CoreOutcome::Denied),
        Err(CalendarError::Unsupported) => Ok(CoreOutcome::Unsupported),
        Err(CalendarError::InvalidInput(kind)) => Err(AddFailure::Invalid(
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
pub(crate) fn outcome_from_code(code: i32) -> Result<AddEventOutcome, CalendarError> {
    match code {
        0 => Ok(AddEventOutcome::Saved),
        1 => Err(CalendarError::PermissionDenied),
        2 => Err(CalendarError::Unsupported),
        3 => Err(CalendarError::Internal("calendar save")),
        4 => Err(CalendarError::Internal("calendar access")),
        5 => Err(CalendarError::Internal("calendar default")),
        6 => Err(CalendarError::Internal("calendar date")),
        7 => Ok(AddEventOutcome::Cancelled),
        8 => Err(CalendarError::Internal("calendar present")),
        _ => Err(CalendarError::Internal("calendar outcome")),
    }
}

/// Android: `.java` 側の結果コード → 結果の写像（SSOT は `CalendarHelper.java` の
/// `RESULT_*`。未知のコードは失敗に倒し成功を作らない）。
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
pub(crate) fn outcome_from_android_code(code: i32) -> Result<AddEventOutcome, CalendarError> {
    match code {
        0 => Ok(AddEventOutcome::HandedToOs),
        1 => Err(CalendarError::Internal("calendar app")),
        2 => Err(CalendarError::Internal("calendar background")),
        3 => Err(CalendarError::Internal("calendar launch")),
        _ => Err(CalendarError::Internal("calendar outcome")),
    }
}

/// OS 側コールバックとの橋渡し（macOS / iOS / Android、pull 型、`RequestSlot` の facade）。
/// 本機能専用の [`SLOT`](bridge::SLOT) を、画面 effect（`SCREEN_OWNER`）と公開入口（[`begin_add_event`]・同期の
/// [`platform_calendar`]、`PUBLIC`）が持ち主の印で分けて共有する（`consumer-app-structure.md` §8.2）。
/// 結果は OS の生の値（`AddEventOutcome` / `CalendarError`）で置き、画面用への写像（`to_outcome`）は画面が取り出すときに行う。
/// ホストのテストでも slot の扱いを確かめるため、`test` でもコンパイルする（OS 呼び出しは含まない）。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android", test))]
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", target_os = "android")),
    allow(dead_code)
)]
pub(crate) mod bridge {
    use crate::contract::AddEventOutcome;
    use crate::error::CalendarError;
    use fandhe_runtime::request_slot::{PublicEntry, RequestSlot, SlotOwner};

    pub(crate) type Outcome = Result<AddEventOutcome, CalendarError>;

    /// busy で断るときの失敗の識別子（先行の要求が進行中、または自分の結果が未取得）。
    pub(crate) const BUSY: &str = "calendar busy";
    /// 要求が失われた（Activity 再生成の `reset`）ときの失敗の識別子。
    pub(crate) const RESET: &str = "calendar reset";

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();
    /// 画面 effect が追う自分の分の要求。
    pub(crate) static SCREEN_OWNER: SlotOwner = SlotOwner::new();
    /// 公開入口が追う自分の分の要求（poll・discard の直列化を含む）。
    pub(crate) static PUBLIC: PublicEntry = PublicEntry::new();

    /// 画面側の開始。進行中の要求と、公開側の未取得の結果は置き換えない。自分の未取得の結果は置き換える。
    /// OS 側（Android の `RequestTracker`・iOS の保持 delegate・macOS の権限応答待ち）は同時に 1 要求しか持てず、
    /// 後続が先行を置き換えても先行の保存は取り消せない（失敗を返したのに後から保存され、再試行で重複する）ため、置き換えない。
    pub(crate) fn begin_screen() -> Option<u64> {
        SCREEN_OWNER.begin_excluding(&SLOT, &[PUBLIC.owner()])
    }

    /// 公開入口の開始。進行中の要求と自分の未取得の結果があれば `None`（busy）。画面側の届いた未取得の結果は
    /// 捨てずに画面の受け取り待ちへ移す。
    pub(crate) fn begin_public() -> Option<u64> {
        PUBLIC.begin(&SLOT, &SCREEN_OWNER)
    }

    /// OS からの結果を置く。`request_id` 不一致・未登録・既に結果ありは捨てて `false`（先に届いたものだけを採る）。
    /// 打ち切られた要求の遅れた結果は持ち主がいないため、次の開始が置き換えて捨てる。
    pub(crate) fn deliver(request_id: u64, result: Outcome) -> bool {
        SLOT.deliver(request_id, result)
    }

    /// 失われた要求を公開側へ知らせる値。
    pub(crate) fn lost() -> Outcome {
        Err(CalendarError::Internal(RESET))
    }

    /// Android: Activity 再生成直後に `android_main` から呼ぶ。[`SLOT`] はプロセス static のため再生成をまたいで残る。
    /// 保留中の要求は復元せず破棄し、以後届く旧 `request_id` の [`deliver`] を無視させる。持ち主の印は残し、
    /// 画面は次の poll で失敗に、公開側（同期の待ちを含む）は次の poll で失われた要求を 1 度だけ通知される。
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset();
    }
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
}

/// OS へ編集画面（または保存）を依頼する。`false` は依頼自体の失敗（呼び出し元が
/// `Err(Internal("calendar present"))` として畳む）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn present(request_id: u64, draft: &EventDraft) -> bool {
    crate::apple::present(request_id, draft)
}

#[cfg(target_os = "android")]
fn present(request_id: u64, draft: &EventDraft) -> bool {
    crate::android::request_calendar_insert(request_id, draft)
}

/// 画面の状態型に依存しない入口。現在の OS の `CalendarEvents` 実装を返す
/// （`docs/architecture/consumer-app-structure.md` §8.2）。
///
/// macOS / iOS / Android は OS 側がコールバックで結果を返すため、`add_event` は OS の完了まで待つ（期限は置かない。
/// OS の操作は取り消せず、推測の期限で失敗を返すと後から保存されて再試行で重複するため）。UI スレッド
/// （iOS / macOS のメイン）から呼ばない。待ちを打ち切りたいときは [`begin_add_event`] / [`poll_add_event`] /
/// [`discard_add_event`] を使う。同時に 1 件だけで、先行が終わるまで後から呼ぶと `Internal("calendar busy")` で
/// 即座に戻る（先行は置き換えない）。Activity 再生成で待っている呼び出しは `Internal("calendar reset")` で戻る。
/// それ以外の OS は常に `Unsupported`。
pub fn platform_calendar() -> std::sync::Arc<dyn crate::contract::CalendarEvents> {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    {
        std::sync::Arc::new(CallbackCalendar)
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    {
        std::sync::Arc::new(crate::contract::UnsupportedCalendar)
    }
}

/// コールバック型 OS 向けの同期ラッパ（[`platform_calendar`]）。[`begin_add_event`] と同じ経路で始め、
/// 結果が届くか要求が失われるまで待つ。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
struct CallbackCalendar;

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
impl crate::contract::CalendarEvents for CallbackCalendar {
    fn add_event(&self, draft: &EventDraft) -> Result<AddEventOutcome, CalendarError> {
        let request_id = begin_add_event(draft)?;
        bridge::PUBLIC.wait(&bridge::SLOT, request_id, bridge::lost)
    }
}

/// 公開入口（macOS / iOS / Android）: 検査済みの予定を OS へ渡し、結果を受け取るための要求 id を返す。
/// 結果は [`poll_add_event`] で 1 度だけ取り出す。待ちを打ち切るときは [`discard_add_event`] で捨てる
/// （同期の [`platform_calendar`] は OS の完了まで待つ。打ち切れるのはこちら）。
///
/// - 進行中の要求（画面経由を含む）と、自分の結果が未取得のうちは `Internal("calendar busy")` で断る
/// - 画面側の届いた未取得の結果は捨てずに画面の受け取り待ちへ移して始める（画面はそのまま受け取る）
/// - 依頼自体の失敗は `Err`（結果は届かない）。macOS・iOS・Android 以外では `Unsupported`
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
pub fn begin_add_event(draft: &EventDraft) -> Result<u64, CalendarError> {
    let request_id = bridge::begin_public().ok_or(CalendarError::Internal(bridge::BUSY))?;
    if !present(request_id, draft) {
        bridge::PUBLIC.abort(&bridge::SLOT, request_id);
        return Err(CalendarError::Internal("calendar present"));
    }
    Ok(request_id)
}

/// macOS・iOS・Android 以外では成功へ倒さず `Unsupported`。
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
pub fn begin_add_event(draft: &EventDraft) -> Result<u64, CalendarError> {
    let _ = draft;
    Err(CalendarError::Unsupported)
}

/// [`begin_add_event`] の結果を取り出す。届いていれば 1 度だけ `Some`。未着（`None` のまま）なら OS はまだ進行中で、
/// 待てないときは [`discard_add_event`] で打ち切る。旧 id・公開入口でない id・取り出し済み・捨てた id・0・
/// 対象外の OS は `None`。要求が失われた（Android の Activity 再生成）ときは `Err(Internal("calendar reset"))` を 1 度だけ返す。
///
/// 打ち切った（`discard` した）要求は、OS 側で後から保存されうる（未確定）。そのため結果は届かない。
/// 並行して同じ id を poll しても、結果が二重に返ることはない。
pub fn poll_add_event(request_id: u64) -> Option<Result<AddEventOutcome, CalendarError>> {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    {
        bridge::PUBLIC.poll(&bridge::SLOT, request_id, bridge::lost)
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    {
        let _ = request_id;
        None
    }
}

/// [`begin_add_event`] の自分の要求を捨てて待ちを打ち切る。`request_id` が公開入口の現在の要求のときだけ `true`
/// （他人の id・取り出し済み・0・対象外の OS は `false`）。以後 [`poll_add_event`] は `None`。
///
/// 外すのは受け取りの印だけ（OS が進行中なら slot は残す）で、OS の操作は取り消せない。捨てた要求は OS 側で後から保存されうる（未確定）。
/// 後から届いた結果は id で照合して捨て、新しい要求へ結びつけない。OS がまだ進行中の間は、次の開始
/// （画面・公開とも）は `Internal("calendar busy")` で断られる（重複して保存しないため）。
pub fn discard_add_event(request_id: u64) -> bool {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    {
        bridge::PUBLIC.discard(&bridge::SLOT, request_id)
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    {
        let _ = request_id;
        false
    }
}

/// macOS / iOS / Android 版の effect ドライバ本体。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
fn drive_callback(state: &CalendarAddState, sender: &CompletionSender<CalendarAddState>) {
    use fandhe_core::screens::calendar_add::AddPhase;

    if let Some(request) = take_request(state) {
        let _ = sender.send(Box::new(mark_adding));
        let generation = request.generation;
        // 進行中の要求と公開入口の未取得の結果は置き換えず、画面には処理中の失敗を返す。
        let Some(request_id) = bridge::begin_screen() else {
            let outcome = to_outcome(Err(CalendarError::Internal(bridge::BUSY)));
            let _ = sender.send(Box::new(move |s: &mut CalendarAddState| {
                finish_add(s, generation, outcome);
            }));
            return;
        };
        match EventDraft::parse(&request.title, &request.start, &request.end) {
            Ok(draft) => {
                if !present(request_id, &draft) {
                    // 依頼自体が同期的に失敗した（コールバックは二度と来ない）。
                    bridge::deliver(request_id, Err(CalendarError::Internal("calendar present")));
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
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &bridge::SCREEN_OWNER,
        sender,
        |_request_id| {},
        move |s: &mut CalendarAddState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != AddPhase::Idle {
                finish_add(s, generation, to_outcome(outcome));
                true
            } else {
                false
            }
        },
        // 自分の要求が失われた（Activity 再生成の reset 等）。待ち続けず失敗で終える。
        move |s: &mut CalendarAddState| {
            finish_add(
                s,
                generation,
                to_outcome(Err(CalendarError::Internal(bridge::RESET))),
            );
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
    calendar: &dyn crate::contract::CalendarEvents,
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
    drive(state, sender, &crate::contract::UnsupportedCalendar);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    #[allow(dead_code)]
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

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
            to_outcome(Err(CalendarError::PermissionDenied)),
            Ok(CoreOutcome::Denied)
        );
        assert_eq!(
            to_outcome(Err(CalendarError::Unsupported)),
            Ok(CoreOutcome::Unsupported)
        );
        for kind in ["calendar title", "calendar time", "calendar range"] {
            assert!(matches!(
                to_outcome(Err(CalendarError::InvalidInput(kind))),
                Err(AddFailure::Invalid(_))
            ));
        }
        assert!(matches!(
            to_outcome(Err(CalendarError::Internal("calendar save"))),
            Err(AddFailure::Failed(_))
        ));
    }

    #[test]
    fn outcome_from_code_table() {
        assert!(matches!(outcome_from_code(0), Ok(AddEventOutcome::Saved)));
        assert!(matches!(
            outcome_from_code(1),
            Err(CalendarError::PermissionDenied)
        ));
        assert!(matches!(
            outcome_from_code(2),
            Err(CalendarError::Unsupported)
        ));
        assert!(matches!(
            outcome_from_code(7),
            Ok(AddEventOutcome::Cancelled)
        ));
        for code in [3, 4, 5, 6, 8, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_code(code),
                Err(CalendarError::Internal(_))
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
                Err(CalendarError::Internal(_))
            ));
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    mod non_macos {
        use super::*;
        use fandhe_core::screens::calendar_add::{AddPhase, add};

        fn run(state: &mut CalendarAddState) {
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
            drive(state, &sender, &crate::contract::UnsupportedCalendar);
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

/// 公開入口（begin / poll / discard と同期の待ち）の slot の扱い。OS 呼び出しを含まないためホストで走る。
#[cfg(test)]
mod bridge_tests {
    use super::bridge;
    use super::*;

    /// `bridge` のプロセス共有 static を触るテストを直列化し、始める前に空へ戻す。
    static BRIDGE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn fresh() -> std::sync::MutexGuard<'static, ()> {
        let g = BRIDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::SLOT.reset();
        bridge::SCREEN_OWNER.release(bridge::SCREEN_OWNER.current());
        bridge::PUBLIC
            .owner()
            .release(bridge::PUBLIC.owner().current());
        g
    }

    fn poll(id: u64) -> Option<Result<AddEventOutcome, CalendarError>> {
        bridge::PUBLIC.poll(&bridge::SLOT, id, bridge::lost)
    }

    #[test]
    fn stale_request_id_is_dropped_and_second_deliver_ignored() {
        let _g = fresh();
        let id = bridge::begin_public().unwrap();
        assert!(!bridge::deliver(id + 1000, Ok(AddEventOutcome::Saved)));
        assert_eq!(poll(id), None);
        assert!(bridge::deliver(id, Ok(AddEventOutcome::Cancelled)));
        assert!(!bridge::deliver(id, Ok(AddEventOutcome::Saved)));
        assert_eq!(poll(id), Some(Ok(AddEventOutcome::Cancelled)));
        assert_eq!(poll(id), None);
    }

    /// 打ち切り: poll は `None` のまま戻れ、OS が進行中の間は次の開始が busy。後から届いた結果は捨て、
    /// 次の要求へ結びつかない（重複して保存させない）。
    #[test]
    fn discard_gives_up_waiting_and_drops_the_late_result() {
        let _g = fresh();
        let first = bridge::begin_public().unwrap();
        assert_eq!(poll(first), None);
        assert!(bridge::PUBLIC.discard(&bridge::SLOT, first));
        assert_eq!(poll(first), None);
        // OS はまだ進行中: 画面も公開も始められない。
        assert!(bridge::begin_public().is_none());
        assert!(bridge::begin_screen().is_none());
        // 遅れて完了。結果は持ち主がいない。
        assert!(bridge::deliver(first, Ok(AddEventOutcome::Saved)));
        let second = bridge::begin_public().expect("OS の完了後は次を受け付ける");
        assert_ne!(first, second);
        assert_eq!(poll(first), None);
        assert!(!bridge::deliver(first, Ok(AddEventOutcome::Saved)));
        assert_eq!(poll(second), None, "古い結果が新しい要求へ結びつかない");
        assert!(bridge::deliver(second, Ok(AddEventOutcome::Cancelled)));
        assert_eq!(poll(second), Some(Ok(AddEventOutcome::Cancelled)));
    }

    /// 同期の待ち（`platform_calendar` が使う `wait`）は、結果が届けば戻り、再生成の `reset` でも
    /// `calendar reset` で戻る（止まったままにならない）。
    #[test]
    fn sync_wait_returns_on_result_and_on_reset() {
        let _g = fresh();
        let id = bridge::begin_public().unwrap();
        let h = std::thread::spawn(move || bridge::PUBLIC.wait(&bridge::SLOT, id, bridge::lost));
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert!(bridge::deliver(id, Ok(AddEventOutcome::Saved)));
        assert_eq!(h.join().unwrap(), Ok(AddEventOutcome::Saved));

        let id = bridge::begin_public().unwrap();
        let h = std::thread::spawn(move || bridge::PUBLIC.wait(&bridge::SLOT, id, bridge::lost));
        std::thread::sleep(std::time::Duration::from_millis(60));
        bridge::reset();
        assert_eq!(
            h.join().unwrap(),
            Err(CalendarError::Internal(bridge::RESET))
        );
        assert!(bridge::begin_public().is_some());
    }

    /// 画面と公開入口は互いの進行中の要求を置き換えない。画面側の届いた未取得の結果は、公開入口の開始が
    /// 捨てずに画面の受け取り待ちへ移す。
    #[test]
    fn screen_and_public_exclude_each_other() {
        let _g = fresh();
        let screen = bridge::begin_screen().unwrap();
        assert!(bridge::begin_public().is_none());
        assert!(bridge::deliver(screen, Ok(AddEventOutcome::Saved)));
        let public = bridge::begin_public().expect("画面の結果は退避して始める");
        assert_eq!(
            bridge::SLOT.snapshot_of(screen),
            Some(Some(Ok(AddEventOutcome::Saved)))
        );
        assert!(bridge::begin_screen().is_none());
        assert!(bridge::deliver(public, Ok(AddEventOutcome::HandedToOs)));
        // 公開側の未取得の結果は画面から守る。
        assert!(bridge::begin_screen().is_none());
        assert_eq!(poll(public), Some(Ok(AddEventOutcome::HandedToOs)));
        assert!(bridge::begin_screen().is_some());
    }

    /// 画面の要求が `reset` で失われたら、処理中のまま止まらず失敗で終え、持ち主の印も外す。
    /// OS を呼ばないよう、依頼済みの状態（`Adding`・持ち主の印あり）から始める。
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    #[test]
    fn screen_request_lost_by_reset_ends_in_failure() {
        use fandhe_core::screens::calendar_add::AddPhase;
        struct NoopWaker;
        impl fandhe_runtime::RepaintWaker for NoopWaker {
            fn wake(&self) {}
            fn wake_after(&self, _delay: std::time::Duration) {}
        }
        let _g = fresh();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        let mut st = CalendarAddState {
            phase: AddPhase::Adding,
            ..Default::default()
        };
        bridge::begin_screen().unwrap();
        bridge::reset();
        drive_callback(&st.clone(), &sender);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut st);
        }
        assert_eq!(st.phase, AddPhase::Idle);
        assert!(!st.error.is_empty());
        assert_eq!(bridge::SCREEN_OWNER.current(), 0);
    }
}
