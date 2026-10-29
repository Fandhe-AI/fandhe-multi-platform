//! Phase 11 の拡充ループ（親 #167、Issue #810）: スリープ・画面の消灯と点灯・ロックと解除の
//! 出来事を OS の通知で受け取る契約。`fandhe-egui-proxy` の `device/sleep_lock_events.rs`
//! （effect ドライバ）が [`SleepLockEvents::start`] で購読を開始し、各 OS 実装（macOS・Windows: #810、
//! Android: #811）が本トレイトを実装する。iOS は対応表で保留（`UnsupportedSleepLockEvents`）。Web は `fandhe-web-dom` の
//! `sleep_lock_events` が本トレイトを通らずに同じ結果の種類で実装する（ブラウザの Promise・イベントは
//! 同期のトレイトに載らない。`EventLog` と本モジュールの純関数は共有する）。
//!
//! # 方式: 出来事は OS の通知、今の状態は OS へ問い直す
//!
//! 出来事は OS の通知だけで受け、ロック中かどうかをアプリ内に覚えない。OS が今の状態を返せる
//! 種類（Windows のロック、Android の画面点灯・ロック）は、画面を開いた時と出来事が届くたびに
//! [`SleepLockEvents::read_current`] で OS へ問い直す。タイマーで状態を推測せず、経過時間・時刻は扱わない
//! （単調時計はスリープ中に止まる OS があり、スリープをまたぐ経過は正しく測れない）。
//! 何を知らせ、何を読めるかは OS ごとに違い、[`EventSupport`] で宣言する（画面は OS 名で分岐しない）。
//!
//! Web は出来事を `change` の後の `screenState` の読み直しで受け、同じ購読の中で直前に読んだ値との
//! 違いだけをロック・解除とする（`change` は `userState` の変化でも届き、どちらが変わったかを渡さない
//! ため）。直前の値は出来事を決めるためだけに使い、今の状態として出さない（今の状態は毎回
//! `screenState` を読む）。開始前は `screenState` が null で読めない。
//!
//! # 結果の種類（契約表 10）
//!
//! - 受信中: [`SleepLockEvents::start`] が `Ok` で、guard が生きている間
//! - 停止（利用者の操作・画面を離れた）= 正常。[`SubscriptionGuard`] の drop は失敗を生まない
//! - 開始失敗: `Err(Internal("sleep_lock_events start"))`。失敗時は OS 資源を残さない
//! - 実行中の失敗: [`Signal::Failed`]（OS の通知ループが止まった等）。成功を装わない
//! - 非対応: `Err(Unsupported("sleep_lock_events"))`
//!
//! native の各 OS は `PermissionDenied` を返さない（権限不要）。Web は `idle-detection` の拒否
//! （利用者・Permissions Policy の区別なし）を許可なしとして出し、開始失敗に畳まない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の順序（開始・停止・取り出しは UI スレッドの 1 本の effect に集約し、OS コールバックは
//! 受信順の列へ積むだけ）、嘘の成功の禁止（読めない状態を「ロックされていない」に畳まない。
//! 読み取りの失敗は `Err`、読めない OS は `None`）、資源の解放（停止・画面を離れる・開始途中の失敗の
//! 全経路で購読を解除し、解除後に届いた通知は捨てる）。
//!
//! 守らない範囲: 画面を開く前・止めた後の出来事、Android 14 以上でキャッシュ状態の間に溜められ前面復帰時に
//! まとめて届く通知の順序と畳み込み（回数が少なく出ることがある）、OS 間で「画面の消灯」「ロック」の
//! 定義が違うこと、複数プロセス・複数窓、OS 側の通知の遅れ、同一セッションの他プロセスが送る偽の
//! `WM_WTSSESSION_CHANGE` / `WM_POWERBROADCAST`（記録が増えるだけで、今の状態は OS へ問い直す）、
//! Web の Chromium 系以外（非対応表示）・iframe 埋め込み・Permissions Policy の拒否と利用者の拒否の区別
//! （どちらも許可なし）・許可ダイアログの応答待ちの期限・ページが非表示の間に届く `change` の遅れと、
//! その間にロックと解除が続いたときの取りこぼし・スクリーンセーバーをロックとして数えること。
//!
//! # 後続 OS の要点
//!
//! OS 資源は `id` で持ち、生ポインタを渡さない。停止は「登録解除 → OS の解除 → 解放」の順で、ロックの
//! 内側では待たない。経過時間・時刻・タイマーを使わない。

use std::sync::Arc;

use crate::DeviceError;

/// 受け取る出来事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepLockEvent {
    /// システムがスリープに入る。
    SystemWillSleep,
    /// システムがスリープから戻った。
    SystemDidWake,
    /// 画面が消えた。
    ScreenOff,
    /// 画面が点いた。
    ScreenOn,
    /// ロックされた。
    Locked,
    /// ロックが解除された。
    Unlocked,
}

/// 表示順の全種類。
pub const ALL_EVENTS: [SleepLockEvent; 6] = [
    SleepLockEvent::SystemWillSleep,
    SleepLockEvent::SystemDidWake,
    SleepLockEvent::ScreenOff,
    SleepLockEvent::ScreenOn,
    SleepLockEvent::Locked,
    SleepLockEvent::Unlocked,
];

impl SleepLockEvent {
    /// [`ALL_EVENTS`] 内の位置（回数の配列の添字）。
    pub fn index(self) -> usize {
        match self {
            Self::SystemWillSleep => 0,
            Self::SystemDidWake => 1,
            Self::ScreenOff => 2,
            Self::ScreenOn => 3,
            Self::Locked => 4,
            Self::Unlocked => 5,
        }
    }
}

/// OS が知らせる種類と、今の状態を読めるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSupport {
    /// 種類ごとに OS が通知を出すか（[`SleepLockEvent::index`] の順）。
    pub events: [bool; 6],
    /// 今のロック状態を読めるか。
    pub lock: bool,
    /// 今の画面の点灯状態を読めるか。
    pub screen: bool,
}

impl EventSupport {
    /// 何も取れない（非対応）。
    pub const NONE: EventSupport = EventSupport {
        events: [false; 6],
        lock: false,
        screen: false,
    };

    /// 指定した種類を OS が知らせるか。
    pub fn supports(&self, event: SleepLockEvent) -> bool {
        self.events[event.index()]
    }
}

/// OS が返したロック状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    /// ロック中。
    Locked,
    /// ロックされていない。
    Unlocked,
    /// OS が判定できないと返した。
    Unknown,
}

/// OS が返した画面の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenState {
    /// 点いている。
    On,
    /// 消えている。
    Off,
    /// OS が判定できないと返した。
    Unknown,
}

/// 今の状態。`None` は「この OS では読めない」（失敗でも「ロックされていない」でもない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentState {
    /// ロック状態。
    pub lock: Option<LockState>,
    /// 画面の状態。
    pub screen: Option<ScreenState>,
}

/// OS コールバックからドライバへ渡す合図。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// 出来事が届いた。
    Event(SleepLockEvent),
    /// OS の通知ループが止まった（実行中の失敗）。
    Failed,
}

/// OS コールバックから呼ばれる通知先。呼び出しスレッドは任意で、すぐ戻ること。
pub type OnSignal = Arc<dyn Fn(Signal) + Send + Sync>;

/// 購読の所有者。drop で OS の購読を解除する（利用者の停止は失敗を生まない）。戻った時点で
/// 以降の呼び出しはドライバの登録表で捨てられる。
pub trait SubscriptionGuard: Send {}

/// スリープ・ロックの通知の契約。
pub trait SleepLockEvents: Send + Sync {
    /// OS が知らせる種類と読める状態。
    fn support(&self) -> EventSupport;

    /// 購読を開始する。失敗時は OS 資源を残さない。
    fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError>;

    /// 今の状態を OS へ問う（UI スレッドから同期で呼ぶ）。アプリ内の記憶から作らない。
    fn read_current(&self) -> Result<CurrentState, DeviceError>;
}

/// 非対応環境の実装。成功へ倒さない。
pub struct UnsupportedSleepLockEvents;

impl SleepLockEvents for UnsupportedSleepLockEvents {
    fn support(&self) -> EventSupport {
        EventSupport::NONE
    }

    fn start(&self, _on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
        Err(DeviceError::Unsupported("sleep_lock_events"))
    }

    fn read_current(&self) -> Result<CurrentState, DeviceError> {
        Err(DeviceError::Unsupported("sleep_lock_events"))
    }
}

/// 直近に保持する出来事の件数。
pub const MAX_RECENT: usize = 8;

/// 受信記録（純関数的に畳む）。番号は受信順に 1 から振る。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventLog {
    next_seq: u64,
    recent: Vec<(u64, SleepLockEvent)>,
    counts: [u64; 6],
}

impl EventLog {
    /// 空の記録。
    pub fn new() -> Self {
        Self::default()
    }

    /// 出来事を 1 件記録する。直近 [`MAX_RECENT`] 件を超えた古いものは捨てる（回数には残す）。
    pub fn record(&mut self, event: SleepLockEvent) {
        self.next_seq += 1;
        self.counts[event.index()] += 1;
        self.recent.insert(0, (self.next_seq, event));
        self.recent.truncate(MAX_RECENT);
    }

    /// `(新しい順の直近、種類ごとの回数)`。
    pub fn snapshot(&self) -> (Vec<(u64, SleepLockEvent)>, [u64; 6]) {
        (self.recent.clone(), self.counts)
    }
}

/// `WM_WTSSESSION_CHANGE`。
pub const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
/// `WM_POWERBROADCAST`。
pub const WM_POWERBROADCAST: u32 = 0x0218;
/// `WTS_SESSION_LOCK`。
pub const WTS_SESSION_LOCK: usize = 7;
/// `WTS_SESSION_UNLOCK`。
pub const WTS_SESSION_UNLOCK: usize = 8;
/// `PBT_APMSUSPEND`。
pub const PBT_APMSUSPEND: usize = 4;
/// `PBT_APMRESUMEAUTOMATIC`。
pub const PBT_APMRESUMEAUTOMATIC: usize = 18;

/// Windows のウィンドウメッセージ → 出来事。`PBT_APMRESUMESUSPEND`（利用者の操作での復帰で
/// `PBT_APMRESUMEAUTOMATIC` に続いて届く）と未知の値は `None`（二重計上・捏造をしない）。
pub fn event_from_windows(msg: u32, wparam: usize) -> Option<SleepLockEvent> {
    match (msg, wparam) {
        (WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK) => Some(SleepLockEvent::Locked),
        (WM_WTSSESSION_CHANGE, WTS_SESSION_UNLOCK) => Some(SleepLockEvent::Unlocked),
        (WM_POWERBROADCAST, PBT_APMSUSPEND) => Some(SleepLockEvent::SystemWillSleep),
        (WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC) => Some(SleepLockEvent::SystemDidWake),
        _ => None,
    }
}

/// `WTSINFOEX_LEVEL1_W.SessionFlags` → ロック状態（0 = `WTS_SESSIONSTATE_LOCK`、
/// 1 = `WTS_SESSIONSTATE_UNLOCK`、それ以外は `Unknown`）。Windows 7 / Server 2008 R2 では
/// 値が逆になるが、対象外の版（公式の注記）。
pub fn lock_state_from_session_flags(flags: u32) -> LockState {
    match flags {
        0 => LockState::Locked,
        1 => LockState::Unlocked,
        _ => LockState::Unknown,
    }
}

/// Apple の受け口のコード → 出来事（0 = WillSleep、1 = DidWake、2 = ScreensDidSleep、
/// 3 = ScreensDidWake）。未知は `None`（捏造しない）。
pub fn event_from_apple_code(code: i32) -> Option<SleepLockEvent> {
    match code {
        0 => Some(SleepLockEvent::SystemWillSleep),
        1 => Some(SleepLockEvent::SystemDidWake),
        2 => Some(SleepLockEvent::ScreenOff),
        3 => Some(SleepLockEvent::ScreenOn),
        _ => None,
    }
}

/// Web の Idle Detection `screenState`（`locked` / `unlocked`）→ ロック状態。null・未知の値は
/// `None`（捏造しない。呼び出し側は読み取り失敗として扱う）。
pub fn lock_state_from_idle_screen_state(v: Option<&str>) -> Option<LockState> {
    match v {
        Some("locked") => Some(LockState::Locked),
        Some("unlocked") => Some(LockState::Unlocked),
        _ => None,
    }
}

/// Web の `change` の後に読んだ `screenState` から出来事を決める。`prev` は同じ購読の中で最後に
/// 読んだ値で、判定にだけ使う（今の状態として出さない）。`prev` が無い・同じ値・`Unknown` を含む
/// 遷移は `None`（`userState` だけの変化や、読めない期間をまたぐ差を出来事にしない）。
pub fn event_from_lock_transition(
    prev: Option<LockState>,
    now: LockState,
) -> Option<SleepLockEvent> {
    match (prev?, now) {
        (LockState::Unlocked, LockState::Locked) => Some(SleepLockEvent::Locked),
        (LockState::Locked, LockState::Unlocked) => Some(SleepLockEvent::Unlocked),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_screen_state_maps_known_only() {
        assert_eq!(
            lock_state_from_idle_screen_state(Some("locked")),
            Some(LockState::Locked)
        );
        assert_eq!(
            lock_state_from_idle_screen_state(Some("unlocked")),
            Some(LockState::Unlocked)
        );
        for v in [None, Some(""), Some("dim")] {
            assert_eq!(lock_state_from_idle_screen_state(v), None);
        }
    }

    #[test]
    fn lock_transition_events() {
        use LockState::*;
        let f = event_from_lock_transition;
        assert_eq!(f(Some(Unlocked), Locked), Some(SleepLockEvent::Locked));
        assert_eq!(f(Some(Locked), Unlocked), Some(SleepLockEvent::Unlocked));
        assert_eq!(f(None, Locked), None);
        assert_eq!(f(Some(Locked), Locked), None);
        assert_eq!(f(Some(Unlocked), Unlocked), None);
        assert_eq!(f(Some(Unknown), Locked), None);
        assert_eq!(f(Some(Locked), Unknown), None);
    }

    #[test]
    fn log_numbers_from_one_and_keeps_latest_eight() {
        let mut log = EventLog::new();
        for _ in 0..9 {
            log.record(SleepLockEvent::ScreenOn);
        }
        let (recent, counts) = log.snapshot();
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0].0, 9);
        assert_eq!(recent[7].0, 2);
        assert_eq!(counts[SleepLockEvent::ScreenOn.index()], 9);
    }

    #[test]
    fn log_counts_per_kind() {
        let mut log = EventLog::new();
        log.record(SleepLockEvent::Locked);
        log.record(SleepLockEvent::Unlocked);
        log.record(SleepLockEvent::Locked);
        let (recent, counts) = log.snapshot();
        assert_eq!(counts[SleepLockEvent::Locked.index()], 2);
        assert_eq!(counts[SleepLockEvent::Unlocked.index()], 1);
        assert_eq!(recent[0], (3, SleepLockEvent::Locked));
    }

    #[test]
    fn windows_messages_map() {
        assert_eq!(
            event_from_windows(WM_WTSSESSION_CHANGE, 7),
            Some(SleepLockEvent::Locked)
        );
        assert_eq!(
            event_from_windows(WM_WTSSESSION_CHANGE, 8),
            Some(SleepLockEvent::Unlocked)
        );
        assert_eq!(
            event_from_windows(WM_POWERBROADCAST, 4),
            Some(SleepLockEvent::SystemWillSleep)
        );
        assert_eq!(
            event_from_windows(WM_POWERBROADCAST, 18),
            Some(SleepLockEvent::SystemDidWake)
        );
    }

    #[test]
    fn windows_resume_suspend_and_unknown_are_dropped() {
        // PBT_APMRESUMESUSPEND = 7 は二重計上しない。
        assert_eq!(event_from_windows(WM_POWERBROADCAST, 7), None);
        // logon(5) などの他の session change。
        assert_eq!(event_from_windows(WM_WTSSESSION_CHANGE, 5), None);
        assert_eq!(event_from_windows(0x0001, 7), None);
    }

    #[test]
    fn session_flags_map() {
        assert_eq!(lock_state_from_session_flags(0), LockState::Locked);
        assert_eq!(lock_state_from_session_flags(1), LockState::Unlocked);
        assert_eq!(
            lock_state_from_session_flags(0xFFFF_FFFF),
            LockState::Unknown
        );
        assert_eq!(lock_state_from_session_flags(2), LockState::Unknown);
    }

    #[test]
    fn apple_codes_map() {
        assert_eq!(
            event_from_apple_code(0),
            Some(SleepLockEvent::SystemWillSleep)
        );
        assert_eq!(
            event_from_apple_code(1),
            Some(SleepLockEvent::SystemDidWake)
        );
        assert_eq!(event_from_apple_code(2), Some(SleepLockEvent::ScreenOff));
        assert_eq!(event_from_apple_code(3), Some(SleepLockEvent::ScreenOn));
        assert_eq!(event_from_apple_code(4), None);
        assert_eq!(event_from_apple_code(-1), None);
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedSleepLockEvents;
        assert_eq!(u.support(), EventSupport::NONE);
        assert!(matches!(
            u.start(Arc::new(|_| {})),
            Err(DeviceError::Unsupported("sleep_lock_events"))
        ));
        assert!(matches!(
            u.read_current(),
            Err(DeviceError::Unsupported("sleep_lock_events"))
        ));
    }
}
