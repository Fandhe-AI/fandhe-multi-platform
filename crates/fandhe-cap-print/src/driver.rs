//! Phase 11（親 #94）型サンプル「印刷」（Issue #325）の effect ドライバ。
//! `fandhe_core::screens::print_page`（端末依存ゼロ）の要求を観測し、値を
//! `crate::contract::PrintRequest` で検査してから OS へ渡し、結果を `CompletionSender` 経由で
//! 返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約）。
//! 契約と守る範囲は `contract` のモジュール doc を参照。
//!
//! - iOS / Android: コールバック型のため pull 型 [`bridge`]（`RequestSlot` の facade）で毎フレーム
//!   `drive_callback` が結果を反映する。Android（#328）・iOS（#327）とも実装済み。
//!   **dispatch はここで最終形にしてあり、後続 issue は OS 別
//!   モジュールだけを埋める**
//! - macOS / Windows / それ以外: [`Print`] を UI スレッドで同期に呼ぶ（同期モーダル）。Linux は
//!   `UnsupportedPrint` で「非対応」を表示する
//!
//! # 画面の内容の印刷（print-screen、親 #341、Issue #1010）
//!
//! `fandhe_core::screens::print_screen_page`（状態は `print_page` と共用）の要求を
//! [`print_screen_page_effect`] が観測し、次の順で進める。ダイアログ・結果の三値・橋渡しは
//! 文章の印刷と共用する。
//!
//! 1. 取り込み段（全 OS 共通）: 要求の世代を [`CAPTURE`] に期限（単調時計 `Instant` で 2 秒）つきで置き、
//!    `HostView::request_screenshot` で次フレームの読み戻しを依頼する。届いた読み戻しは世代タグで照合し、
//!    [`capture_step`] が判定と除去を同じロックの中で行う（契約表 2）。期限切れ・寸法不正は
//!    OS を呼ばず `Internal("print capture")` で確定する（ダイアログを出す前の失敗）
//! 2. OS への受け渡し: macOS / Windows は [`Print::print_image`] を UI スレッドで同期に呼ぶ。
//!    iOS / Android は OS 別モジュールの `begin_print_image` に委譲して [`bridge`] をポーリングする
//!    （**後続の b-ios #1011 / b-android #1012 は自分の OS 別モジュールだけを書き換える**。
//!    dispatch・fallback・core 画面は触らない）。Linux ほかは取り込まず非対応で即確定する
//!
//! 背景にいる間は読み戻しが届かず、期限切れで失敗になる（守らない範囲）。

use std::sync::Mutex;
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
use std::time::Duration;
use std::time::Instant;

use crate::contract::{PrintImage, PrintOutcome, PrintRequest};
use crate::error::PrintError;
use fandhe_core::screens::print_page::{
    CoreOutcome, PrintFailure, PrintPageState, PrintRequestView, take_request,
};
use fandhe_core::screens::print_screen_page::CAPTURE_FAILED_TEXT;

#[cfg(not(any(target_os = "ios", target_os = "android")))]
use crate::contract::Print;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_core::screens::print_page::{finish_print, mark_printing};

use fandhe_runtime::CompletionSender;
use fandhe_runtime::Screenshot;
use fandhe_runtime::request_slot::POLL_INTERVAL;

/// 画面の取り込みを待つ期限（単調時計で測る。契約表 3）。
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
const CAPTURE_DEADLINE: Duration = Duration::from_secs(2);

/// `PrintError` を画面用の結果へ写像する。内部構造・入力値は出さず固定文言だけにする
/// （security.md A09）。
fn to_outcome(result: Result<PrintOutcome, PrintError>) -> Result<CoreOutcome, PrintFailure> {
    match result {
        Ok(PrintOutcome::Submitted) => Ok(CoreOutcome::Submitted),
        Ok(PrintOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Ok(PrintOutcome::Unconfirmed) => Ok(CoreOutcome::Unconfirmed),
        Err(PrintError::Unsupported) => Ok(CoreOutcome::Unsupported),
        Err(PrintError::InvalidInput(kind)) => Err(PrintFailure::Invalid(
            match kind {
                "print title" => "見出し",
                "print body" => "本文",
                _ => "入力",
            }
            .to_string(),
        )),
        // 画面を取り込めない失敗（ダイアログを出す前に確定）。印刷の開始失敗と文言を分ける。
        Err(PrintError::Internal("print capture")) => {
            Err(PrintFailure::Failed(CAPTURE_FAILED_TEXT.to_string()))
        }
        Err(_) => Err(PrintFailure::Failed(
            "OS の印刷を開始できませんでした".to_string(),
        )),
    }
}

/// iOS の結果コード表（SSOT。`ios/fandhe_ios_print.m` の結果コードと一致させる）。未知のコードは失敗に倒す。
/// 0 = completed かつ error なし / 1 = completed NO かつ error なし（取り消し）/ 2 = error あり /
/// 3 = 提示不可（main 外）/ 4 = `isPrintingAvailable == NO`。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))] // iOS の結果受け口（`ios.rs`）が使う
pub(crate) fn outcome_from_ios_code(code: i32) -> Result<PrintOutcome, PrintError> {
    match code {
        0 => Ok(PrintOutcome::Submitted),
        1 => Ok(PrintOutcome::Cancelled),
        2 => Err(PrintError::Internal("print failed")),
        3 => Err(PrintError::Internal("print present")),
        4 => Err(PrintError::Unsupported),
        _ => Err(PrintError::Internal("print outcome")),
    }
}

/// Android の結果コード表（SSOT。Java `PrintHelper.RESULT_*` と一致させる）。
/// 0 = QUEUED / STARTED / BLOCKED / COMPLETED / 1 = CANCELED / 2 = 2 秒後も CREATED /
/// 3 = FAILED・onWrite 失敗 / 4 = `print()` 例外・Activity 無し。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn outcome_from_android_code(code: i32) -> Result<PrintOutcome, PrintError> {
    match code {
        0 => Ok(PrintOutcome::Submitted),
        1 => Ok(PrintOutcome::Cancelled),
        2 => Ok(PrintOutcome::Unconfirmed),
        3 => Err(PrintError::Internal("print failed")),
        4 => Err(PrintError::Internal("print present")),
        _ => Err(PrintError::Internal("print outcome")),
    }
}

/// 画面の値から検査済みの要求を作る。OS へ渡す唯一の入口（検査の SSOT は `PrintRequest`）。
fn build_request(view: &PrintRequestView) -> Result<PrintRequest, PrintError> {
    PrintRequest::new(&view.title, &view.body)
}

/// OS 側コールバックとの橋渡し（iOS / Android、pull 型、`RequestSlot` の facade）。
/// `print` と `print-screen` で共用する [`SLOT`](bridge::SLOT)（1 プロセスで動く画面は 1 つ）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use crate::contract::PrintOutcome;
    use crate::error::PrintError;

    use fandhe_runtime::request_slot::{RequestSlot, SlotOwner};

    /// OS の生の結果。画面用への写像は取り出す側が行う（画面経由でも公開入口でも同じ slot を使う）。
    pub(crate) type Outcome = Result<PrintOutcome, PrintError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 画面側の要求の持ち主印。公開入口の進行中・未取得の結果を画面側が置き換えないために使う。
    pub(crate) static SCREEN: SlotOwner = SlotOwner::new();

    /// 画面側の依頼直前に呼ぶ。公開入口の要求が進行中・未取得なら `None`（置き換えない）。
    pub(crate) fn begin() -> Option<u64> {
        SCREEN.begin_keeping_foreign(&SLOT)
    }

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    #[allow(dead_code)] // 後続 issue（b-ios / b-android）の受け口が使う
    pub(crate) fn deliver(request_id: u64, result: Result<PrintOutcome, PrintError>) -> bool {
        SLOT.deliver(request_id, result)
    }

    /// Android: Activity 再生成直後に登録表（`fandhe_runtime::teardown`）から呼ばれる。保留中の要求は復元せず破棄し、
    /// 以後届く旧 `request_id` の [`deliver`] を無視させる。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// Activity 再生成直後に登録表（`fandhe_runtime::teardown`）から呼ばれる。保留中の要求・取り込み待ちは
/// 復元せず破棄し、以後届く旧要求の結果を無視させる（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
    // 取り込み待ちも破棄する（保留は復元しない。契約表 1）。
    *CAPTURE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// OS へ印刷ダイアログの表示を依頼する。`Ok` は受理済みで結果は後から [`bridge::deliver`] で届く。
/// `Err` は依頼自体の失敗（呼び出し元がその場で `deliver` する）。
#[cfg(target_os = "ios")]
fn begin_print(request_id: u64, req: &PrintRequest) -> Result<(), PrintError> {
    crate::ios::begin_print(request_id, req)
}

#[cfg(target_os = "android")]
fn begin_print(request_id: u64, req: &PrintRequest) -> Result<(), PrintError> {
    crate::android::begin_print(request_id, req)
}

/// 画面の画像を印刷ダイアログへ渡す依頼（iOS / Android）。契約は [`begin_print`] と同じ。
/// 実体は OS 別モジュール（b-ios #1011 / b-android #1012 が埋める）。
#[cfg(target_os = "ios")]
fn begin_print_image(request_id: u64, img: &PrintImage) -> Result<(), PrintError> {
    crate::ios::begin_print_image(request_id, img)
}

#[cfg(target_os = "android")]
fn begin_print_image(request_id: u64, img: &PrintImage) -> Result<(), PrintError> {
    crate::android::begin_print_image(request_id, img)
}

/// 画面の状態型に依存しない公開入口（iOS / Android）。文章の印刷を依頼して要求 id を返す。
/// 結果は [`poll_print`] で取り出す。先行の要求（画面の保留を含む）が未取得のあいだは `Internal` で拒否し、置き換えない。
/// 依頼自体が失敗したときは保留を残さず `Err` を返す。
/// Android で Activity が破棄されると、追跡中の要求へ破棄時の OS のジョブ状態から決めた結果が配られる
/// （投入済みなら Submitted、取り消しなら Cancelled、確定できなければ Unconfirmed か失敗）。
/// 外部アプリは [`poll_print`] で取り出すまで次の要求が busy になる。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn begin_print_request(req: &PrintRequest) -> Result<u64, PrintError> {
    register_teardown();
    // 未取得の結果も守る（`keep` が常に真）。取り出すまで占有を維持し、新しい要求は busy にする。
    let id = bridge::SLOT
        .try_begin_unless(|_| true)
        .ok_or(PrintError::Internal("print busy"))?;
    begin_print(id, req).inspect_err(|_| bridge::SLOT.clear(id))?;
    Ok(id)
}

/// [`begin_print_request`] の画面の画像版。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn begin_print_image_request(img: &PrintImage) -> Result<u64, PrintError> {
    register_teardown();
    // 未取得の結果も守る（`keep` が常に真）。取り出すまで占有を維持し、新しい要求は busy にする。
    let id = bridge::SLOT
        .try_begin_unless(|_| true)
        .ok_or(PrintError::Internal("print busy"))?;
    begin_print_image(id, img).inspect_err(|_| bridge::SLOT.clear(id))?;
    Ok(id)
}

/// 要求 id の結果が届いていれば取り出して返す（取り出すと保留は消える）。未着・別の id・再生成で破棄済みは `None`。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn poll_print(request_id: u64) -> Option<Result<PrintOutcome, PrintError>> {
    register_teardown();
    bridge::SLOT.take_ready(request_id)
}

/// iOS / Android 版の effect ドライバ本体。保留中（`Printing`）は新規要求を受けない
/// （画面の phase が二重押しを止める。iOS の共有印刷コントローラの再 present 対策）。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn drive_callback(state: &PrintPageState, sender: &CompletionSender<PrintPageState>) {
    use fandhe_core::screens::print_page::mark_printing;

    if let Some(view) = take_request(state) {
        let _ = sender.send(Box::new(mark_printing));
        let Some(request_id) = bridge::begin() else {
            send_busy(sender, view.generation);
            return;
        };
        // 入力不正は OS へ渡さずその場で画面へ返す。
        let accepted = build_request(&view).and_then(|r| begin_print(request_id, &r));
        if let Err(e) = accepted {
            bridge::deliver(request_id, Err(e));
        }
        return;
    }
    poll_bridge(state, sender);
}

/// 公開入口の要求が進行中・未取得で画面側の要求を受けられないとき、画面へ失敗を返す。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn send_busy(sender: &CompletionSender<PrintPageState>, generation: u64) {
    use fandhe_core::screens::print_page::finish_print;

    let outcome = to_outcome(Err(PrintError::Internal("print busy")));
    let _ = sender.send(Box::new(move |s: &mut PrintPageState| {
        finish_print(s, generation, outcome);
    }));
}

/// [`bridge`] に届いた結果を毎フレーム反映する（`print` と `print-screen` で共用）。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn poll_bridge(state: &PrintPageState, sender: &CompletionSender<PrintPageState>) {
    use fandhe_core::screens::print_page::{PrintPhase, finish_print};

    if state.phase != PrintPhase::Printing {
        return;
    }
    let generation = state.generation;
    // 画面側の分だけを取る。公開入口の要求は触らない。slot が別 id へ変わった・空になったら失敗で確定する。
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &bridge::SCREEN,
        sender,
        |_request_id| {},
        move |s: &mut PrintPageState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != PrintPhase::Idle {
                finish_print(s, generation, to_outcome(outcome));
                true
            } else {
                false
            }
        },
        move |s: &mut PrintPageState| {
            if s.phase != PrintPhase::Idle {
                finish_print(
                    s,
                    generation,
                    to_outcome(Err(PrintError::Internal("print lost"))),
                );
            }
        },
    );
}

/// `registry.rs` から `print_page` 画面用に配線される effect ドライバ（iOS / Android）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn print_page_effect(state: &PrintPageState, sender: &CompletionSender<PrintPageState>) {
    register_teardown();
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

/// OS 別の [`Print`] 実装を返す（macOS / Windows / それ以外）。画面の状態型に依存しないため、
/// フレームワークの外のアプリも使える。iOS・Android は結果が後から届く型のため `Print` を持たず、effect ドライバだけを使う。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn platform_print() -> Box<dyn Print> {
    register_teardown();
    #[cfg(target_os = "macos")]
    return crate::macos::implementation();
    #[cfg(target_os = "windows")]
    return crate::windows::implementation();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(crate::contract::UnsupportedPrint);
}

/// `registry.rs` から配線される effect ドライバ（macOS・Windows・Linux）。
///
/// ponytail: 印刷ダイアログは同期モーダルなので UI スレッドで直接呼ぶ（`file_picker` が macOS で
/// rfd の同期ダイアログを呼ぶのと同じ判断）。戻らない呼び出しが見つかったら、作業スレッド +
/// bridge へ移す。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn print_page_effect(state: &PrintPageState, sender: &CompletionSender<PrintPageState>) {
    register_teardown();
    if take_request(state).is_none() {
        return;
    }
    drive(state, sender, platform_print().as_ref());
}

/// 取り込み待ちの要求（世代と期限）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingCapture {
    generation: u64,
    deadline: Instant,
}

/// 画面の取り込み待ち。1 プロセスで動く画面は 1 つなので 1 件だけ持つ。
static CAPTURE: Mutex<Option<PendingCapture>> = Mutex::new(None);

/// [`capture_step`] の判定結果。
#[derive(Debug)]
enum CaptureStep {
    /// 待ちが無い、または古い世代の待ちを捨てた。何もしない。
    Idle,
    /// 期限内で読み戻しが未着。
    Waiting,
    /// 取り込みが確定した（成功は画像、失敗は `Internal("print capture")`）。
    Done(Result<PrintImage, PrintError>),
}

/// 取り込み待ちを置く（既にある待ちは新しい要求で置き換える）。
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
fn arm_capture(slot: &Mutex<Option<PendingCapture>>, generation: u64, now: Instant) {
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(PendingCapture {
        generation,
        deadline: now + CAPTURE_DEADLINE,
    });
}

/// 取り込み待ちを 1 フレームぶん進める。判定と除去は同じロックの内側で行う（契約表 2）。
/// 期限後に届いた読み戻しは成功にせず捨てる（契約表 3）。`shot` は世代タグで照合済みの読み戻し。
fn capture_step(
    slot: &Mutex<Option<PendingCapture>>,
    generation: u64,
    now: Instant,
    shot: Option<Screenshot>,
) -> CaptureStep {
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pending) = *guard else {
        return CaptureStep::Idle;
    };
    if pending.generation != generation {
        *guard = None;
        return CaptureStep::Idle;
    }
    if now > pending.deadline {
        *guard = None;
        return CaptureStep::Done(Err(PrintError::Internal("print capture")));
    }
    let Some(shot) = shot else {
        return CaptureStep::Waiting;
    };
    *guard = None;
    let image = match (u32::try_from(shot.width), u32::try_from(shot.height)) {
        (Ok(w), Ok(h)) => PrintImage::new(shot.rgba, w, h),
        _ => Err(PrintError::Internal("print capture")),
    };
    CaptureStep::Done(image)
}

/// 結果を画面へ反映する完了を送る（世代が違えば `finish_print` が捨てる）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn send_finish(
    sender: &CompletionSender<PrintPageState>,
    generation: u64,
    result: Result<PrintOutcome, PrintError>,
) {
    let outcome = to_outcome(result);
    let _ = sender.send(Box::new(move |s: &mut PrintPageState| {
        finish_print(s, generation, outcome);
    }));
}

/// 同期経路（macOS・Windows・Linux）の 1 フレームぶん。取り込みが確定したら [`Print::print_image`] を
/// 呼ぶ。取り込み失敗では OS を呼ばない。テストから引数を注入できるよう本体を分けてある。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn drive_screen(
    state: &PrintPageState,
    sender: &CompletionSender<PrintPageState>,
    printer: &dyn Print,
    slot: &Mutex<Option<PendingCapture>>,
    now: Instant,
    shot: Option<Screenshot>,
) {
    use fandhe_core::screens::print_page::PrintPhase;

    if state.phase != PrintPhase::Printing {
        return;
    }
    let generation = state.generation;
    match capture_step(slot, generation, now, shot) {
        CaptureStep::Idle => {}
        CaptureStep::Waiting => sender.request_repaint_after(POLL_INTERVAL),
        CaptureStep::Done(Err(e)) => send_finish(sender, generation, Err(e)),
        CaptureStep::Done(Ok(img)) => send_finish(sender, generation, printer.print_image(&img)),
    }
}

/// 要求を受けた直後の処理。対応 OS では取り込み待ちを置いて読み戻しを依頼する。
#[cfg(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
fn on_screen_request(generation: u64, sender: &CompletionSender<PrintPageState>) {
    arm_capture(&CAPTURE, generation, Instant::now());
    sender.host_view().request_screenshot(generation);
}

/// Linux ほか: 取り込みをせず、非対応で即確定する。
#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
)))]
fn on_screen_request(generation: u64, sender: &CompletionSender<PrintPageState>) {
    send_finish(sender, generation, Err(PrintError::Unsupported));
}

/// `registry.rs` から `print_screen_page` 画面用に配線される effect ドライバ（macOS・Windows・Linux）。
/// 要求を受けたら取り込みを始め、以後のフレームで [`drive_screen`] が進める。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn print_screen_page_effect(state: &PrintPageState, sender: &CompletionSender<PrintPageState>) {
    register_teardown();
    if let Some(view) = take_request(state) {
        let _ = sender.send(Box::new(mark_printing));
        on_screen_request(view.generation, sender);
        return;
    }
    let shot = sender.host_view().tagged_screenshot(state.generation);
    drive_screen(
        state,
        sender,
        platform_print().as_ref(),
        &CAPTURE,
        Instant::now(),
        shot,
    );
}

/// `registry.rs` から配線される effect ドライバ（iOS / Android）。取り込みが確定したら OS 別の
/// `begin_print_image` へ渡し、結果は [`bridge`] のポーリングで反映する。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn print_screen_page_effect(state: &PrintPageState, sender: &CompletionSender<PrintPageState>) {
    register_teardown();
    use fandhe_core::screens::print_page::{PrintPhase, mark_printing};

    if let Some(view) = take_request(state) {
        let _ = sender.send(Box::new(mark_printing));
        on_screen_request(view.generation, sender);
        return;
    }
    if state.phase != PrintPhase::Printing {
        return;
    }
    let generation = state.generation;
    let shot = sender.host_view().tagged_screenshot(generation);
    match capture_step(&CAPTURE, generation, Instant::now(), shot) {
        // 取り込みは済んでいる。OS のダイアログの結果を待つ。
        CaptureStep::Idle => poll_bridge(state, sender),
        CaptureStep::Waiting => sender.request_repaint_after(POLL_INTERVAL),
        CaptureStep::Done(Err(e)) => {
            let outcome = to_outcome(Err(e));
            let _ = sender.send(Box::new(move |s: &mut PrintPageState| {
                fandhe_core::screens::print_page::finish_print(s, generation, outcome);
            }));
        }
        CaptureStep::Done(Ok(img)) => {
            let Some(request_id) = bridge::begin() else {
                send_busy(sender, generation);
                return;
            };
            if let Err(e) = begin_print_image(request_id, &img) {
                bridge::deliver(request_id, Err(e));
                // 入力が無いと次フレームが来ず Printing のまま止まるため、結果の反映を起こす。
                sender.request_repaint_after(Duration::ZERO);
            }
        }
    }
}

/// 終了処理と Android の再生成時の破棄を登録表（`fandhe_runtime::teardown`）へ登録する（Issue #1453）。
/// 公開の入口・effect・公開型のメソッドの先頭で呼ぶ。初めて使われたときに登録され、2 回目以降は何もしない。
pub(crate) fn register_teardown() {
    #[cfg(target_os = "android")]
    fandhe_runtime::teardown::register(
        fandhe_runtime::teardown::Teardown::new("print")
            .on_android_recreate(crate::reset_pending_tracking),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::print_page as screen;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _: Duration) {}
    }

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
            to_outcome(Err(PrintError::Unsupported)),
            Ok(CoreOutcome::Unsupported)
        );
        assert_eq!(
            to_outcome(Err(PrintError::InvalidInput("print title"))),
            Err(PrintFailure::Invalid("見出し".into()))
        );
        assert_eq!(
            to_outcome(Err(PrintError::InvalidInput("print body"))),
            Err(PrintFailure::Invalid("本文".into()))
        );
        assert_eq!(
            to_outcome(Err(PrintError::InvalidInput("other"))),
            Err(PrintFailure::Invalid("入力".into()))
        );
        assert!(matches!(
            to_outcome(Err(PrintError::Internal("print failed"))),
            Err(PrintFailure::Failed(_))
        ));
    }

    #[test]
    fn to_outcome_capture_arm_is_distinct_from_print_failure() {
        let cap = to_outcome(Err(PrintError::Internal("print capture")));
        assert_eq!(cap, Err(PrintFailure::Failed(CAPTURE_FAILED_TEXT.into())));
        assert_ne!(cap, to_outcome(Err(PrintError::Internal("print failed"))));
    }

    fn pending_slot(generation: u64, now: Instant) -> Mutex<Option<PendingCapture>> {
        let slot = Mutex::new(None);
        arm_capture(&slot, generation, now);
        slot
    }

    fn red(w: usize, h: usize) -> Screenshot {
        Screenshot {
            width: w,
            height: h,
            rgba: [255u8, 0, 0, 255].repeat(w * h),
        }
    }

    #[test]
    fn capture_step_waits_then_delivers_once() {
        let t0 = Instant::now();
        let slot = pending_slot(1, t0);
        assert!(matches!(
            capture_step(&slot, 1, t0, None),
            CaptureStep::Waiting
        ));
        let img = red(2, 3);
        let CaptureStep::Done(Ok(p)) = capture_step(&slot, 1, t0, Some(img.clone())) else {
            panic!("取り込めるはず");
        };
        assert_eq!((p.width(), p.height()), (2, 3));
        // 除去済み。2 回目は何もしない。
        assert!(matches!(
            capture_step(&slot, 1, t0, Some(img.clone())),
            CaptureStep::Idle
        ));
    }

    #[test]
    fn capture_step_expiry_fails_and_late_shot_is_ignored() {
        let t0 = Instant::now();
        let slot = pending_slot(1, t0);
        let late = t0 + CAPTURE_DEADLINE + Duration::from_millis(1);
        let img = red(1, 1);
        // 期限後に読み戻しが届いていても成功にしない。
        assert!(matches!(
            capture_step(&slot, 1, late, Some(img.clone())),
            CaptureStep::Done(Err(PrintError::Internal("print capture")))
        ));
        assert!(matches!(
            capture_step(&slot, 1, late, Some(img.clone())),
            CaptureStep::Idle
        ));
    }

    #[test]
    fn capture_step_drops_other_generation() {
        let t0 = Instant::now();
        let slot = pending_slot(1, t0);
        let img = red(1, 1);
        assert!(matches!(
            capture_step(&slot, 2, t0, Some(img.clone())),
            CaptureStep::Idle
        ));
        // 古い待ちは除去されている。
        assert!(slot.lock().unwrap().is_none());
    }

    #[test]
    fn capture_step_rejects_empty_image() {
        let t0 = Instant::now();
        let slot = pending_slot(1, t0);
        let img = red(0, 0);
        assert!(matches!(
            capture_step(&slot, 1, t0, Some(img.clone())),
            CaptureStep::Done(Err(PrintError::Internal("print capture")))
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
            Err(PrintError::Unsupported)
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
        use crate::contract::UnsupportedPrint;
        use std::sync::atomic::{AtomicU32, Ordering};

        /// 呼び出し回数を数え、決めた結果を返す偽実装。
        struct Fake {
            calls: AtomicU32,
            result: fn() -> Result<PrintOutcome, PrintError>,
        }

        impl Print for Fake {
            fn print(&self, _r: &PrintRequest) -> Result<PrintOutcome, PrintError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                (self.result)()
            }
        }

        fn fake(result: fn() -> Result<PrintOutcome, PrintError>) -> Fake {
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
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
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
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
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
            let f = fake(|| Err(PrintError::Internal("print failed")));
            let s = run(&f);
            assert!(s.status.is_empty());
            assert!(s.error.starts_with(screen::FAILED_PREFIX));
            assert!(!s.error.contains("print"));
        }

        /// `print_image` の呼び出し回数を数え、決めた結果を返す偽実装。
        struct FakeImage {
            calls: AtomicU32,
            result: fn() -> Result<PrintOutcome, PrintError>,
        }

        impl Print for FakeImage {
            fn print(&self, _r: &PrintRequest) -> Result<PrintOutcome, PrintError> {
                unreachable!("画面の印刷は print_image を使う");
            }
            fn print_image(&self, _i: &PrintImage) -> Result<PrintOutcome, PrintError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                (self.result)()
            }
        }

        fn fake_image(result: fn() -> Result<PrintOutcome, PrintError>) -> FakeImage {
            FakeImage {
                calls: AtomicU32::new(0),
                result,
            }
        }

        /// 押下・処理中への遷移・取り込み待ちの設置までを済ませた状態から、1 フレーム回す。
        /// `elapsed` は設置からの経過、`shot` は届いた読み戻し。
        fn run_screen(
            printer: &dyn Print,
            elapsed: Duration,
            shot: Option<Screenshot>,
        ) -> PrintPageState {
            let mut s = PrintPageState::default();
            screen::print(&mut s);
            mark_printing(&mut s);
            let t0 = Instant::now();
            let slot = pending_slot(s.generation, t0);
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
            drive_screen(&s, &sender, printer, &slot, t0 + elapsed, shot);
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            s
        }

        #[test]
        fn screen_submitted_calls_os_once() {
            let f = fake_image(|| Ok(PrintOutcome::Submitted));
            let s = run_screen(&f, Duration::ZERO, Some(red(2, 2)));
            assert_eq!(s.status, screen::SUBMITTED_STATUS);
            assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn screen_waiting_does_not_call_os_or_finish() {
            let f = fake_image(|| Ok(PrintOutcome::Submitted));
            let s = run_screen(&f, Duration::ZERO, None);
            assert_eq!(s.phase, screen::PrintPhase::Printing);
            assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn screen_expired_capture_never_reaches_os_or_submitted() {
            let f = fake_image(|| Ok(PrintOutcome::Submitted));
            let late = CAPTURE_DEADLINE + Duration::from_millis(1);
            let s = run_screen(&f, late, Some(red(2, 2)));
            assert_eq!(f.calls.load(Ordering::SeqCst), 0);
            assert_eq!(
                s.error,
                format!(
                    "{}{}",
                    screen::FAILED_PREFIX,
                    fandhe_core::screens::print_screen_page::CAPTURE_FAILED_TEXT
                )
            );
            assert_ne!(s.status, screen::SUBMITTED_STATUS);
        }

        #[test]
        fn screen_invalid_size_never_reaches_os() {
            let f = fake_image(|| Ok(PrintOutcome::Submitted));
            let s = run_screen(&f, Duration::ZERO, Some(red(0, 0)));
            assert_eq!(f.calls.load(Ordering::SeqCst), 0);
            assert!(s.error.starts_with(screen::FAILED_PREFIX));
        }

        #[test]
        fn screen_cancelled_is_neither_error_nor_submitted() {
            let f = fake_image(|| Ok(PrintOutcome::Cancelled));
            let s = run_screen(&f, Duration::ZERO, Some(red(1, 1)));
            assert_eq!(s.status, screen::CANCELLED_STATUS);
            assert!(s.error.is_empty());
        }

        #[test]
        fn screen_unsupported_os_shows_unsupported() {
            // 既定実装の print_image は非対応。
            let s = run_screen(&UnsupportedPrint, Duration::ZERO, Some(red(1, 1)));
            assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
            assert_ne!(s.status, screen::SUBMITTED_STATUS);
        }

        #[test]
        fn screen_stale_pending_generation_is_dropped_without_os_call() {
            let f = fake_image(|| Ok(PrintOutcome::Submitted));
            let mut s = PrintPageState::default();
            screen::print(&mut s);
            mark_printing(&mut s);
            let t0 = Instant::now();
            let slot = pending_slot(s.generation.wrapping_sub(1), t0);
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
            drive_screen(&s, &sender, &f, &slot, t0, Some(red(1, 1)));
            assert!(rx.try_recv().is_err());
            assert_eq!(f.calls.load(Ordering::SeqCst), 0);
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
        let id = bridge::SLOT.begin();
        assert!(!bridge::deliver(id + 1000, Ok(PrintOutcome::Submitted)));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(id, Ok(PrintOutcome::Cancelled)));
        assert!(!bridge::deliver(id, Ok(PrintOutcome::Submitted)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(ready, Some(Ok(PrintOutcome::Cancelled)));
        bridge::SLOT.clear(id);
    }
}
