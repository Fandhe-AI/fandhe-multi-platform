//! UWB による相手との距離（と方向の可否）の契約（Issue #653、親 #129、対応表 `uwb-ranging`）。
//! 同じアプリを入れた近くの端末と測距用の識別子（discovery token）を交換し、相手までの距離を
//! 出し続けるための橋渡し。`fandhe-egui-proxy` の `device/uwb_ranging.rs` が `start` / `stop` と
//! 間隔つきの [`UwbRanging::read`] を行う（pull 型。OS 側が最新値を持ち、画面側が読む。
//! `activity_recognition` と同型）。実体は iOS の `NearbyInteraction` + `MultipeerConnectivity`（後続 #654）。
//!
//! 距離は表示のためだけの値で、認可・施錠・本人確認などの判断には使わない。
//!
//! # 契約
//!
//! - 結果は [`UwbPoll`] か [`UwbFailure`]。`DeviceError` へ variant を足すと既存の網羅 match が壊れるため
//!   機能固有の enum にする（`compass::CompassFailure` と同じ判断）
//! - [`UwbPoll::NoMeasurement`] は「測距は始まったが距離が無い（圏外・遮蔽・最初の値待ち）」。
//!   0 m や直前の値で代用しない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - 距離は [`validate_distance_m`] を通った値だけを表示する。検証は丸め（[`to_centimeters`]）の前に行う
//! - 能力（[`UwbCapabilities`]）は許可なしで読める。判定できない項目は [`Support::Unknown`] とし、
//!   [`Support::Yes`] へ畳まない
//! - 相手から受けた token は非信頼の入力。実装は [`is_valid_token_len`] で長さを検査してから、
//!   secure coding かつ `NIDiscoveryToken` 1 クラスだけを許す解凍をする。解凍できない token は捨てて
//!   [`UwbPoll::Exchanging`] のまま待つ
//!
//! # 結果の分類（契約表 10）
//!
//! | 場面 | 分類 |
//! | --- | --- |
//! | 利用者の「停止」 | 待機へ戻す。キャンセルで、失敗ではない |
//! | 相手の終了（[`UwbPoll::PeerEnded`]）・相手なし（[`EndReason::NoPeer`]） | 正常終了 |
//! | 背景遷移（[`UwbPoll::Interrupted`]） | 中断。実装が資源を解放済みで、自動では再開しない |
//! | 非対応・非搭載・権限拒否・時間切れ・その他 | それぞれ別の失敗 |
//!
//! 停止のタイミングごとの扱い:
//!
//! | 停止の時点 | 扱い |
//! | --- | --- |
//! | 探索中 | MPC の advertise / browse を止めて待機へ。失敗にしない |
//! | token 交換中 | MPC セッションを閉じて待機へ。失敗にしない |
//! | 測距中 | `NISession` を無効化し MPC を閉じて待機へ。最後の距離は残さない |
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序（画面の `session` と実装内の世代で照合）、圏外を 0 m や
//! 古い値で見せない、停止・失敗・中断・時間切れの全経路で MPC と `NISession` を解放する、
//! 相手の token を検証せずに使わない、嘘の成功を返さない（非対応 OS は [`UnsupportedUwbRanging`]）。
//!
//! 守らない: 3 台以上が居るときの相手の選び方（最初に token を交換できた 1 台）、交換窓の端での
//! 非対称（片方は相手なし、片方は時間切れ）、同じサービス種別を名乗る別インスタンスの認証、
//! 距離の精度・更新間隔・遮蔽による欠測（OS が決める）、背景からの自動再開、複数プロセスの
//! 同時測距、表示の遅れ（[`READ_INTERVAL`] + OS の更新周期）、ローカルネットワークの許可を
//! 拒否したときの扱い（MPC は通知しないため相手なしに見える）。
//!
//! # 並行経路（契約表 11）
//!
//! Rust 側は画面の driver 1 本の `wanted` の立ち上がり・立ち下がりでだけ `start` / `stop` する。
//! iOS 実装は MPC の delegate・`NISession` の delegate・背景遷移の通知・`start` / `stop` を
//! 1 本の serial queue に集める。main へ `dispatch_sync` する呼び出しを、main 側も取るロックで囲まない。
//!
//! # 時刻
//!
//! 期限・古さの判定は、呼び出し側が単調時計（`Instant` 由来）から作ったミリ秒の `f64` で行う
//! （壁時計は使わない、契約表 3）。
//!
//! # ログ
//!
//! 距離・token・相手の表示名・`NSError` のコードをログへ出さない（契約表 8、`security.md`）。
//!
//! # 後続 iOS 実装（#654）への要点
//!
//! - 能力は `NISession.deviceCapabilities`（iOS 16+）。それより前は `isSupported` で距離を判定し、
//!   方向は [`Support::Unknown`] にする
//! - token は `NSKeyedArchiver` で送る。`didRemoveNearbyObjects` の理由が `peerEnded` なら
//!   [`UwbPoll::PeerEnded`]、`timeout` なら [`UwbPoll::NoMeasurement`]
//! - `sessionWasSuspended` と背景遷移では資源を解放して [`UwbPoll::Interrupted`] にする
//! - コールバックで共有状態を更新した直後に `crate::ios::request_repaint` を呼ぶ
//! - Info.plist に `NSNearbyInteractionUsageDescription`・`NSBonjourServices`・
//!   `NSLocalNetworkUsageDescription` を足す

use std::time::Duration;

/// MPC のサービス種別（15 文字以内）。対応表の `_fandhe-uwb._tcp` / `_udp` と一致させる。
/// 値は #648 の実機確認で見直す。
pub const SERVICE_TYPE: &str = "fandhe-uwb";

/// 開始から測距開始まで（探索 + token 交換）の窓。期限は driver の単調時計だけで測る
/// （OS 側に別の期限を持たない）。値は #648 の実機確認で見直す。
pub const EXCHANGE_WINDOW_MS: f64 = 30_000.0;

/// driver が OS を読み直す間隔。
pub const READ_INTERVAL: Duration = Duration::from_millis(100);

/// 受信番号が進まない時間がこれを超えたら距離を出さない（古い値を見せない）。
pub const STALE_AFTER_MS: f64 = 2000.0;

/// 受け入れる距離の上限（m）。
pub const MAX_DISTANCE_M: f32 = 50.0;

/// 相手から受ける discovery token（アーカイブ済みバイト列）の上限バイト数。
pub const MAX_TOKEN_BYTES: usize = 4096;

/// 能力の 1 項目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// 使える。
    Yes,
    /// 使えない。
    No,
    /// OS 版の都合で判定できない。`Yes` へ畳まない。
    Unknown,
}

/// 端末の測距能力（許可なしで読める）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UwbCapabilities {
    /// 距離を測れるか。
    pub distance: Support,
    /// 方向を測れるか。
    pub direction: Support,
}

/// 距離の 1 サンプル。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeSample {
    /// OS の生の距離（m）。検証は driver が [`validate_distance_m`] で 1 か所だけ行う。
    pub distance_m: f32,
    /// 受信番号。プロセス内で単調増加し、開始し直しても巻き戻さない。
    pub seq: u64,
}

/// [`UwbRanging::read`] の成功結果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UwbPoll {
    /// 相手を探している。未開始・停止後もこれを返す。
    Searching,
    /// 相手とつながり、token を交換中。
    Exchanging,
    /// 許可ダイアログの応答待ち。期限に数えない。
    AwaitingPermission,
    /// 距離が出ている。
    Measured(RangeSample),
    /// 測距は始まったが距離が無い。0 m や前回値で代用しない。
    NoMeasurement,
    /// 相手が測距を止めた・去った。終端で、失敗ではない。
    PeerEnded,
    /// 背景遷移や OS の中断で、実装が資源を解放済み。終端で、自動では再開しない。
    Interrupted,
}

/// 期限の判定に使う段階。終端は持たない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangePhase {
    /// 探索中。
    Searching,
    /// token 交換中。
    Exchanging,
    /// 許可待ち。
    AwaitingPermission,
    /// 測距が始まった（距離の有無は問わない）。
    Ranging,
}

impl UwbPoll {
    /// 期限判定用の段階。終端（`PeerEnded` / `Interrupted`）は `None`。
    pub fn exchange_phase(&self) -> Option<ExchangePhase> {
        match self {
            UwbPoll::Searching => Some(ExchangePhase::Searching),
            UwbPoll::Exchanging => Some(ExchangePhase::Exchanging),
            UwbPoll::AwaitingPermission => Some(ExchangePhase::AwaitingPermission),
            UwbPoll::Measured(_) | UwbPoll::NoMeasurement => Some(ExchangePhase::Ranging),
            UwbPoll::PeerEnded | UwbPoll::Interrupted => None,
        }
    }
}

/// 正常終了の理由。`NoPeer` は driver が合成し、他は実装の終端を写す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// 相手が測距を止めた。
    PeerEnded,
    /// 中断（背景遷移等）。
    Interrupted,
    /// 窓の間に一度も相手と token を交換できなかった。失敗ではない。
    NoPeer,
}

/// 失敗の種別。OS 固有のコードは含めない（契約表 8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UwbFailure {
    /// OS に API が無い（macOS・Windows・Linux・未実装の iOS スタブ）。
    UnsupportedOnOs,
    /// API はあるが端末に UWB が無い。
    NotCapable,
    /// 近くのデバイスとのやり取りの許可が拒否された。
    PermissionDenied,
    /// 窓の中で token 交換が終わらない。driver が合成する。
    Timeout,
    /// 上記以外（Info.plist の未宣言を含む）。
    Failed,
}

/// UWB 測距の契約。UI スレッドから同期で呼ばれるため、どのメソッドもすぐに戻ること。
pub trait UwbRanging: Send + Sync {
    /// 端末の能力。同期で軽く、許可は要らない。
    fn capabilities(&self) -> Result<UwbCapabilities, UwbFailure>;

    /// 探索と token 交換を始める。冪等。同期で分かる失敗（非搭載・非対応）はここで返す。
    fn start(&self) -> Result<(), UwbFailure>;

    /// 現在の状態。直前の値を返し続けない。終端（`PeerEnded` / `Interrupted`）は `stop` まで同じ値を返す。
    fn read(&self) -> Result<UwbPoll, UwbFailure>;

    /// 止めて資源を解放する。冪等で失敗しない。戻る前に実装内の世代を進め、以降に届く
    /// OS コールバックを捨てる。MPC と `NISession` の解放は実装の serial queue で必ず行う。
    fn stop(&self);
}

/// 非対応 OS の実装。どのメソッドも [`UwbFailure::UnsupportedOnOs`] を返し、ダミーの成功へ倒さない。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedUwbRanging;

impl UwbRanging for UnsupportedUwbRanging {
    fn capabilities(&self) -> Result<UwbCapabilities, UwbFailure> {
        Err(UwbFailure::UnsupportedOnOs)
    }
    fn start(&self) -> Result<(), UwbFailure> {
        Err(UwbFailure::UnsupportedOnOs)
    }
    fn read(&self) -> Result<UwbPoll, UwbFailure> {
        Err(UwbFailure::UnsupportedOnOs)
    }
    fn stop(&self) {}
}

/// 距離（m）の検証。有限で `0.0..=MAX_DISTANCE_M` のときだけ通す。丸めの前に行う（契約表 5）。
pub fn validate_distance_m(raw: f32) -> Option<f32> {
    if raw.is_finite() && (0.0..=MAX_DISTANCE_M).contains(&raw) {
        Some(raw)
    } else {
        None
    }
}

/// 検証済みの距離（m）をセンチメートルへ丸める。
pub fn to_centimeters(m: f32) -> u32 {
    (m * 100.0).round() as u32
}

/// 相手の token の長さの検査（`1..=MAX_TOKEN_BYTES`）。解凍の前に行う。
pub fn is_valid_token_len(len: usize) -> bool {
    (1..=MAX_TOKEN_BYTES).contains(&len)
}

/// 窓が切れた理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    /// 一度も交換まで進まなかった（正常終了の `NoPeer`）。
    NoPeer,
    /// 交換中に窓を越えた（失敗の `Timeout`）。
    Timeout,
}

/// 開始から測距開始までの窓の期限。起点を持つのは driver のこの値だけ。
#[derive(Debug, Clone, Copy, Default)]
pub struct ExchangeDeadline {
    origin: Option<f64>,
    exchanged: bool,
    done: bool,
}

impl ExchangeDeadline {
    /// 起点を置く（開始時）。
    pub fn begin(&mut self, now_ms: f64) {
        *self = Self {
            origin: Some(now_ms),
            exchanged: false,
            done: false,
        };
    }

    /// 記憶を捨てる。
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 段階を観測する。窓を越えていれば理由を返す。許可待ちの間は起点を消し、許可後の最初の観測を
    /// 新しい起点にする。測距が始まった後は期限を外す。期限を過ぎてから届いた測距は受け取らない。
    pub fn observe(&mut self, phase: ExchangePhase, now_ms: f64) -> Option<Expiry> {
        if self.done {
            return None;
        }
        match phase {
            ExchangePhase::AwaitingPermission => {
                self.origin = None;
                None
            }
            ExchangePhase::Ranging => {
                if self.origin.is_some_and(|o| now_ms - o > EXCHANGE_WINDOW_MS) {
                    return Some(self.expiry());
                }
                self.done = true;
                self.origin = None;
                None
            }
            ExchangePhase::Searching | ExchangePhase::Exchanging => {
                if phase == ExchangePhase::Exchanging {
                    self.exchanged = true;
                }
                let o = *self.origin.get_or_insert(now_ms);
                if now_ms - o > EXCHANGE_WINDOW_MS {
                    Some(self.expiry())
                } else {
                    None
                }
            }
        }
    }

    fn expiry(&self) -> Expiry {
        if self.exchanged {
            Expiry::Timeout
        } else {
            Expiry::NoPeer
        }
    }
}

/// 受信番号が進まない時間を測り、古い距離を出さないための記憶。
#[derive(Debug, Clone, Copy, Default)]
pub struct StaleTracker {
    last: Option<(u64, f64)>,
}

impl StaleTracker {
    /// 記憶を捨てる。
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// 受信番号を観測する。進んでいれば真、[`STALE_AFTER_MS`] を超えて進まなければ偽。
    pub fn observe(&mut self, seq: u64, now_ms: f64) -> bool {
        match self.last {
            Some((s, since)) if s == seq => now_ms - since <= STALE_AFTER_MS,
            _ => {
                self.last = Some((seq, now_ms));
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_validation() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.1, 50.1] {
            assert_eq!(validate_distance_m(bad), None);
        }
        assert_eq!(validate_distance_m(0.0), Some(0.0));
        assert_eq!(validate_distance_m(MAX_DISTANCE_M), Some(MAX_DISTANCE_M));
    }

    #[test]
    fn centimeters_round() {
        assert_eq!(to_centimeters(1.234), 123);
        assert_eq!(to_centimeters(1.236), 124);
        assert_eq!(to_centimeters(0.0), 0);
        assert_eq!(to_centimeters(50.0), 5000);
    }

    #[test]
    fn token_len() {
        assert!(!is_valid_token_len(0));
        assert!(is_valid_token_len(1));
        assert!(is_valid_token_len(MAX_TOKEN_BYTES));
        assert!(!is_valid_token_len(MAX_TOKEN_BYTES + 1));
    }

    fn begun() -> ExchangeDeadline {
        let mut d = ExchangeDeadline::default();
        d.begin(0.0);
        d
    }

    #[test]
    fn searching_past_window_is_no_peer() {
        let mut d = begun();
        assert_eq!(
            d.observe(ExchangePhase::Searching, EXCHANGE_WINDOW_MS),
            None
        );
        assert_eq!(
            d.observe(ExchangePhase::Searching, EXCHANGE_WINDOW_MS + 1.0),
            Some(Expiry::NoPeer)
        );
    }

    #[test]
    fn exchanging_past_window_is_timeout() {
        let mut d = begun();
        assert_eq!(d.observe(ExchangePhase::Exchanging, 1000.0), None);
        assert_eq!(d.observe(ExchangePhase::Searching, 1500.0), None);
        assert_eq!(
            d.observe(ExchangePhase::Searching, EXCHANGE_WINDOW_MS + 1.0),
            Some(Expiry::Timeout)
        );
    }

    #[test]
    fn permission_wait_is_not_counted() {
        let mut d = begun();
        assert_eq!(d.observe(ExchangePhase::AwaitingPermission, 1000.0), None);
        assert_eq!(
            d.observe(ExchangePhase::AwaitingPermission, 600_000.0),
            None
        );
        // 許可後の最初の観測が新しい起点。
        assert_eq!(d.observe(ExchangePhase::Searching, 600_100.0), None);
        assert_eq!(d.observe(ExchangePhase::Searching, 620_000.0), None);
        assert_eq!(
            d.observe(
                ExchangePhase::Searching,
                600_100.0 + EXCHANGE_WINDOW_MS + 1.0
            ),
            Some(Expiry::NoPeer)
        );
    }

    #[test]
    fn no_deadline_once_ranging() {
        let mut d = begun();
        assert_eq!(d.observe(ExchangePhase::Ranging, 1000.0), None);
        assert_eq!(d.observe(ExchangePhase::Searching, 900_000.0), None);
        assert_eq!(d.observe(ExchangePhase::Ranging, 900_000.0), None);
    }

    #[test]
    fn late_ranging_is_rejected() {
        let mut d = begun();
        assert_eq!(d.observe(ExchangePhase::Exchanging, 100.0), None);
        assert_eq!(
            d.observe(ExchangePhase::Ranging, EXCHANGE_WINDOW_MS + 1.0),
            Some(Expiry::Timeout)
        );
    }

    #[test]
    fn stale_tracker() {
        let mut t = StaleTracker::default();
        assert!(t.observe(1, 0.0));
        assert!(t.observe(1, STALE_AFTER_MS));
        assert!(!t.observe(1, STALE_AFTER_MS + 1.0));
        assert!(t.observe(2, STALE_AFTER_MS + 2.0));
        t.reset();
        assert!(t.observe(2, 9999.0));
    }

    #[test]
    fn terminal_polls_have_no_phase() {
        assert_eq!(UwbPoll::PeerEnded.exchange_phase(), None);
        assert_eq!(UwbPoll::Interrupted.exchange_phase(), None);
        assert_eq!(
            UwbPoll::NoMeasurement.exchange_phase(),
            Some(ExchangePhase::Ranging)
        );
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedUwbRanging;
        assert_eq!(u.capabilities(), Err(UwbFailure::UnsupportedOnOs));
        assert_eq!(u.start(), Err(UwbFailure::UnsupportedOnOs));
        assert_eq!(u.read(), Err(UwbFailure::UnsupportedOnOs));
        u.stop();
    }
}
