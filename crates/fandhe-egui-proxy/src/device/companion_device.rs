//! Phase 11 型サンプル「付属機器のペアリング」（親 #143、Issue #740）の effect ドライバ。
//! `fandhe_core::screens::companion_device_show`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::companion_device::CompanionDevicePicker` に機器選択画面の表示を依頼して、
//! OS 層から届く結果を `CompletionSender` 経由で画面へ返す（`crate::screens::registry` の
//! `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//! 契約（結果の三値・期限・守る範囲）の SSOT は `fandhe_device::companion_device`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # OS 別の分担（dispatch・bridge・本ファイルは #740 で最終形。後続の子はここを触らない）
//!
//! - Android: `android_companion_device`（#741。`CompanionDeviceManager`、API 33 以上）
//! - iOS: 保留（`AccessorySetupKit` を宣言すると `bluetooth-le` と両立しない）。OS 別モジュールを
//!   置かず fallback に含める。保留を解くときは dispatch の変更を別 issue で扱う
//! - macOS・Windows・Linux: 対応表で `no` のため `UnsupportedCompanionDevicePicker` 固定
//!   （ダミーの成功実装へは倒さない）。Web は対象外
//!
//! # OS 層が呼ぶ受け口 2 つ
//!
//! - [`try_begin_present`]: 選択画面を出す直前に呼ぶ。`false` なら出さない（期限切れ・古い要求・
//!   中断済み）。`true` を返した要求だけが成功値（選んだ・キャンセル）を返せる
//! - [`complete_from_os`]: 結果を 1 回だけ送る唯一の受け口。古い要求 id・重複は捨てられる。
//!   関連付けの解除は呼ぶ前に済ませる（契約は `fandhe_device::companion_device`）
//!
//! # 期限（契約表 3）
//!
//! 選択画面が出るまでの [`PRESENT_DEADLINE`] だけを持つ。起点は要求を観測した時の `Instant`
//! （単調時計）。期限切れの確定（`expire_if_due` と `try_begin_present` 内）は表示開始の記録と
//! 同じロックで行うため、期限切れを確定した後に OS が選択画面を出すことはない。表示開始後は
//! 期限を持たない。
//!
//! # スレッド
//!
//! drive は UI スレッド（毎フレーム）、`try_begin_present` と `complete_from_os` は OS 層の
//! スレッド（Android は JNI 経由の任意スレッド）から呼ばれる。判定・`RequestSlot::deliver`・
//! `active` の更新は同じロックの内側で行う。
//!
//! # 秘密値（契約表 8）
//!
//! 機器名と OS のエラー文はログに出さない。失敗は理由の種別だけを持つ。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::companion_device_show::CompanionDeviceShowState;
use fandhe_core::screens::companion_device_show::{
    finish_pick, mark_running, take_request, CoreFailure, CoreOutcome, Phase,
};
use fandhe_device::companion_device::{
    CompanionDeviceFailure, CompanionDeviceOutcome, CompanionDevicePicker, PRESENT_DEADLINE,
};

use super::request_slot::{poll_and_finish, RequestSlot, POLL_INTERVAL};
use crate::app::CompletionSender;

/// OS 層から届く結果。
type Outcome = Result<CompanionDeviceOutcome, CompanionDeviceFailure>;

/// 現在のプラットフォームに応じた `CompanionDevicePicker` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn CompanionDevicePicker> {
    #[cfg(target_os = "android")]
    {
        super::android_companion_device::implementation()
    }
    // iOS は保留（モジュール doc 参照）のため、ここに含める。
    #[cfg(not(target_os = "android"))]
    {
        Arc::new(fandhe_device::companion_device::UnsupportedCompanionDevicePicker)
    }
}

/// 進行中の要求。
struct Active {
    request_id: u64,
    /// 要求を観測した時刻（単調時計）。期限の起点。
    started: Instant,
    /// OS が選択画面を出し始めたことを `try_begin_present` で記録済みか。
    presenting: bool,
}

/// 要求の追跡と期限・表示開始の排他。テストが並列で干渉しないよう static から切り出して
/// 引数で渡す（`poll_and_finish` が `'static` を要るためスロットは leak）。
struct Bridge {
    slot: &'static RequestSlot<Outcome>,
    active: Mutex<Option<Active>>,
    deadline: Duration,
}

impl Bridge {
    fn new(deadline: Duration) -> Self {
        Self {
            slot: Box::leak(Box::new(RequestSlot::new())),
            active: Mutex::new(None),
            deadline,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Active>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 発送済み（OS 呼び出し中、または結果が画面へ未反映）の要求があるか。
    fn has_pending(&self) -> bool {
        self.lock().is_some() || self.slot.snapshot().is_some()
    }

    /// 新しい要求を登録して id を返す。OS へ渡す前に呼び、即時の完了も照合できるようにする。
    fn begin(&self) -> u64 {
        let mut active = self.lock();
        let request_id = self.slot.begin();
        *active = Some(Active {
            request_id,
            started: Instant::now(),
            presenting: false,
        });
        request_id
    }

    /// 表示開始の記録。現在の要求で、未確定で、期限前のときだけ `true`。
    /// 期限を過ぎていれば `TimedOut` を確定して `false`。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))] // 呼び出し元は Android 実装と単体テスト。
    fn try_begin_present(&self, request_id: u64) -> bool {
        let mut active = self.lock();
        let Some(a) = active.as_mut().filter(|a| a.request_id == request_id) else {
            return false;
        };
        if a.presenting {
            return false;
        }
        if a.started.elapsed() >= self.deadline {
            self.slot
                .deliver(request_id, Err(CompanionDeviceFailure::TimedOut));
            *active = None;
            return false;
        }
        a.presenting = true;
        true
    }

    /// OS 層からの唯一の結果の受け口。
    fn complete_from_os(&self, request_id: u64, result: Outcome) {
        let mut active = self.lock();
        let Some(a) = active.as_ref().filter(|a| a.request_id == request_id) else {
            return;
        };
        // 表示開始を記録していない要求の成功値は嘘の成功になるため成功にしない。
        let result = match result {
            Ok(_) if !a.presenting => Err(CompanionDeviceFailure::Internal),
            r => r,
        };
        self.slot.deliver(request_id, result);
        *active = None;
    }

    /// 表示開始前に期限を過ぎていれば `TimedOut` を確定する。
    fn expire_if_due(&self) {
        let mut active = self.lock();
        let due = active
            .as_ref()
            .is_some_and(|a| !a.presenting && a.started.elapsed() >= self.deadline);
        if due {
            if let Some(a) = active.take() {
                self.slot
                    .deliver(a.request_id, Err(CompanionDeviceFailure::TimedOut));
            }
        }
    }

    fn reset(&self) {
        let mut active = self.lock();
        *active = None;
        self.slot.reset();
    }
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

fn bridge() -> &'static Bridge {
    BRIDGE.get_or_init(|| Bridge::new(PRESENT_DEADLINE))
}

/// OS 層が選択画面を出す直前に呼ぶ。`false` なら出さない（モジュール doc 参照）。
// Android 実装が呼ぶ。Android 以外のビルドでは未使用。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn try_begin_present(request_id: u64) -> bool {
    bridge().try_begin_present(request_id)
}

/// OS 層が結果を 1 回だけ送る受け口（モジュール doc 参照）。
// Android 実装が呼ぶ。Android 以外のビルドでは未使用。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn complete_from_os(request_id: u64, result: Outcome) {
    bridge().complete_from_os(request_id, result);
}

/// Activity 再生成時（`android_main` の再実行）に、保留中の要求を破棄する。
/// 画面状態は `default()`（世代 0）に戻るため、保留が残ると次の要求が発送されず飲み込まれる。
/// 旧要求の遅延完了は id 不一致で捨てられる（関連付けの解除は OS 層の責務）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(b) = BRIDGE.get() {
        b.reset();
    }
}

/// OS 層の失敗を画面の失敗種別へ写す。機器名・OS の文言は渡さない。
fn to_core_failure(f: CompanionDeviceFailure) -> CoreFailure {
    match f {
        CompanionDeviceFailure::Unsupported => CoreFailure::Unsupported,
        CompanionDeviceFailure::LocationOff => CoreFailure::LocationOff,
        CompanionDeviceFailure::BluetoothOff => CoreFailure::BluetoothOff,
        CompanionDeviceFailure::NotFound => CoreFailure::NotFound,
        CompanionDeviceFailure::NotForeground => CoreFailure::NotForeground,
        CompanionDeviceFailure::TimedOut => CoreFailure::TimedOut,
        CompanionDeviceFailure::Interrupted => CoreFailure::Interrupted,
        CompanionDeviceFailure::Internal => CoreFailure::Internal,
    }
}

/// 成功側の結果を画面の型へ写す。名前は正規化済みの文字列だけを渡す。
fn to_core_outcome(o: CompanionDeviceOutcome) -> CoreOutcome {
    match o {
        CompanionDeviceOutcome::Selected(name) => {
            CoreOutcome::Selected(name.map(|n| n.as_str().to_owned()))
        }
        CompanionDeviceOutcome::Cancelled => CoreOutcome::Cancelled,
    }
}

/// 要求を登録して OS へ渡す。同期失敗はその場で確定する。
fn start_request(bridge: &Bridge, imp: &dyn CompanionDevicePicker) {
    let request_id = bridge.begin();
    if let Err(f) = imp.pick(request_id) {
        bridge.complete_from_os(request_id, Err(f));
    }
}

/// 1 フレームぶんの処理。要求の発送・期限判定・完了の画面反映を行う。
fn drive(
    state: &CompanionDeviceShowState,
    sender: &CompletionSender<CompanionDeviceShowState>,
    imp: &Arc<dyn CompanionDevicePicker>,
    bridge: &Bridge,
) {
    if take_request(state).is_some() {
        // 保留ありは発送済み（`mark_running` の適用待ち）。二重に発送しない。
        if bridge.has_pending() {
            return;
        }
        let _ = sender.send(Box::new(mark_running));
        start_request(bridge, &**imp);
        sender.request_repaint_after(POLL_INTERVAL);
        return;
    }
    if state.phase != Phase::Running {
        return;
    }
    let generation = state.generation;
    bridge.expire_if_due();
    poll_and_finish(
        bridge.slot,
        sender,
        |_| {},
        move |s: &mut CompanionDeviceShowState, outcome| {
            let result = outcome.map(to_core_outcome).map_err(to_core_failure);
            finish_pick(s, generation, result)
        },
    );
}

/// `registry.rs` の `effect` 区画から `companion_device_show` 画面用に配線される effect ドライバ。
pub fn companion_device_show_effect(
    state: &CompanionDeviceShowState,
    sender: &CompletionSender<CompanionDeviceShowState>,
) {
    static IMPL: OnceLock<Arc<dyn CompanionDevicePicker>> = OnceLock::new();
    drive(state, sender, IMPL.get_or_init(platform), bridge());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::companion_device_show::{error_text, request_pick, status_text};
    use fandhe_device::companion_device::{DeviceName, UnsupportedCompanionDevicePicker};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<CompanionDeviceShowState>>;

    fn channel() -> (CompletionSender<CompanionDeviceShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn new_bridge(deadline: Duration) -> &'static Bridge {
        Box::leak(Box::new(Bridge::new(deadline)))
    }

    #[derive(Clone, Copy)]
    enum Mode {
        /// 同期で失敗を返す。
        Fail(CompanionDeviceFailure),
        /// 何も起きない（OS が応答しない）。
        Silent,
        /// 表示を始め、名前つきで選ぶ。
        PresentThenSelect(Option<&'static str>),
        /// 表示を始め、`ms` 待ってからキャンセルを送る。
        PresentThenCancel(u64),
        /// 表示を始めずに選択を送る（契約違反の OS）。
        SelectWithoutPresent,
        /// 表示を始めずにキャンセルを送る（契約違反の OS）。
        CancelWithoutPresent,
        /// 表示を始めずに中断を送る。
        Interrupt,
    }

    struct Fake {
        mode: Mode,
        calls: AtomicUsize,
        bridge: &'static Bridge,
    }

    impl CompanionDevicePicker for Fake {
        fn pick(&self, request_id: u64) -> Result<(), CompanionDeviceFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let b = self.bridge;
            match self.mode {
                Mode::Fail(f) => return Err(f),
                Mode::Silent => {}
                Mode::PresentThenSelect(name) => {
                    std::thread::spawn(move || {
                        if b.try_begin_present(request_id) {
                            let n = name.and_then(DeviceName::from_os);
                            b.complete_from_os(request_id, Ok(CompanionDeviceOutcome::Selected(n)));
                        }
                    });
                }
                Mode::PresentThenCancel(ms) => {
                    std::thread::spawn(move || {
                        if b.try_begin_present(request_id) {
                            std::thread::sleep(Duration::from_millis(ms));
                            b.complete_from_os(request_id, Ok(CompanionDeviceOutcome::Cancelled));
                        }
                    });
                }
                Mode::SelectWithoutPresent => {
                    b.complete_from_os(
                        request_id,
                        Ok(CompanionDeviceOutcome::Selected(DeviceName::from_os("x"))),
                    );
                }
                Mode::CancelWithoutPresent => {
                    b.complete_from_os(request_id, Ok(CompanionDeviceOutcome::Cancelled));
                }
                Mode::Interrupt => {
                    b.complete_from_os(request_id, Err(CompanionDeviceFailure::Interrupted));
                }
            }
            Ok(())
        }
    }

    fn fake(mode: Mode, bridge: &'static Bridge) -> Arc<Fake> {
        Arc::new(Fake {
            mode,
            calls: AtomicUsize::new(0),
            bridge,
        })
    }

    /// フレームを回して届いた完了を適用する。`Idle` に戻るか 5 秒で打ち切る。
    fn pump(
        state: &mut CompanionDeviceShowState,
        sender: &CompletionSender<CompanionDeviceShowState>,
        rx: &Rx,
        imp: &Arc<dyn CompanionDevicePicker>,
        bridge: &Bridge,
    ) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            drive(state, sender, imp, bridge);
            while let Ok(c) = rx.try_recv() {
                c(state);
            }
            if state.phase == Phase::Idle || Instant::now() > end {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn run(mode: Mode, deadline: Duration) -> (CompanionDeviceShowState, Arc<Fake>) {
        let bridge = new_bridge(deadline);
        let f = fake(mode, bridge);
        let imp: Arc<dyn CompanionDevicePicker> = f.clone();
        let (sender, rx) = channel();
        let mut state = CompanionDeviceShowState::default();
        request_pick(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        (state, f)
    }

    #[test]
    fn no_request_does_nothing() {
        let bridge = new_bridge(Duration::from_secs(1));
        let imp: Arc<dyn CompanionDevicePicker> = fake(Mode::Silent, bridge);
        let (sender, rx) = channel();
        drive(&CompanionDeviceShowState::default(), &sender, &imp, bridge);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn present_then_select_is_success_with_and_without_name() {
        let (s, f) = run(
            Mode::PresentThenSelect(Some("Band")),
            Duration::from_secs(30),
        );
        assert_eq!(
            s.result,
            Some(Ok(CoreOutcome::Selected(Some("Band".to_string()))))
        );
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        let (s, _) = run(Mode::PresentThenSelect(None), Duration::from_secs(30));
        assert_eq!(s.result, Some(Ok(CoreOutcome::Selected(None))));
    }

    #[test]
    fn present_then_cancel_is_not_a_failure() {
        let (s, _) = run(Mode::PresentThenCancel(0), Duration::from_secs(30));
        assert_eq!(s.result, Some(Ok(CoreOutcome::Cancelled)));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn unsupported_impl_is_unsupported_not_success() {
        let bridge = new_bridge(Duration::from_secs(30));
        let imp: Arc<dyn CompanionDevicePicker> = Arc::new(UnsupportedCompanionDevicePicker);
        let (sender, rx) = channel();
        let mut state = CompanionDeviceShowState::default();
        request_pick(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        assert_eq!(state.result, Some(Err(CoreFailure::Unsupported)));
    }

    #[test]
    fn fallback_platform_is_unsupported_off_android() {
        #[cfg(not(target_os = "android"))]
        assert_eq!(platform().pick(1), Err(CompanionDeviceFailure::Unsupported));
    }

    #[test]
    fn sync_failures_are_reported() {
        for (f, expect) in [
            (
                CompanionDeviceFailure::LocationOff,
                CoreFailure::LocationOff,
            ),
            (
                CompanionDeviceFailure::BluetoothOff,
                CoreFailure::BluetoothOff,
            ),
            (
                CompanionDeviceFailure::NotForeground,
                CoreFailure::NotForeground,
            ),
            (CompanionDeviceFailure::NotFound, CoreFailure::NotFound),
            (CompanionDeviceFailure::Internal, CoreFailure::Internal),
        ] {
            let (s, _) = run(Mode::Fail(f), Duration::from_secs(30));
            assert_eq!(s.result, Some(Err(expect)));
        }
    }

    #[test]
    fn interrupted_is_a_failure() {
        let (s, _) = run(Mode::Interrupt, Duration::from_secs(30));
        assert_eq!(s.result, Some(Err(CoreFailure::Interrupted)));
    }

    #[test]
    fn deadline_before_present_times_out_and_blocks_later_present() {
        let bridge = new_bridge(Duration::from_millis(20));
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn CompanionDevicePicker> = f.clone();
        let (sender, rx) = channel();
        let mut state = CompanionDeviceShowState::default();
        request_pick(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        assert_eq!(state.result, Some(Err(CoreFailure::TimedOut)));
        // 期限切れを確定した後は、遅れて来た OS に選択画面を出させない。
        assert!(!bridge.try_begin_present(1));
    }

    #[test]
    fn present_attempt_after_deadline_times_out_atomically() {
        let bridge = new_bridge(Duration::from_millis(10));
        let id = bridge.begin();
        std::thread::sleep(Duration::from_millis(30));
        assert!(!bridge.try_begin_present(id));
        assert_eq!(
            bridge.slot.snapshot(),
            Some((id, Some(Err(CompanionDeviceFailure::TimedOut))))
        );
        // 期限後に届いた成功は捨てられる（active は除去済み）。
        bridge.complete_from_os(id, Ok(CompanionDeviceOutcome::Cancelled));
        assert_eq!(
            bridge.slot.snapshot(),
            Some((id, Some(Err(CompanionDeviceFailure::TimedOut))))
        );
    }

    #[test]
    fn after_present_deadline_no_longer_applies() {
        // 期限 30ms。表示開始後に 120ms かけて閉じても TimedOut にならない。
        let (s, _) = run(Mode::PresentThenCancel(120), Duration::from_millis(30));
        assert_eq!(s.result, Some(Ok(CoreOutcome::Cancelled)));
    }

    #[test]
    fn success_values_without_present_are_internal() {
        for mode in [Mode::SelectWithoutPresent, Mode::CancelWithoutPresent] {
            let (s, _) = run(mode, Duration::from_secs(30));
            assert_eq!(s.result, Some(Err(CoreFailure::Internal)));
        }
    }

    #[test]
    fn stale_request_id_is_dropped_and_duplicates_first_wins() {
        let bridge = new_bridge(Duration::from_secs(30));
        let id = bridge.begin();
        assert!(bridge.try_begin_present(id));
        bridge.complete_from_os(id + 1, Err(CompanionDeviceFailure::NotFound));
        assert_eq!(bridge.slot.snapshot(), Some((id, None)));
        bridge.complete_from_os(id, Ok(CompanionDeviceOutcome::Cancelled));
        bridge.complete_from_os(id, Err(CompanionDeviceFailure::Internal));
        assert_eq!(
            bridge.slot.snapshot(),
            Some((id, Some(Ok(CompanionDeviceOutcome::Cancelled))))
        );
        // 表示開始は 1 回だけ。
        let id2 = {
            bridge.reset();
            bridge.begin()
        };
        assert!(bridge.try_begin_present(id2));
        assert!(!bridge.try_begin_present(id2));
    }

    #[test]
    fn stale_generation_completion_is_ignored() {
        let mut state = CompanionDeviceShowState::default();
        request_pick(&mut state);
        mark_running(&mut state);
        assert!(!finish_pick(&mut state, 99, Ok(CoreOutcome::Cancelled)));
        assert_eq!(state.phase, Phase::Running);
    }

    #[test]
    fn double_drive_before_mark_applied_picks_once() {
        let bridge = new_bridge(Duration::from_secs(30));
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn CompanionDevicePicker> = f.clone();
        let (sender, rx) = channel();
        let mut state = CompanionDeviceShowState::default();
        request_pick(&mut state);
        drive(&state, &sender, &imp, bridge);
        drive(&state, &sender, &imp, bridge);
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 1, "mark_running は 1 件のみ");
    }

    #[test]
    fn reset_drops_old_completion_and_allows_next_dispatch() {
        let bridge = new_bridge(Duration::from_secs(30));
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn CompanionDevicePicker> = f.clone();
        let (sender, rx) = channel();
        let mut state = CompanionDeviceShowState::default();
        request_pick(&mut state);
        drive(&state, &sender, &imp, bridge);
        let old_id = bridge.slot.snapshot().unwrap().0;
        // Activity 再生成: 画面は default に戻り、記録を捨てる。
        bridge.reset();
        bridge.complete_from_os(old_id, Ok(CompanionDeviceOutcome::Cancelled));
        assert_eq!(bridge.slot.snapshot(), None);
        let mut fresh = CompanionDeviceShowState::default();
        request_pick(&mut fresh);
        while rx.try_recv().is_ok() {}
        drive(&fresh, &sender, &imp, bridge);
        assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn failure_texts_never_contain_device_name() {
        for f in [
            CompanionDeviceFailure::NotFound,
            CompanionDeviceFailure::Internal,
            CompanionDeviceFailure::LocationOff,
        ] {
            let (s, _) = run(Mode::Fail(f), Duration::from_secs(30));
            let t = format!("{}{}", error_text(&s), status_text(&s));
            assert!(!t.contains("Band"), "{t}");
        }
    }
}
