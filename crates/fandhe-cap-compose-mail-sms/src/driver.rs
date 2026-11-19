//! Phase 11（親 #1142）型サンプル「メール・SMS の作成」（Issue #1384）の effect ドライバ。
//! `fandhe_core::screens::compose_mail_sms`（端末依存ゼロ）の要求を観測し、値を
//! `crate::contract::ComposeRequest` で検査してから OS へ渡し、結果を
//! `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、
//! `fandhe-cap-app-badge/src/driver.rs` と同型）。契約と守る範囲は `crate::contract` の
//! モジュール doc を参照。
//!
//! - iOS / Android: コールバック型のため pull 型 [`bridge`]（`RequestSlot` の facade）で毎フレーム
//!   `drive_callback` が結果を反映する。**dispatch はここで最終形にしてあり、後続 issue は OS 別
//!   モジュールだけを埋める**
//! - macOS / Windows / それ以外: [`ComposeMailSms`] を UI スレッドで同期に呼ぶ（macOS の添付付きも
//!   `NSSharingService` を同期に呼び、完了は待たない）。Windows の添付付きは modal の Simple MAPI を
//!   作業スレッドで呼び、結果を `CompletionSender` で後から届ける（#1043、`crate::windows`）。
//!   Linux は `UnsupportedComposeMailSms` で「非対応」を表示する

use crate::contract::{self, ComposeAttachment, ComposeOutcome, ComposeRequest};
use crate::error::ComposeMailSmsError;
use fandhe_core::screens::compose_mail_sms::{
    ComposeFailure, ComposeMailSmsState, ComposeRequestView, ComposeTarget, CoreOutcome,
    take_request,
};

#[cfg(not(any(target_os = "ios", target_os = "android")))]
use crate::contract::ComposeMailSms;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_core::screens::compose_mail_sms::{finish_compose, mark_composing};

use fandhe_runtime::CompletionSender;

/// `ComposeMailSmsError` を画面用の結果へ写像する。内部構造・入力値は出さず固定文言だけにする
/// （security.md A09）。
fn to_outcome(
    result: Result<ComposeOutcome, ComposeMailSmsError>,
) -> Result<CoreOutcome, ComposeFailure> {
    match result {
        Ok(ComposeOutcome::Queued) => Ok(CoreOutcome::Queued),
        Ok(ComposeOutcome::Saved) => Ok(CoreOutcome::Saved),
        Ok(ComposeOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Ok(ComposeOutcome::HandedOff) => Ok(CoreOutcome::HandedOff),
        Ok(ComposeOutcome::NoHandler) => Ok(CoreOutcome::NoHandler),
        Err(ComposeMailSmsError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(ComposeMailSmsError::InvalidInput(kind)) => Err(ComposeFailure::Invalid(
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
        Err(ComposeMailSmsError::Internal(label)) => Err(ComposeFailure::Failed(
            match label {
                l if l == contract::ATTACHMENT_FILE => {
                    "添付の一時ファイルを用意できませんでした"
                }
                l if l == contract::CLEANUP => {
                    "前回の添付の一時ファイルを消せなかったため、作成画面を開きませんでした"
                }
                l if l == contract::BUSY => "前の作成画面が開いているか、前の結果がまだ受け取られていません",
                l if l == contract::LOST => "作成画面の要求が失われました。もう一度お試しください",
                l if l == contract::MAPI => {
                    "既定のメールアプリで添付付きの作成を開けませんでした（新しい Outlook は非対応）"
                }
                _ => "OS の作成画面を開けませんでした",
            }
            .to_string(),
        )),
    }
}

/// iOS の結果コード表（SSOT。`.m` の `RESULT_*` と一致させる）。未知のコードは失敗に倒す。
#[allow(dead_code)] // b-ios が結果受け口から使う
pub(crate) fn outcome_from_ios_code(code: i32) -> Result<ComposeOutcome, ComposeMailSmsError> {
    match code {
        0 => Ok(ComposeOutcome::Queued),
        1 => Ok(ComposeOutcome::Saved),
        2 => Ok(ComposeOutcome::Cancelled),
        3 => Err(ComposeMailSmsError::Internal("compose failed")),
        4 => Ok(ComposeOutcome::NoHandler),
        5 => Err(ComposeMailSmsError::Internal("compose present")),
        _ => Err(ComposeMailSmsError::Internal("compose outcome")),
    }
}

/// Android の結果コード表（SSOT。Java 側 `ComposeHelper.RESULT_*` と一致させる）。
#[allow(dead_code)] // b-android が結果受け口から使う
pub(crate) fn outcome_from_android_code(code: i32) -> Result<ComposeOutcome, ComposeMailSmsError> {
    match code {
        0 => Ok(ComposeOutcome::HandedOff),
        1 => Ok(ComposeOutcome::NoHandler),
        2 => Err(ComposeMailSmsError::Internal("compose background")),
        3 => Err(ComposeMailSmsError::Internal("compose present")),
        _ => Err(ComposeMailSmsError::Internal("compose outcome")),
    }
}

/// 画面の値から検査済みの要求を作る。OS へ渡す唯一の入口（検査の SSOT は `ComposeRequest`）。
fn build_request(view: &ComposeRequestView) -> Result<ComposeRequest, ComposeMailSmsError> {
    match view.target {
        ComposeTarget::Mail => {
            // macOS の `NSSharingService` は To しか渡せないため、添付付きは CC / BCC を付けない
            // （画面にもその旨を出す。黙って落とすのではなく要求の側で外す）。
            let none: &[String] = &[];
            let (cc, bcc) = if cfg!(target_os = "macos") && view.attachment.is_some() {
                (none, none)
            } else {
                (&view.cc[..], &view.bcc[..])
            };
            let req = ComposeRequest::mail(&view.to, cc, bcc, &view.subject, &view.body)?;
            match &view.attachment {
                Some(a) => req.with_attachment(ComposeAttachment::new(
                    &a.file_name,
                    &a.mime_type,
                    &a.data,
                )?),
                None => Ok(req),
            }
        }
        ComposeTarget::Sms if view.to.len() > 1 => {
            Err(ComposeMailSmsError::InvalidInput("compose to"))
        }
        ComposeTarget::Sms => {
            ComposeRequest::sms(view.to.first().map_or("", String::as_str), &view.body)
        }
    }
}

/// OS 側コールバックとの橋渡し（iOS / Android、pull 型、`RequestSlot` の facade）。
/// 他機能とは共有しない本機能専用の [`SLOT`](bridge::SLOT) を、画面 effect（`SCREEN_OWNER`）と公開入口
/// （[`begin_compose_mail_sms`]、`PUBLIC_OWNER`）が持ち主の印で分けて共有する（`consumer-app-structure.md` §8.2）。
/// 結果は OS の生の値（`ComposeOutcome` / `ComposeMailSmsError`）で置き、画面用への写像（`to_outcome`）は画面が取り出すときに行う。
/// ホストのテストでも slot の扱いを確かめるため、`test` でもコンパイルする（OS 呼び出しは含まない）。
#[cfg(any(target_os = "ios", target_os = "android", test))]
#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
pub(crate) mod bridge {
    use crate::contract::{ComposeOutcome, LOST};
    use crate::error::ComposeMailSmsError;
    use fandhe_runtime::request_slot::{RequestSlot, SlotOwner};

    pub(crate) type Outcome = Result<ComposeOutcome, ComposeMailSmsError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();
    /// 画面 effect が追う自分の分の要求。
    pub(crate) static SCREEN_OWNER: SlotOwner = SlotOwner::new();
    /// 公開入口 [`super::begin_compose_mail_sms`] が追う自分の分の要求。
    pub(crate) static PUBLIC_OWNER: SlotOwner = SlotOwner::new();
    /// 公開側の poll・discard 本体（持ち主の照合・取得・解放）を直列にする。並行 poll で結果が二重に返らないため。
    static PUBLIC_POLL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 画面側の開始。進行中の要求（作成画面が開いたまま）と、公開側の未取得の結果は置き換えない。
    /// 自分の未取得の結果は置き換える。
    pub(crate) fn begin_screen() -> Option<u64> {
        SCREEN_OWNER.begin_excluding(&SLOT, &[&PUBLIC_OWNER])
    }

    /// 公開入口の開始。進行中の要求と、自分の未取得の結果があれば `None`（busy）。画面側の届いた未取得の結果は
    /// 捨てずに画面の受け取り待ちへ移す（`SlotOwner::begin_parking`）。画面は `poll_and_finish_owned` でそのまま受け取り、
    /// 受け取られない分は画面の次の開始か [`reset`] で解放する（残るのは最大 1 件）。持ち主のいない届いた結果は置き換えて捨てる。
    pub(crate) fn begin_public() -> Option<u64> {
        PUBLIC_OWNER.begin_parking(&SLOT, &SCREEN_OWNER)
    }

    /// OS からの結果を置く。`request_id` 不一致・未登録・既に結果ありは捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(request_id: u64, result: Outcome) -> bool {
        SLOT.deliver(request_id, result)
    }

    /// [`super::poll_compose_mail_sms`] の本体。持ち主が `request_id` のときだけ見る。届いていれば 1 度だけ `Some` で
    /// 持ち主を外す。slot の要求が失われた（reset 等で空・別の要求に替わった）ときは `Err(Internal(LOST))` を 1 度だけ返す。
    pub(crate) fn poll_public(request_id: u64) -> Option<Outcome> {
        let _guard = PUBLIC_POLL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if request_id == 0 || PUBLIC_OWNER.current() != request_id {
            return None;
        }
        if let Some(result) = SLOT.take_ready(request_id) {
            PUBLIC_OWNER.release(request_id);
            return Some(result);
        }
        if SLOT.snapshot().is_some_and(|(id, _)| id == request_id) {
            return None;
        }
        PUBLIC_OWNER.release(request_id);
        Some(Err(ComposeMailSmsError::Internal(LOST)))
    }

    /// [`super::discard_compose_mail_sms`] の本体。持ち主が `request_id` のときだけ持ち主を外して `true`。
    /// 結果が届いていれば slot も空ける。進行中なら slot は残す: 作成画面は閉じられないため、OS の結果が届くまで
    /// 次の開始（画面・公開とも）を busy で断る（device-bridge-contracts §9「完了の通知が来るまで未解放の記録を消さない」）。
    /// 閉じたあとの届いた結果は持ち主がいないため、次の開始が置き換える。
    pub(crate) fn discard_public(request_id: u64) -> bool {
        let _guard = PUBLIC_POLL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if request_id == 0 || PUBLIC_OWNER.current() != request_id {
            return false;
        }
        let in_flight = SLOT
            .snapshot()
            .is_some_and(|(id, ready)| id == request_id && ready.is_none());
        if !in_flight {
            SLOT.clear(request_id);
        }
        PUBLIC_OWNER.release(request_id);
        true
    }

    /// Android: Activity 再生成直後に `android_main` から呼ぶ。保留中の要求は復元せず破棄し、
    /// 以後届く旧 `request_id` の [`deliver`] を無視させる（`app_badge::bridge::reset` と同じ）。受け取り待ちの結果も捨てる。
    /// 持ち主の印は残し、画面は次の poll で失敗に、公開側は次の [`poll_public`] で失われた要求を 1 度だけ通知する。
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset();
    }

    /// 公開入口の依頼自体が失敗したときの後片付け（結果は届かないので slot と印を空ける）。
    pub(crate) fn abort_public(request_id: u64) {
        SLOT.clear(request_id);
        PUBLIC_OWNER.release(request_id);
    }
}

/// gallery の `android_main`（Activity 再生成）から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
}

/// OS へ作成画面の表示を依頼する。`Ok` は受理済みで結果は後から [`bridge::deliver`] で届く。
/// `Err` は依頼自体の失敗（結果は届かない。画面経路はその場で `deliver` し、公開入口は `bridge::abort_public` で空けて `Err` を返す）。
#[cfg(target_os = "ios")]
fn begin_compose(request_id: u64, req: &ComposeRequest) -> Result<(), ComposeMailSmsError> {
    crate::ios::begin_compose(request_id, req)
}

#[cfg(target_os = "android")]
fn begin_compose(request_id: u64, req: &ComposeRequest) -> Result<(), ComposeMailSmsError> {
    crate::android::begin_compose(request_id, req)
}

/// iOS / Android 版の effect ドライバ本体。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn drive_callback(state: &ComposeMailSmsState, sender: &CompletionSender<ComposeMailSmsState>) {
    use fandhe_core::screens::compose_mail_sms::{ComposePhase, finish_compose, mark_composing};

    if let Some(view) = take_request(state) {
        let _ = sender.send(Box::new(mark_composing));
        let generation = view.generation;
        // 進行中の要求（作成画面が開いたまま）と公開入口の未取得の結果は置き換えず、画面には処理中の失敗を返す。
        let Some(request_id) = bridge::begin_screen() else {
            let outcome = to_outcome(Err(ComposeMailSmsError::Internal(contract::BUSY)));
            let _ = sender.send(Box::new(move |s: &mut ComposeMailSmsState| {
                finish_compose(s, generation, outcome);
            }));
            return;
        };
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
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &bridge::SCREEN_OWNER,
        sender,
        |_request_id| {},
        move |s: &mut ComposeMailSmsState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != ComposePhase::Idle {
                finish_compose(s, generation, to_outcome(outcome));
                true
            } else {
                false
            }
        },
        // 自分の要求が失われた（Activity 再生成の reset 等）。待ち続けず失敗で終える。
        move |s: &mut ComposeMailSmsState| {
            finish_compose(
                s,
                generation,
                to_outcome(Err(ComposeMailSmsError::Internal(contract::LOST))),
            );
        },
    );
}

/// 公開入口（iOS / Android）: 検査済みの要求で OS の作成画面を開き、結果を受け取るための要求 id を返す。
/// 画面の状態型に依存せず、フレームワークの外のアプリも使える（`consumer-app-structure.md` §8.2）。結果は
/// [`poll_compose_mail_sms`] で 1 度だけ取り出す。使わない要求は [`discard_compose_mail_sms`] で捨てる。
///
/// - 進行中の要求（画面経由を含む。作成画面が開いたまま）と、自分の結果が未取得のうちは `Internal(BUSY)` で断る
/// - 画面側の届いた未取得の結果は捨てずに画面の受け取り待ちへ移して始める（画面はそのまま受け取る）
/// - 依頼自体の失敗は `Err`（結果は届かない）。macOS・Windows は同期の `platform_compose_mail_sms` を使い、
///   この入口は iOS / Android 以外では `Unsupported`
///
/// iOS の天井: busy は Rust の slot が決め、空くのは `.m` の delegate の完了通知だけ。delegate が呼ばれないまま
/// 作成画面が閉じられる（ホストアプリが提示の積み重ねごと閉じる等）と、プロセス終了まで画面・公開とも busy のまま残る
/// （iOS には reset が無く、discard も進行中の slot は残す）。`.m` は `modalInPresentation` でスワイプの閉じを塞いでいる。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn begin_compose_mail_sms(req: &ComposeRequest) -> Result<u64, ComposeMailSmsError> {
    let request_id = bridge::begin_public().ok_or(ComposeMailSmsError::Internal(contract::BUSY))?;
    if let Err(e) = begin_compose(request_id, req) {
        bridge::abort_public(request_id);
        return Err(e);
    }
    Ok(request_id)
}

/// iOS / Android 以外では成功へ倒さず `Unsupported`（macOS・Windows は同期の [`platform_compose_mail_sms`]）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn begin_compose_mail_sms(req: &ComposeRequest) -> Result<u64, ComposeMailSmsError> {
    let _ = req;
    Err(ComposeMailSmsError::Unsupported("compose"))
}

/// [`begin_compose_mail_sms`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・公開入口でない id・
/// 取り出し済み・0・iOS / Android 以外は `None`。要求が失われた（Android の Activity 再生成等）ときは
/// `Err(Internal(LOST))` を 1 度だけ返す。並行して同じ id を poll しても、結果が二重に返ることはない。
pub fn poll_compose_mail_sms(
    request_id: u64,
) -> Option<Result<ComposeOutcome, ComposeMailSmsError>> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        bridge::poll_public(request_id)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = request_id;
        None
    }
}

/// [`begin_compose_mail_sms`] の自分の要求を捨てる。`request_id` が公開入口の現在の要求のときだけ `true`
/// （他人の id・取り出し済み・0・iOS / Android 以外は `false`）。以後 [`poll_compose_mail_sms`] は `None`。
/// 作成画面は閉じない。開いたままなら、OS の結果が届くまで次の開始（画面・公開とも）は busy になる。
pub fn discard_compose_mail_sms(request_id: u64) -> bool {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        bridge::discard_public(request_id)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = request_id;
        false
    }
}

/// `registry.rs` から `compose_mail_sms` 画面用に配線される effect ドライバ（iOS / Android）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn compose_mail_sms_effect(
    state: &ComposeMailSmsState,
    sender: &CompletionSender<ComposeMailSmsState>,
) {
    drive_callback(state, sender);
}

/// 同期経路（macOS・Windows・Linux）: 検査してから [`ComposeMailSms`] を呼ぶ。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn drive(
    state: &ComposeMailSmsState,
    sender: &CompletionSender<ComposeMailSmsState>,
    composer: &dyn ComposeMailSms,
) {
    let Some(view) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_composing));
    let generation = view.generation;
    let req = build_request(&view);
    // Windows の添付付きは Simple MAPI の modal を作業スレッドで開き、結果は後から届ける
    // （UI スレッドを止めない。在席フラグと一時ファイルは `crate::windows` が持つ）。
    #[cfg(target_os = "windows")]
    let req = match req {
        Ok(r) if r.attachment().is_some() => {
            let s = sender.clone();
            let started =
                crate::windows::begin_attachment(r, move |res| finish(&s, generation, res));
            if let Err(e) = started {
                finish(sender, generation, Err(e));
            }
            return;
        }
        other => other,
    };
    finish(sender, generation, req.and_then(|r| composer.compose(&r)));
}

/// 結果を画面用に写像して、世代つきで完了キューへ送る（`finish_compose` が古い世代を捨てる）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn finish(
    sender: &CompletionSender<ComposeMailSmsState>,
    generation: u64,
    result: Result<ComposeOutcome, ComposeMailSmsError>,
) {
    let outcome = to_outcome(result);
    let _ = sender.send(Box::new(move |s: &mut ComposeMailSmsState| {
        finish_compose(s, generation, outcome);
    }));
}

/// OS 別の [`ComposeMailSms`] 実装を返す（macOS / Windows / それ以外）。画面の状態型に依存しないため、フレームワークの外のアプリも使える。
/// iOS / Android は結果をコールバックで受ける別の型（`bridge`）で、同期の [`ComposeMailSms`] を持たないため載せない。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn platform_compose_mail_sms() -> Box<dyn ComposeMailSms> {
    #[cfg(target_os = "macos")]
    return crate::macos::implementation();
    #[cfg(target_os = "windows")]
    return crate::windows::implementation();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(crate::contract::UnsupportedComposeMailSms);
}

/// `registry.rs` から配線される effect ドライバ（macOS・Windows・Linux）。
///
/// ponytail: 作成画面を開くのは OS への URL 1 回の軽い呼び出しなので UI スレッドで同期に呼ぶ。
/// 戻らない呼び出しが見つかったら、作業スレッド + bridge へ移す。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn compose_mail_sms_effect(
    state: &ComposeMailSmsState,
    sender: &CompletionSender<ComposeMailSmsState>,
) {
    if take_request(state).is_none() {
        return;
    }
    drive(state, sender, platform_compose_mail_sms().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    #[allow(dead_code)]
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }
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
            to_outcome(Err(ComposeMailSmsError::Unsupported("compose"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert!(matches!(
            to_outcome(Err(ComposeMailSmsError::InvalidInput("compose to"))),
            Err(ComposeFailure::Invalid(m)) if m == "宛先"
        ));
        assert!(matches!(
            to_outcome(Err(ComposeMailSmsError::Internal("compose present"))),
            Err(ComposeFailure::Failed(_))
        ));
    }

    /// iOS / Android 以外の公開入口は成功へ倒さない。
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    #[test]
    fn public_entry_is_unsupported_off_ios_android() {
        let req = ComposeRequest::sms("", "x").unwrap();
        assert_eq!(
            begin_compose_mail_sms(&req),
            Err(ComposeMailSmsError::Unsupported("compose"))
        );
        assert_eq!(poll_compose_mail_sms(1), None);
        assert!(!discard_compose_mail_sms(1));
    }

    #[test]
    fn to_outcome_maps_attachment_labels_to_fixed_text() {
        for l in [
            contract::ATTACHMENT_FILE,
            contract::CLEANUP,
            contract::BUSY,
            contract::LOST,
            contract::MAPI,
        ] {
            let Err(ComposeFailure::Failed(m)) = to_outcome(Err(ComposeMailSmsError::Internal(l)))
            else {
                panic!("{l}");
            };
            assert!(!m.contains("compose") && !m.is_empty());
            assert_ne!(m, "OS の作成画面を開けませんでした");
        }
        assert_eq!(
            to_outcome(Err(ComposeMailSmsError::Unsupported("compose cc"))),
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
            Err(ComposeMailSmsError::InvalidInput("compose to"))
        ));
        let bad_cc = ComposeRequestView {
            to: vec!["a@b.c".into()],
            cc: vec!["x@y.z&bcc=q@r.s".into()],
            ..bad
        };
        let err = build_request(&bad_cc).unwrap_err();
        assert!(matches!(
            err,
            ComposeMailSmsError::InvalidInput("compose cc")
        ));
        assert_eq!(
            to_outcome(Err(err)),
            Err(ComposeFailure::Invalid("CC".into()))
        );
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    mod sync_path {
        use super::*;
        use crate::contract::UnsupportedComposeMailSms;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        /// 呼び出し回数を数え、決めた結果を返す偽実装。
        struct Fake {
            calls: AtomicU32,
            result: fn() -> Result<ComposeOutcome, ComposeMailSmsError>,
        }

        impl ComposeMailSms for Fake {
            fn compose(&self, _r: &ComposeRequest) -> Result<ComposeOutcome, ComposeMailSmsError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                (self.result)()
            }
        }

        fn fake(result: fn() -> Result<ComposeOutcome, ComposeMailSmsError>) -> Fake {
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
            let sender = CompletionSender::new(tx, Arc::new(NoopWaker));
            drive(&s, &sender, composer);
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            s
        }

        /// 実際の `to_url` を通す（URL 経路の OS 実装と同じ。添付は `Unsupported` になる）。
        /// 使うのは Windows で走らせない添付のテストだけ。
        #[cfg(not(target_os = "windows"))]
        struct UrlOnly;

        #[cfg(not(target_os = "windows"))]
        impl ComposeMailSms for UrlOnly {
            fn compose(&self, r: &ComposeRequest) -> Result<ComposeOutcome, ComposeMailSmsError> {
                r.to_url().map(|_| ComposeOutcome::HandedOff)
            }
        }

        // Windows の添付付きは `drive` が実 MAPI（`begin_attachment`）へ直行し composer を使わないため、
        // 実ダイアログを開かないようこのテストは Windows で走らせない（MAPI 経路は `crate::windows` のテストが持つ）。
        #[cfg(not(target_os = "windows"))]
        #[test]
        fn attachment_over_url_path_shows_unsupported() {
            let mut s = ComposeMailSmsState::default();
            screen::compose_mail_with_attachment(&mut s);
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, Arc::new(NoopWaker));
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
            let sender = CompletionSender::new(tx, Arc::new(NoopWaker));
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
            let f = fake(|| Err(ComposeMailSmsError::Internal("compose present")));
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

    /// bridge（iOS / Android の slot）の公開入口と画面の共有。slot は static なので直列に走らせる。
    mod bridge_slot {
        use super::super::bridge;
        use crate::contract::{ComposeOutcome, LOST};
        use crate::error::ComposeMailSmsError;
        use std::sync::Mutex;

        static SERIAL: Mutex<()> = Mutex::new(());

        fn clear() {
            bridge::SLOT.reset();
            bridge::SCREEN_OWNER.release(bridge::SCREEN_OWNER.current());
            bridge::PUBLIC_OWNER.release(bridge::PUBLIC_OWNER.current());
        }

        fn poll_screen() -> u8 {
            let (tx, rx) = std::sync::mpsc::channel();
            let sender =
                fandhe_runtime::CompletionSender::new(tx, std::sync::Arc::new(super::NoopWaker));
            fandhe_runtime::request_slot::poll_and_finish_owned(
                &bridge::SLOT,
                &bridge::SCREEN_OWNER,
                &sender,
                |_| {},
                |s: &mut u8, o| {
                    *s = if o == Ok(ComposeOutcome::Saved) { 1 } else { 2 };
                    true
                },
                |s: &mut u8| *s = 9,
            );
            let mut state = 0u8;
            while let Ok(c) = rx.try_recv() {
                c(&mut state);
            }
            state
        }

        #[test]
        fn public_begin_parks_unpolled_screen_result_and_screen_receives_it() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            clear();
            let screen = bridge::begin_screen().unwrap();
            assert!(bridge::deliver(screen, Ok(ComposeOutcome::Saved)));
            let public = bridge::begin_public().expect("画面の未取得の結果で busy にならない");
            assert_eq!(poll_screen(), 1, "画面は移された結果をそのまま受け取る");
            assert!(bridge::SLOT.snapshot_of(screen).is_none());
            assert_eq!(bridge::poll_public(public), None);
            assert!(bridge::deliver(public, Ok(ComposeOutcome::Cancelled)));
            assert_eq!(
                bridge::poll_public(public),
                Some(Ok(ComposeOutcome::Cancelled))
            );
            assert_eq!(bridge::poll_public(public), None, "結果は 1 度だけ");
            clear();
        }

        #[test]
        fn in_flight_is_busy_for_both_and_public_result_is_kept_from_screen() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            clear();
            let screen = bridge::begin_screen().unwrap();
            assert!(bridge::begin_public().is_none());
            assert!(bridge::begin_screen().is_none());
            assert!(bridge::deliver(screen, Ok(ComposeOutcome::Saved)));
            assert_eq!(poll_screen(), 1);
            let public = bridge::begin_public().unwrap();
            assert!(bridge::begin_screen().is_none());
            assert!(bridge::deliver(public, Ok(ComposeOutcome::Queued)));
            assert!(
                bridge::begin_screen().is_none(),
                "公開側の未取得の結果は画面から守る"
            );
            assert!(bridge::begin_public().is_none(), "自分の未取得の結果も守る");
            assert_eq!(
                bridge::poll_public(public),
                Some(Ok(ComposeOutcome::Queued))
            );
            assert!(bridge::begin_screen().is_some());
            clear();
        }

        #[test]
        fn parked_result_is_released_by_next_screen_begin_and_reset() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            clear();
            let screen = bridge::begin_screen().unwrap();
            assert!(bridge::deliver(screen, Ok(ComposeOutcome::Saved)));
            let public = bridge::begin_public().unwrap();
            assert!(bridge::deliver(public, Ok(ComposeOutcome::Queued)));
            assert!(bridge::poll_public(public).is_some());
            assert!(bridge::SLOT.snapshot_of(screen).is_some());
            bridge::begin_screen().unwrap();
            assert!(
                bridge::SLOT.snapshot_of(screen).is_none(),
                "画面の次の開始で解放"
            );
            clear();
            let screen = bridge::begin_screen().unwrap();
            assert!(bridge::deliver(screen, Ok(ComposeOutcome::Saved)));
            bridge::begin_public().unwrap();
            bridge::reset();
            assert!(bridge::SLOT.snapshot_of(screen).is_none(), "reset で解放");
            clear();
        }

        #[test]
        fn discard_in_flight_keeps_busy_until_os_result_and_late_result_is_dropped() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            clear();
            let public = bridge::begin_public().unwrap();
            assert!(!bridge::discard_public(public + 1));
            assert!(bridge::discard_public(public));
            assert!(!bridge::discard_public(public), "2 回目は何もしない");
            assert_eq!(bridge::poll_public(public), None);
            // 作成画面が開いたまま: OS の結果が届くまで次の開始は busy。
            assert!(bridge::begin_public().is_none());
            assert!(bridge::begin_screen().is_none());
            // 閉じて届いた結果は持ち主がいないので、次の開始が置き換える。
            assert!(bridge::deliver(public, Ok(ComposeOutcome::Saved)));
            let next = bridge::begin_public().unwrap();
            assert_ne!(next, public);
            assert_eq!(bridge::poll_public(next), None);
            clear();
        }

        #[test]
        fn lost_public_request_is_reported_once() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            clear();
            let public = bridge::begin_public().unwrap();
            bridge::reset();
            assert_eq!(
                bridge::poll_public(public),
                Some(Err(ComposeMailSmsError::Internal(LOST)))
            );
            assert_eq!(bridge::poll_public(public), None);
            clear();
        }

        #[test]
        fn stale_request_id_is_dropped_and_second_deliver_ignored() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            clear();
            let id = bridge::begin_screen().unwrap();
            assert!(!bridge::deliver(id + 1000, Ok(ComposeOutcome::Queued)));
            assert!(bridge::deliver(id, Ok(ComposeOutcome::Cancelled)));
            assert!(!bridge::deliver(id, Ok(ComposeOutcome::Queued)));
            assert_eq!(
                bridge::SLOT.snapshot_of(id),
                Some(Some(Ok(ComposeOutcome::Cancelled)))
            );
            clear();
        }

        #[test]
        fn reset_fails_screen_request_once() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            clear();
            bridge::begin_screen().unwrap();
            bridge::reset();
            assert_eq!(poll_screen(), 9, "失われた画面の要求は失敗で終える");
            assert_eq!(poll_screen(), 0, "通知は 1 度だけ");
            clear();
        }

        #[test]
        fn abort_public_frees_slot_and_discard_of_ready_result_clears_it() {
            let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            clear();
            let public = bridge::begin_public().unwrap();
            bridge::abort_public(public);
            assert_eq!(bridge::poll_public(public), None);
            assert!(bridge::SLOT.snapshot().is_none());
            let public = bridge::begin_public().unwrap();
            assert!(bridge::deliver(public, Ok(ComposeOutcome::Saved)));
            assert!(bridge::discard_public(public));
            assert!(
                bridge::SLOT.snapshot().is_none(),
                "届いた結果は discard で捨てる"
            );
            assert!(bridge::begin_screen().is_some());
            clear();
        }
    }
}
