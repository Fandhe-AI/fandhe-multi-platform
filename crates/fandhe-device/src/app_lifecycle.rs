//! Phase 11 の拡充ループ（親 #178、Issue #893）: アプリの前面・背面・凍結の出来事を OS の通知で
//! 受け取る契約。`fandhe-egui-proxy` の `device/app_lifecycle.rs`（effect ドライバ）が
//! [`AppLifecycle::start`] で購読を開始し、各 OS 実装（macOS・Windows: #893、iOS: b-ios、Android:
//! b-android、Web: `fandhe-web-dom`）が本トレイトを実装する。それ以外の環境は
//! [`UnsupportedAppLifecycle`] が非対応を返す。
//!
//! # 方式: 出来事は OS の通知、今の状態は OS へ問い直す
//!
//! 出来事は OS の通知だけで受け、前面・見えているかをアプリ内に覚えない。画面を開いた時と出来事が
//! 届くたびに [`AppLifecycle::read_current`] で OS へ問い直す。タイマー・遅延で状態を推測せず、
//! 届かなかった出来事を補わない。何を知らせ、何を読めるかは OS ごとに違い、[`EventSupport`] で
//! 宣言する（画面は OS 名で分岐しない）。凍結・bfcache 復帰は Web の一部だけが知らせる。
//!
//! # 結果の種類（契約表 10）
//!
//! - 受信中: [`AppLifecycle::start`] が `Ok` で、guard が生きている間
//! - 停止（利用者の操作・画面を離れた）= 正常。[`SubscriptionGuard`] の drop は失敗を生まない
//! - 開始失敗: `Err(Internal("app_lifecycle start"))`。失敗時は OS 資源を残さない
//! - 実行中の失敗: [`Signal::Failed`]（OS の通知元が失われた等）。成功を装わない
//! - 非対応: `Err(Unsupported("app_lifecycle"))`
//!
//! `PermissionDenied` は返さない（どの OS も権限不要）。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の順序（開始・停止・取り出しは UI スレッドの 1 本の effect に集約し、OS
//! コールバックは受信順の列へ積むだけ）、嘘の成功の禁止（読めない状態を前面・見えているに畳まない。
//! 読み取りの失敗は `Err`、読めない OS は `None`）、資源の解放（停止・画面を離れる・開始途中の失敗の
//! 全経路で購読を解除し、解除後に届いた通知は捨てる）。
//!
//! 守らない範囲: 画面を開く前・止めた後の出来事、OS 間の「操作の対象」「見える」の定義の差、
//! Android の停止中の Activity 再生成、複数プロセス・複数窓・複数 scene、OS 側の通知の遅れ、
//! 同一セッションの他プロセスが送る偽の `WM_ACTIVATEAPP` / `WM_SIZE`（記録が増えるだけで、
//! 今の状態は OS へ問い直す）。範囲外の出来事: 終了の通知、iOS の suspended、Android のキャッシュ凍結、
//! macOS の App Nap、Web の discard、システムのスリープ・ロック（`sleep_lock_events` の担当）。
//!
//! # 後続 OS の要点
//!
//! OS 資源は `id` で持ち、生ポインタを渡さない。停止は「登録解除 → OS の解除 → 解放」の順で、ロックの
//! 内側では待たない。Windows の `WM_SIZE` の種別 1 つだけ状態を持つ（[`event_from_windows`]）。
//! Apple 系の出来事コードと状態の符号化は [`event_from_apple_code`] / [`current_from_apple_bits`]
//! を使う（iOS・Android も同じ符号化で返せる）。

use std::sync::Arc;

use crate::DeviceError;

/// 受け取る出来事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    /// 操作の対象になった（前面・アクティブ）。
    Activated,
    /// 操作の対象から外れた。
    Deactivated,
    /// 見えなくなった（背面・最小化・完全に隠れた）。
    Hidden,
    /// 見えるようになった。
    Shown,
    /// 凍結された（Web の Page Lifecycle `freeze`）。
    Frozen,
    /// 凍結から戻った（`resume`）。
    Unfrozen,
    /// ページのキャッシュ（bfcache）から戻った。
    RestoredFromCache,
}

/// 表示順の全種類。
pub const ALL_EVENTS: [LifecycleEvent; 7] = [
    LifecycleEvent::Activated,
    LifecycleEvent::Deactivated,
    LifecycleEvent::Hidden,
    LifecycleEvent::Shown,
    LifecycleEvent::Frozen,
    LifecycleEvent::Unfrozen,
    LifecycleEvent::RestoredFromCache,
];

impl LifecycleEvent {
    /// [`ALL_EVENTS`] 内の位置（回数の配列の添字）。
    pub fn index(self) -> usize {
        match self {
            Self::Activated => 0,
            Self::Deactivated => 1,
            Self::Hidden => 2,
            Self::Shown => 3,
            Self::Frozen => 4,
            Self::Unfrozen => 5,
            Self::RestoredFromCache => 6,
        }
    }
}

/// OS が知らせる種類と、今の状態を読めるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSupport {
    /// 種類ごとに OS が通知を出すか（[`LifecycleEvent::index`] の順）。
    pub events: [bool; 7],
    /// 今の「操作の対象か」を読めるか。
    pub active: bool,
    /// 今の「見えているか」を読めるか。
    pub visible: bool,
}

impl EventSupport {
    /// 何も取れない（非対応）。
    pub const NONE: EventSupport = EventSupport {
        events: [false; 7],
        active: false,
        visible: false,
    };

    /// 指定した種類を OS が知らせるか。
    pub fn supports(&self, event: LifecycleEvent) -> bool {
        self.events[event.index()]
    }
}

/// OS が返した「操作の対象か」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveState {
    /// 操作の対象。
    Active,
    /// 操作の対象ではない。
    Inactive,
    /// OS が判定できないと返した。
    Unknown,
}

/// OS が返した「見えているか」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityState {
    /// 見えている。
    Visible,
    /// 見えていない。
    Hidden,
    /// OS が判定できないと返した。
    Unknown,
}

/// 今の状態。`None` は「この OS では読めない」（失敗でも「前面」でもない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentState {
    /// 操作の対象か。
    pub active: Option<ActiveState>,
    /// 見えているか。
    pub visible: Option<VisibilityState>,
}

/// OS コールバックからドライバへ渡す合図。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// 出来事が届いた。
    Event(LifecycleEvent),
    /// OS の通知元が失われた（実行中の失敗）。
    Failed,
}

/// OS コールバックから呼ばれる通知先。呼び出しスレッドは任意で、すぐ戻ること。
pub type OnSignal = Arc<dyn Fn(Signal) + Send + Sync>;

/// 購読の所有者。drop で OS の購読を解除する（利用者の停止は失敗を生まない）。戻った時点で
/// 以降の呼び出しはドライバの登録表で捨てられる。
pub trait SubscriptionGuard: Send {}

/// アプリの前面・背面・凍結の通知の契約。
pub trait AppLifecycle: Send + Sync {
    /// OS が知らせる種類と読める状態。
    fn support(&self) -> EventSupport;

    /// 購読を開始する。失敗時は OS 資源を残さない。
    fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError>;

    /// 今の状態を OS へ問う（UI スレッドから同期で呼ぶ）。アプリ内の記憶から作らない。
    fn read_current(&self) -> Result<CurrentState, DeviceError>;
}

/// 非対応環境の実装。成功へ倒さない。
pub struct UnsupportedAppLifecycle;

impl AppLifecycle for UnsupportedAppLifecycle {
    fn support(&self) -> EventSupport {
        EventSupport::NONE
    }

    fn start(&self, _on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
        Err(DeviceError::Unsupported("app_lifecycle"))
    }

    fn read_current(&self) -> Result<CurrentState, DeviceError> {
        Err(DeviceError::Unsupported("app_lifecycle"))
    }
}

/// 直近に保持する出来事の件数。
pub const MAX_RECENT: usize = 8;

/// 受信記録（純関数的に畳む）。番号は受信順に 1 から振る。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventLog {
    next_seq: u64,
    recent: Vec<(u64, LifecycleEvent)>,
    counts: [u64; 7],
}

impl EventLog {
    /// 空の記録。
    pub fn new() -> Self {
        Self::default()
    }

    /// 出来事を 1 件記録する。直近 [`MAX_RECENT`] 件を超えた古いものは捨てる（回数には残す）。
    pub fn record(&mut self, event: LifecycleEvent) {
        self.next_seq += 1;
        self.counts[event.index()] += 1;
        self.recent.insert(0, (self.next_seq, event));
        self.recent.truncate(MAX_RECENT);
    }

    /// `(新しい順の直近、種類ごとの回数)`。
    pub fn snapshot(&self) -> (Vec<(u64, LifecycleEvent)>, [u64; 7]) {
        (self.recent.clone(), self.counts)
    }
}

/// Apple の受け口のコード → 出来事（0 = 操作の対象になった、1 = 操作の対象から外れた、
/// 2 = 見えなくなった、3 = 見えるようになった）。未知は `None`（捏造しない）。macOS と iOS で共用する。
pub fn event_from_apple_code(code: i32) -> Option<LifecycleEvent> {
    match code {
        0 => Some(LifecycleEvent::Activated),
        1 => Some(LifecycleEvent::Deactivated),
        2 => Some(LifecycleEvent::Hidden),
        3 => Some(LifecycleEvent::Shown),
        _ => None,
    }
}

/// 状態の符号化 → 今の状態。bit 0-1 が操作の対象か（0 = いいえ / 1 = はい / 2 = 不明）、
/// bit 2-3 が見えているか（同じ符号化）。負の値・3・上位ビットなど規定外は `Err`
/// （読めない状態を前面・見えているに畳まない）。
pub fn current_from_apple_bits(bits: i32) -> Result<CurrentState, DeviceError> {
    let fail = DeviceError::Internal("app_lifecycle read");
    if bits < 0 || bits >> 4 != 0 {
        return Err(fail);
    }
    let active = match bits & 0b11 {
        0 => ActiveState::Inactive,
        1 => ActiveState::Active,
        2 => ActiveState::Unknown,
        _ => return Err(fail),
    };
    let visible = match (bits >> 2) & 0b11 {
        0 => VisibilityState::Hidden,
        1 => VisibilityState::Visible,
        2 => VisibilityState::Unknown,
        _ => return Err(fail),
    };
    Ok(CurrentState {
        active: Some(active),
        visible: Some(visible),
    })
}

/// `WM_ACTIVATEAPP`。
pub const WM_ACTIVATEAPP: u32 = 0x001C;
/// `WM_SIZE`。
pub const WM_SIZE: u32 = 0x0005;
/// `SIZE_RESTORED`。
pub const SIZE_RESTORED: usize = 0;
/// `SIZE_MINIMIZED`。
pub const SIZE_MINIMIZED: usize = 1;
/// `SIZE_MAXIMIZED`。
pub const SIZE_MAXIMIZED: usize = 2;

/// Windows のウィンドウメッセージ → `(出来事, 更新後の「直前が最小化か」)`。
/// 最小化からの復帰（`SIZE_RESTORED` / `SIZE_MAXIMIZED`）だけを `Shown` にし、通常のリサイズは
/// 出来事にしない。`SIZE_MAXSHOW` / `SIZE_MAXHIDE` など他の値と未知のメッセージは出来事なしで、
/// `was_minimized` を変えない。
pub fn event_from_windows(
    msg: u32,
    wparam: usize,
    was_minimized: bool,
) -> (Option<LifecycleEvent>, bool) {
    match (msg, wparam) {
        (WM_ACTIVATEAPP, 0) => (Some(LifecycleEvent::Deactivated), was_minimized),
        (WM_ACTIVATEAPP, _) => (Some(LifecycleEvent::Activated), was_minimized),
        (WM_SIZE, SIZE_MINIMIZED) => (Some(LifecycleEvent::Hidden), true),
        (WM_SIZE, SIZE_RESTORED | SIZE_MAXIMIZED) => {
            ((was_minimized).then_some(LifecycleEvent::Shown), false)
        }
        _ => (None, was_minimized),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_numbers_from_one_and_keeps_latest_eight() {
        let mut log = EventLog::new();
        for _ in 0..9 {
            log.record(LifecycleEvent::Shown);
        }
        let (recent, counts) = log.snapshot();
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0].0, 9);
        assert_eq!(recent[7].0, 2);
        assert_eq!(counts[LifecycleEvent::Shown.index()], 9);
    }

    #[test]
    fn log_counts_per_kind() {
        let mut log = EventLog::new();
        log.record(LifecycleEvent::Activated);
        log.record(LifecycleEvent::RestoredFromCache);
        log.record(LifecycleEvent::Activated);
        let (recent, counts) = log.snapshot();
        assert_eq!(counts[0], 2);
        assert_eq!(counts[6], 1);
        assert_eq!(recent[0], (3, LifecycleEvent::Activated));
    }

    #[test]
    fn all_events_match_index() {
        for (i, e) in ALL_EVENTS.iter().enumerate() {
            assert_eq!(e.index(), i);
        }
    }

    #[test]
    fn apple_codes_map_and_unknown_is_dropped() {
        assert_eq!(event_from_apple_code(0), Some(LifecycleEvent::Activated));
        assert_eq!(event_from_apple_code(1), Some(LifecycleEvent::Deactivated));
        assert_eq!(event_from_apple_code(2), Some(LifecycleEvent::Hidden));
        assert_eq!(event_from_apple_code(3), Some(LifecycleEvent::Shown));
        assert_eq!(event_from_apple_code(4), None);
        assert_eq!(event_from_apple_code(-1), None);
    }

    #[test]
    fn apple_bits_map_each_combination() {
        let c = current_from_apple_bits(1 | (1 << 2)).unwrap();
        assert_eq!(c.active, Some(ActiveState::Active));
        assert_eq!(c.visible, Some(VisibilityState::Visible));
        let c = current_from_apple_bits(0).unwrap();
        assert_eq!(c.active, Some(ActiveState::Inactive));
        assert_eq!(c.visible, Some(VisibilityState::Hidden));
        let c = current_from_apple_bits(2 | (2 << 2)).unwrap();
        assert_eq!(c.active, Some(ActiveState::Unknown));
        assert_eq!(c.visible, Some(VisibilityState::Unknown));
    }

    #[test]
    fn apple_bits_reject_negative_and_out_of_range() {
        assert!(current_from_apple_bits(-1).is_err());
        assert!(current_from_apple_bits(-2).is_err());
        assert!(current_from_apple_bits(3).is_err());
        assert!(current_from_apple_bits(3 << 2).is_err());
        assert!(current_from_apple_bits(1 << 4).is_err());
        assert!(current_from_apple_bits(i32::MAX).is_err());
    }

    #[test]
    fn windows_activate_maps() {
        assert_eq!(
            event_from_windows(WM_ACTIVATEAPP, 1, false),
            (Some(LifecycleEvent::Activated), false)
        );
        assert_eq!(
            event_from_windows(WM_ACTIVATEAPP, 0, true),
            (Some(LifecycleEvent::Deactivated), true)
        );
    }

    #[test]
    fn windows_size_transitions() {
        assert_eq!(
            event_from_windows(WM_SIZE, SIZE_MINIMIZED, false),
            (Some(LifecycleEvent::Hidden), true)
        );
        assert_eq!(
            event_from_windows(WM_SIZE, SIZE_RESTORED, true),
            (Some(LifecycleEvent::Shown), false)
        );
        assert_eq!(
            event_from_windows(WM_SIZE, SIZE_MAXIMIZED, true),
            (Some(LifecycleEvent::Shown), false)
        );
        // 最小化でなかったウィンドウのリサイズは出来事にしない。
        assert_eq!(
            event_from_windows(WM_SIZE, SIZE_RESTORED, false),
            (None, false)
        );
        assert_eq!(
            event_from_windows(WM_SIZE, SIZE_MAXIMIZED, false),
            (None, false)
        );
    }

    #[test]
    fn windows_other_values_keep_state() {
        // SIZE_MAXSHOW = 3 / SIZE_MAXHIDE = 4。
        assert_eq!(event_from_windows(WM_SIZE, 3, true), (None, true));
        assert_eq!(event_from_windows(WM_SIZE, 4, false), (None, false));
        assert_eq!(event_from_windows(0x0001, 1, true), (None, true));
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedAppLifecycle;
        assert_eq!(u.support(), EventSupport::NONE);
        assert!(!u.support().supports(LifecycleEvent::Activated));
        assert!(matches!(
            u.start(Arc::new(|_| {})),
            Err(DeviceError::Unsupported("app_lifecycle"))
        ));
        assert!(matches!(
            u.read_current(),
            Err(DeviceError::Unsupported("app_lifecycle"))
        ));
    }
}
