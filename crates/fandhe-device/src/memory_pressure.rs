//! Phase 11 の拡充ループ（親 #179、Issue #911）: メモリ逼迫（と Web の CPU 負荷）の通知を OS の
//! 通知で受け取る契約。`fandhe-egui-proxy` の `device/memory_pressure.rs`（effect ドライバ）が
//! [`MemoryPressure::start`] で購読を開始し、各 OS 実装（macOS・Windows: #911、iOS: b-ios、Android:
//! b-android、Web: `fandhe-web-dom`）が本トレイトを実装する。それ以外の環境は
//! [`UnsupportedMemoryPressure`] が非対応を返す。
//!
//! # 方式: 合図は OS の通知、今の状態は OS へ問い直す
//!
//! 合図は OS の通知だけで受け、「平常」をアプリ内で仮定しない（最初の通知までは「まだ届いていない」）。
//! 今の状態を読める OS（Android・Windows）は [`MemoryPressure::read_current`] で画面を開いた時と
//! 合図のたびに OS へ問い直し、最後の合図から作らない。タイマー・ポーリングは使わない。何を知らせ、
//! 何を読めるかは OS ごとに違い、[`EventSupport`] で宣言する（画面は OS 名で分岐しない）。
//!
//! # 結果の種類（契約表 10）
//!
//! - 受信中: [`MemoryPressure::start`] が `Ok` で、guard が生きている間
//! - 停止（利用者の操作・画面を離れた）= 正常。[`SubscriptionGuard`] の drop は失敗を生まない
//! - 開始失敗: `Err(Internal("memory_pressure start"))`。失敗時は OS 資源を残さない
//! - 許可なし（Web の Permissions Policy 拒否）: `Err(PermissionDenied("memory_pressure"))`。
//!   非対応とは別の文言で出す
//! - 非対応（Web の `NotSupportedError` を含む）: `Err(Unsupported("memory_pressure"))`
//! - 実行中の失敗: [`Signal::Failed`]（Windows の待機失敗など）。成功を装わない
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の順序（開始・停止・取り出しは UI スレッドの 1 本の effect に集約し、OS
//! コールバックは受信順の列へ積むだけ）、嘘の成功の禁止（読めない状態を「逼迫していない」に畳まない。
//! 読み取りの失敗は `Err`、読めない OS は `None`）、資源の解放（停止・画面を離れる・開始途中の失敗の
//! 全経路で購読を解除し、解除後に届いた通知は捨てる）。
//!
//! 守らない範囲: 画面を開く前・止めた後の合図、背景で停止中の合図、Windows の中間帯をまたぐ変化
//! （Low → 中間 → Low 等）、Web の implicit focus を失った間の記録と間引き、OS 間の「逼迫」基準の差、
//! システム全体の値でありプロセス単位でないこと、複数プロセス・複数窓・複数タブ、OS 側の通知の遅れ。
//! 合図に応じたメモリ解放などの処理はしない。
//!
//! # 後続 OS の要点
//!
//! OS 資源は `id` で持ち、生ポインタを渡さない。停止は「登録解除 → OS の解除 → 解放」の順で、ロックの
//! 内側では待たない。iOS はメモリ警告と dispatch source を [`event_from_apple`] の符号化で返す。
//! Android は trim 値を [`PressureEvent::Trim`] にそのまま持ち（未知値も捨てない）、`lowMemory` は
//! [`MemoryLevel::Low`] / [`MemoryLevel::NotLow`] で返す。Web は `fandhe-web-dom` が同じ型を使う。

use std::sync::Arc;

use crate::DeviceError;

/// 受け取る合図の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureEvent {
    /// iOS のメモリ警告（`didReceiveMemoryWarning`）。
    MemoryWarning,
    /// iOS・macOS の段階: 平常。
    Normal,
    /// iOS・macOS の段階: 警告。
    Warning,
    /// iOS・macOS の段階: 危機的。
    Critical,
    /// Android の `onTrimMemory(level)`。値をそのまま持つ（未知値も捨てない）。
    Trim(i32),
    /// Windows の `LowMemoryResourceNotification`（メモリ不足）。
    Low,
    /// Windows の `HighMemoryResourceNotification`（メモリに余裕）。
    High,
    /// Web の CPU 負荷: nominal（メモリではない）。
    CpuNominal,
    /// Web の CPU 負荷: fair。
    CpuFair,
    /// Web の CPU 負荷: serious。
    CpuSerious,
    /// Web の CPU 負荷: critical。
    CpuCritical,
}

/// 表示順の全種類（[`PressureEvent::Trim`] は値違いでも 1 枠）。
pub const ALL_EVENTS: [PressureEvent; 11] = [
    PressureEvent::MemoryWarning,
    PressureEvent::Normal,
    PressureEvent::Warning,
    PressureEvent::Critical,
    PressureEvent::Trim(0),
    PressureEvent::Low,
    PressureEvent::High,
    PressureEvent::CpuNominal,
    PressureEvent::CpuFair,
    PressureEvent::CpuSerious,
    PressureEvent::CpuCritical,
];

impl PressureEvent {
    /// [`ALL_EVENTS`] 内の位置（回数の配列の添字）。
    pub fn index(self) -> usize {
        match self {
            Self::MemoryWarning => 0,
            Self::Normal => 1,
            Self::Warning => 2,
            Self::Critical => 3,
            Self::Trim(_) => 4,
            Self::Low => 5,
            Self::High => 6,
            Self::CpuNominal => 7,
            Self::CpuFair => 8,
            Self::CpuSerious => 9,
            Self::CpuCritical => 10,
        }
    }
}

/// OS が知らせる種類と、今の状態を読めるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSupport {
    /// 種類ごとに OS が通知を出すか（[`PressureEvent::index`] の順）。
    pub events: [bool; 11],
    /// 今の状態を [`MemoryPressure::read_current`] で読めるか。
    pub current: bool,
}

impl EventSupport {
    /// 何も取れない（非対応）。
    pub const NONE: EventSupport = EventSupport {
        events: [false; 11],
        current: false,
    };

    /// 指定した種類を OS が知らせるか。
    pub fn supports(&self, event: PressureEvent) -> bool {
        self.events[event.index()]
    }
}

/// OS が返した今のメモリの状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryLevel {
    /// 不足（Windows の Low、Android の `lowMemory` = true）。
    Low,
    /// 不足ではない（Android の `lowMemory` = false）。
    NotLow,
    /// 余裕あり（Windows の High）。
    High,
    /// 不足でも余裕でもない中間帯（Windows）。
    Middle,
}

/// OS コールバックからドライバへ渡す合図。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// 合図が届いた。
    Event(PressureEvent),
    /// OS の通知元が失われた（実行中の失敗）。
    Failed,
}

/// OS コールバックから呼ばれる通知先。呼び出しスレッドは任意で、すぐ戻ること。
pub type OnSignal = Arc<dyn Fn(Signal) + Send + Sync>;

/// 購読の所有者。drop で OS の購読を解除する（利用者の停止は失敗を生まない）。戻った時点で
/// 以降の呼び出しはドライバの登録表で捨てられる。
pub trait SubscriptionGuard: Send {}

/// メモリ逼迫・負荷の通知の契約。
pub trait MemoryPressure: Send + Sync {
    /// OS が知らせる種類と読める状態。
    fn support(&self) -> EventSupport;

    /// 購読を開始する。失敗時は OS 資源を残さない。
    fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError>;

    /// 今の状態を OS へ問う（UI スレッドから同期で呼ぶ）。`Ok(None)` はこの OS では読めない、
    /// `Err` は読み取りの失敗（「逼迫していない」に畳まない）。アプリ内の記憶から作らない。
    fn read_current(&self) -> Result<Option<MemoryLevel>, DeviceError>;
}

/// 非対応環境の実装。成功へ倒さない。
pub struct UnsupportedMemoryPressure;

impl MemoryPressure for UnsupportedMemoryPressure {
    fn support(&self) -> EventSupport {
        EventSupport::NONE
    }

    fn start(&self, _on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
        Err(DeviceError::Unsupported("memory_pressure"))
    }

    fn read_current(&self) -> Result<Option<MemoryLevel>, DeviceError> {
        Err(DeviceError::Unsupported("memory_pressure"))
    }
}

/// 直近に保持する合図の件数。
pub const MAX_RECENT: usize = 8;

/// 受信記録（純関数的に畳む）。番号は受信順に 1 から振る。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventLog {
    next_seq: u64,
    recent: Vec<(u64, PressureEvent)>,
    counts: [u64; 11],
}

impl EventLog {
    /// 空の記録。
    pub fn new() -> Self {
        Self::default()
    }

    /// 合図を 1 件記録する。直近 [`MAX_RECENT`] 件を超えた古いものは捨てる（回数には残す）。
    pub fn record(&mut self, event: PressureEvent) {
        self.next_seq += 1;
        self.counts[event.index()] += 1;
        self.recent.insert(0, (self.next_seq, event));
        self.recent.truncate(MAX_RECENT);
    }

    /// `(新しい順の直近、種類ごとの回数)`。
    pub fn snapshot(&self) -> (Vec<(u64, PressureEvent)>, [u64; 11]) {
        (self.recent.clone(), self.counts)
    }
}

/// Apple の受け口 → 合図。`kind` 0 は dispatch source の data（NORMAL=0x1 / WARN=0x2 /
/// CRITICAL=0x4 のうちちょうど 1 ビットのときだけ段階にする）、`kind` 1 はメモリ警告（iOS）。
/// 0 ビット・複数ビット・未知ビット・未知の kind は `None`（捏造しない）。macOS と iOS で共用する。
pub fn event_from_apple(kind: i32, flags: u64) -> Option<PressureEvent> {
    match (kind, flags) {
        (0, 0x1) => Some(PressureEvent::Normal),
        (0, 0x2) => Some(PressureEvent::Warning),
        (0, 0x4) => Some(PressureEvent::Critical),
        (1, _) => Some(PressureEvent::MemoryWarning),
        _ => None,
    }
}

/// Windows の Low / High の signaled 状態 → 今の状態。両方 signaled は規定外で `Err`
/// （読めない状態を成功に畳まない）。
pub fn windows_level(low: bool, high: bool) -> Result<MemoryLevel, DeviceError> {
    match (low, high) {
        (true, false) => Ok(MemoryLevel::Low),
        (false, true) => Ok(MemoryLevel::High),
        (false, false) => Ok(MemoryLevel::Middle),
        (true, true) => Err(DeviceError::Internal("memory_pressure read")),
    }
}

/// Windows の待機計画 `(Low を待つか, High を待つか)`。Low / High は条件が成り立つ間 signaled の
/// 状態オブジェクトなので、成り立っている側を待つと即時に戻り続ける。このため反対側だけを待つ。
pub fn windows_wait_plan(level: MemoryLevel) -> (bool, bool) {
    match level {
        MemoryLevel::Low => (false, true),
        MemoryLevel::High => (true, false),
        // Middle と、Windows では出ない NotLow は両側を待つ。
        MemoryLevel::Middle | MemoryLevel::NotLow => (true, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_numbers_from_one_and_keeps_latest_eight() {
        let mut log = EventLog::new();
        for _ in 0..9 {
            log.record(PressureEvent::Low);
        }
        let (recent, counts) = log.snapshot();
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0].0, 9);
        assert_eq!(recent[7].0, 2);
        assert_eq!(counts[PressureEvent::Low.index()], 9);
    }

    #[test]
    fn log_counts_trim_in_one_slot() {
        let mut log = EventLog::new();
        log.record(PressureEvent::Trim(5));
        log.record(PressureEvent::Trim(-1));
        log.record(PressureEvent::Trim(999));
        let (recent, counts) = log.snapshot();
        assert_eq!(counts[4], 3);
        assert_eq!(recent[0], (3, PressureEvent::Trim(999)));
        assert_eq!(recent[1], (2, PressureEvent::Trim(-1)));
    }

    #[test]
    fn all_events_match_index() {
        for (i, e) in ALL_EVENTS.iter().enumerate() {
            assert_eq!(e.index(), i);
        }
    }

    #[test]
    fn apple_single_bits_map() {
        assert_eq!(event_from_apple(0, 0x1), Some(PressureEvent::Normal));
        assert_eq!(event_from_apple(0, 0x2), Some(PressureEvent::Warning));
        assert_eq!(event_from_apple(0, 0x4), Some(PressureEvent::Critical));
        assert_eq!(event_from_apple(1, 0), Some(PressureEvent::MemoryWarning));
    }

    #[test]
    fn apple_invalid_is_dropped() {
        assert_eq!(event_from_apple(0, 0), None);
        assert_eq!(event_from_apple(0, 0x3), None);
        assert_eq!(event_from_apple(0, 0x7), None);
        assert_eq!(event_from_apple(0, 0x8), None);
        assert_eq!(event_from_apple(0, u64::MAX), None);
        assert_eq!(event_from_apple(2, 0x1), None);
        assert_eq!(event_from_apple(-1, 0x1), None);
    }

    #[test]
    fn windows_level_four_cases() {
        assert_eq!(windows_level(true, false).unwrap(), MemoryLevel::Low);
        assert_eq!(windows_level(false, true).unwrap(), MemoryLevel::High);
        assert_eq!(windows_level(false, false).unwrap(), MemoryLevel::Middle);
        assert!(matches!(
            windows_level(true, true),
            Err(DeviceError::Internal("memory_pressure read"))
        ));
    }

    #[test]
    fn windows_wait_plan_waits_opposite_side() {
        assert_eq!(windows_wait_plan(MemoryLevel::Low), (false, true));
        assert_eq!(windows_wait_plan(MemoryLevel::High), (true, false));
        assert_eq!(windows_wait_plan(MemoryLevel::Middle), (true, true));
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedMemoryPressure;
        assert_eq!(u.support(), EventSupport::NONE);
        assert!(!u.support().supports(PressureEvent::Low));
        assert!(matches!(
            u.start(Arc::new(|_| {})),
            Err(DeviceError::Unsupported("memory_pressure"))
        ));
        assert!(matches!(
            u.read_current(),
            Err(DeviceError::Unsupported("memory_pressure"))
        ));
    }
}
