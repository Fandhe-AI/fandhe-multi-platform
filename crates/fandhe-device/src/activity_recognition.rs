//! 行動認識（歩行・走行・乗り物など）の契約（Issue #613、親 #124）。端末が推定した現在の
//! 行動種別と確度を画面に出すための橋渡しで、`fandhe-egui-proxy` の
//! `device/activity_recognition.rs` が購読の開始・停止と間隔つきの [`ActivityRecognizer::read`] を行う
//! （pull 型。OS 側が最新値を持ち、画面側が読む。`compass` と同型）。
//!
//! # 契約
//!
//! - 結果は [`ActivityPoll`] か [`ActivityFailure`]。`DeviceError` へ variant を足すと既存の網羅 match が
//!   壊れるため機能固有の enum にする（`compass::CompassFailure` と同じ判断）
//! - [`ActivityRecognizer::read`] の [`ActivityPoll::Waiting`] は「まだ値が無い / 停止後 / 無効なサンプルを
//!   捨てた後」。直前の値や [`ActivityKind::Stationary`] で代用しない
//!   （`device-bridge-contracts.md`「6. 嘘の成功」）
//! - [`ActivityKind::Unknown`] は OS が「判定なし」と返したことを表す。静止とは別物
//! - **三値**（契約表 10）: 利用者キャンセルの経路は無く、「停止」は失敗ではなく待機へ戻す。
//!   権限拒否・期限切れ（開始から最初の 1 件が [`NO_READING_TIMEOUT_MS`] 届かない）・非対応は
//!   それぞれ別の失敗として表示する
//! - 行動種別・確度・HRESULT・`NSError` 等の OS 固有値をログへ出さない
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内の開始・停止・受信の順序（世代照合）、嘘の値を出さない（非搭載・拒否・未取得・
//! 判定なしを「静止」等へ畳まない）、二重に購読しない、停止・背景遷移・失敗の全経路で購読と
//! リスナーを解除する。
//!
//! 守らない: OS 間で分類が一致すること、判定の遅れ（OS 側の推定に数秒〜数十秒かかる）、
//! [`READ_INTERVAL`] 以内の多重の遷移（最新 1 件だけ保持）、Windows の `Idle` / `Fidgeting` の区別、
//! 背景中の行動、過去の履歴、複数プロセス、iOS のフィットネストラッキング設定を切ったときの扱い。
//!
//! 契約表 11（並行経路）への対応: 経路は画面の driver 1 本に集約され、購読の開始・停止は
//! `wanted` の立ち上がり・立ち下がりでだけ行う。Android も同じで、受信は Receiver から static へ
//! 入れ、`read` が pull する。
//!
//! # Android
//!
//! 骨格段階: 現状の Android 実装は [`ActivityFailure::Unsupported`] を返すスタブで、下記は b-android
//! 実装後に満たす契約（実装時に実体と突き合わせる）。
//!
//! Play services の `ActivityRecognitionClient.requestActivityUpdates(interval, PendingIntent)` を使う
//! （Transition API は確度が無く遷移時しか通知しないため採らない）。
//!
//! - API 28 以下、または `GoogleApiAvailability` の判定が SUCCESS 以外のとき、`start` は
//!   [`ActivityFailure::Unsupported`]（更新ダイアログは出さない）
//! - 実行時の `ACTIVITY_RECOGNITION`（身体活動）許可を待つ間は [`ActivityPoll::AwaitingPermission`]
//!   （期限に数えない）。拒否と `SecurityException` は [`ActivityFailure::PermissionDenied`]、
//!   `ApiException` などは [`ActivityFailure::Failed`]
//! - 背景遷移（`onStop`）で購読を解除して最新値を捨て、復帰（`onStart`）で張り直す
//! - OS の整数コードの写像は [`classify_detected`] が正。Java は生のコードを渡すだけで複製しない
//! - 最初の 1 件の期限は全 OS 共通の [`NO_READING_TIMEOUT_MS`]。Android で 5 秒以内に届くかは
//!   実機未確認で、既知の天井として扱う
//! - 守らない: GMS 側の配信間隔と遅延、PendingIntent 経由の配送遅れ

use std::time::Duration;

/// 画面側が OS を読み直す間隔。
pub const READ_INTERVAL: Duration = Duration::from_millis(250);

/// 開始から最初の 1 件が届くまでの期限。1 件届いた後は期限を設けない（行動が変わらない間は
/// 通知が来ないため）。
pub const NO_READING_TIMEOUT_MS: f64 = 5000.0;

/// 画面に残す履歴の上限件数。
pub const HISTORY_LIMIT: usize = 20;

/// 行動の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    /// 判定なし（OS が分類できなかった）。静止ではない。
    Unknown,
    /// 静止。
    Stationary,
    /// 歩行。
    Walking,
    /// 走行。
    Running,
    /// 自転車。
    Cycling,
    /// 乗り物（自動車・電車など）。
    Automotive,
}

/// 確度。表示するだけで判定には使わない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// 低い。
    Low,
    /// 中。
    Medium,
    /// 高い。
    High,
}

/// ある時点の行動。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityReading {
    /// 種別。
    pub kind: ActivityKind,
    /// 確度。
    pub confidence: Confidence,
    /// 実装がプロセス内で単調増加させる受信番号。購読し直しても巻き戻さない。
    pub seq: u64,
}

/// [`ActivityRecognizer::read`] の成功結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityPoll {
    /// 購読中でまだ値が無い（未開始・停止後・無効サンプルを捨てた後を含む）。
    Waiting,
    /// OS の許可ダイアログの応答待ち（iOS の「モーションとフィットネス」、Android の「身体活動」）。期限に数えない。
    AwaitingPermission,
    /// 最新の行動。
    Reading(ActivityReading),
}

/// 行動認識の失敗種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityFailure {
    /// センサーが無い・API が無い・OS 版が足りない（Android は GMS が無い・使えない、API 28 以下を含む）。
    Unsupported,
    /// 利用者または OS 設定が利用を拒否した。
    PermissionDenied,
    /// 上記以外の OS 呼び出し失敗。
    Failed,
}

/// 行動認識の契約（モジュール doc の「契約」節）。UI スレッドで同期に呼ばれるため短時間で戻ること。
pub trait ActivityRecognizer: Send + Sync {
    /// 購読を始める。冪等。同期で分かる失敗（センサー無し・API 無し）はここで返す。
    fn start(&self) -> Result<(), ActivityFailure>;
    /// 最新の行動を返す。未開始・停止後・値なしは [`ActivityPoll::Waiting`]。直前の値を返し続けない。
    fn read(&self) -> Result<ActivityPoll, ActivityFailure>;
    /// 購読を止め、保持している最新値を捨てる。冪等で失敗しない。
    fn stop(&self);
}

/// 対象外 OS・未実装 OS の dispatch 先。ダミーの成功実装へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedActivityRecognizer;

impl ActivityRecognizer for UnsupportedActivityRecognizer {
    fn start(&self) -> Result<(), ActivityFailure> {
        Err(ActivityFailure::Unsupported)
    }
    fn read(&self) -> Result<ActivityPoll, ActivityFailure> {
        Err(ActivityFailure::Unsupported)
    }
    fn stop(&self) {}
}

/// OS が排他でなく同時に立てる分類フラグを 1 つの種別へまとめる。
/// 優先順位は automotive > cycling > running > walking > stationary。全部 `false` は
/// [`ActivityKind::Unknown`]（iOS の `unknown` フラグは見ない）。信号待ちの車内（静止と乗り物が
/// 同時に立つ）は乗り物になる。
pub fn classify_flags(
    stationary: bool,
    walking: bool,
    running: bool,
    automotive: bool,
    cycling: bool,
) -> ActivityKind {
    if automotive {
        ActivityKind::Automotive
    } else if cycling {
        ActivityKind::Cycling
    } else if running {
        ActivityKind::Running
    } else if walking {
        ActivityKind::Walking
    } else if stationary {
        ActivityKind::Stationary
    } else {
        ActivityKind::Unknown
    }
}

// Android `DetectedActivity` の型コード（正は Rust 側。Java は生のコードを渡す）。
const ANDROID_IN_VEHICLE: i32 = 0;
const ANDROID_ON_BICYCLE: i32 = 1;
const ANDROID_ON_FOOT: i32 = 2;
const ANDROID_STILL: i32 = 3;
const ANDROID_UNKNOWN: i32 = 4;
const ANDROID_TILTING: i32 = 5;
const ANDROID_WALKING: i32 = 7;
const ANDROID_RUNNING: i32 = 8;

/// Android の確度（0〜100）を [`Confidence`] へ写す。範囲外は `None`（捨てる）。
pub fn confidence_from_percent(p: i32) -> Option<Confidence> {
    match p {
        0..=49 => Some(Confidence::Low),
        50..=74 => Some(Confidence::Medium),
        75..=100 => Some(Confidence::High),
        _ => None,
    }
}

/// Android の `getProbableActivities()`（確度の高い順）を `(type, confidence)` の組で受け、
/// 先頭から種別と確度を決める。空・確度が範囲外・未知のコードは `None`（静止へ畳まない）。
/// 先頭が ON_FOOT のときは配列内の WALKING / RUNNING の確度の大きい方（同点は歩行、無ければ Unknown）。
/// `ActivityRecognitionResult` の `read` が通す。
pub fn classify_detected(probable: &[(i32, i32)]) -> Option<(ActivityKind, Confidence)> {
    if probable
        .iter()
        .any(|&(_, c)| confidence_from_percent(c).is_none())
    {
        return None;
    }
    let &(top, top_conf) = probable.first()?;
    let conf = confidence_from_percent(top_conf)?;
    let kind = match top {
        ANDROID_IN_VEHICLE => ActivityKind::Automotive,
        ANDROID_ON_BICYCLE => ActivityKind::Cycling,
        ANDROID_RUNNING => ActivityKind::Running,
        ANDROID_WALKING => ActivityKind::Walking,
        ANDROID_STILL => ActivityKind::Stationary,
        ANDROID_UNKNOWN | ANDROID_TILTING => ActivityKind::Unknown,
        ANDROID_ON_FOOT => {
            let best = |t: i32| probable.iter().find(|&&(k, _)| k == t).map(|&(_, c)| c);
            match (best(ANDROID_WALKING), best(ANDROID_RUNNING)) {
                (Some(w), Some(r)) if r > w => ActivityKind::Running,
                (Some(_), _) => ActivityKind::Walking,
                (None, Some(_)) => ActivityKind::Running,
                (None, None) => ActivityKind::Unknown,
            }
        }
        _ => return None,
    };
    Some((kind, conf))
}

/// [`ActivityPoll`] の種別（期限判定に渡す、値を持たない写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollKind {
    /// [`ActivityPoll::Waiting`]。
    Waiting,
    /// [`ActivityPoll::AwaitingPermission`]。
    AwaitingPermission,
    /// [`ActivityPoll::Reading`]。
    Reading,
}

impl From<&ActivityPoll> for PollKind {
    fn from(p: &ActivityPoll) -> Self {
        match p {
            ActivityPoll::Waiting => PollKind::Waiting,
            ActivityPoll::AwaitingPermission => PollKind::AwaitingPermission,
            ActivityPoll::Reading(_) => PollKind::Reading,
        }
    }
}

/// 開始から最初の 1 件までの期限の追跡（時刻はミリ秒の `f64`。wasm32 で `Instant::now()` が
/// panic するため呼び出し側が単調時計から作って渡す）。1 件届いた後は期限なし。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FirstReadingDeadline {
    since: Option<f64>,
    received: bool,
}

impl FirstReadingDeadline {
    /// 起点を置く（購読の開始時）。受信済みの記録は消す。
    pub fn begin(&mut self, now_ms: f64) {
        self.since = Some(now_ms);
        self.received = false;
    }

    /// 1 回の読み取り結果を記録し、期限切れなら `true`。
    /// - `Reading`: 受信済みにして期限を外す
    /// - `AwaitingPermission`: 起点を消す（許可待ちは数えない。許可後の最初の `Waiting` が新しい起点）
    /// - `Waiting`: 受信済みなら `false`。未受信なら起点からの経過が [`NO_READING_TIMEOUT_MS`] を
    ///   超えたときだけ `true`
    pub fn observe(&mut self, poll: PollKind, now_ms: f64) -> bool {
        match poll {
            PollKind::Reading => {
                self.received = true;
                false
            }
            PollKind::AwaitingPermission => {
                self.since = None;
                false
            }
            PollKind::Waiting => {
                if self.received {
                    return false;
                }
                let since = *self.since.get_or_insert(now_ms);
                now_ms - since > NO_READING_TIMEOUT_MS
            }
        }
    }

    /// 背景から復帰したとき、未受信なら起点を引き直す。
    pub fn resume(&mut self, now_ms: f64) {
        if !self.received {
            self.since = Some(now_ms);
        }
    }

    /// 起点と受信記録を捨てる（停止時）。
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detected_simple_codes() {
        use ActivityKind::*;
        for (code, kind) in [
            (0, Automotive),
            (1, Cycling),
            (3, Stationary),
            (4, Unknown),
            (5, Unknown),
            (7, Walking),
            (8, Running),
        ] {
            assert_eq!(
                classify_detected(&[(code, 80)]),
                Some((kind, Confidence::High))
            );
        }
    }

    #[test]
    fn detected_on_foot() {
        use ActivityKind::*;
        let k = |v: &[(i32, i32)]| classify_detected(v).map(|r| r.0);
        assert_eq!(k(&[(2, 90), (7, 60), (8, 30)]), Some(Walking));
        assert_eq!(k(&[(2, 90), (8, 60), (7, 30)]), Some(Running));
        assert_eq!(k(&[(2, 90), (7, 30), (8, 30)]), Some(Walking));
        assert_eq!(k(&[(2, 90), (3, 10)]), Some(Unknown));
    }

    #[test]
    fn detected_rejects_bad_input() {
        assert_eq!(classify_detected(&[]), None);
        for code in [6, 9, -1] {
            assert_eq!(classify_detected(&[(code, 50)]), None);
        }
        assert_eq!(classify_detected(&[(7, 80), (3, 101)]), None);
        assert_eq!(classify_detected(&[(7, -1)]), None);
    }

    #[test]
    fn confidence_boundaries() {
        use Confidence::*;
        for (p, c) in [
            (0, Low),
            (49, Low),
            (50, Medium),
            (74, Medium),
            (75, High),
            (100, High),
        ] {
            assert_eq!(confidence_from_percent(p), Some(c));
        }
        assert_eq!(confidence_from_percent(-1), None);
        assert_eq!(confidence_from_percent(101), None);
    }

    #[test]
    fn classify_priority() {
        use ActivityKind::*;
        assert_eq!(classify_flags(false, false, false, false, false), Unknown);
        assert_eq!(classify_flags(true, false, false, false, false), Stationary);
        assert_eq!(classify_flags(true, false, false, true, false), Automotive);
        assert_eq!(classify_flags(false, true, true, false, false), Running);
        assert_eq!(classify_flags(false, true, false, false, true), Cycling);
        assert_eq!(classify_flags(true, true, true, true, true), Automotive);
        assert_eq!(classify_flags(true, true, false, false, false), Walking);
    }

    #[test]
    fn deadline_expires_only_after_timeout() {
        let mut d = FirstReadingDeadline::default();
        d.begin(0.0);
        assert!(!d.observe(PollKind::Waiting, NO_READING_TIMEOUT_MS));
        assert!(d.observe(PollKind::Waiting, NO_READING_TIMEOUT_MS + 1.0));
    }

    #[test]
    fn permission_wait_is_not_counted() {
        let mut d = FirstReadingDeadline::default();
        d.begin(0.0);
        assert!(!d.observe(PollKind::AwaitingPermission, 60_000.0));
        // 許可後の最初の Waiting が新しい起点。
        assert!(!d.observe(PollKind::Waiting, 61_000.0));
        assert!(!d.observe(PollKind::Waiting, 61_000.0 + NO_READING_TIMEOUT_MS));
        assert!(d.observe(PollKind::Waiting, 61_001.0 + NO_READING_TIMEOUT_MS));
    }

    #[test]
    fn no_deadline_after_first_reading() {
        let mut d = FirstReadingDeadline::default();
        d.begin(0.0);
        assert!(!d.observe(PollKind::Reading, 1.0));
        assert!(!d.observe(PollKind::Waiting, 999_999.0));
        d.reset();
        assert!(!d.observe(PollKind::Waiting, 0.0));
    }

    #[test]
    fn resume_restarts_unreceived_only() {
        let mut d = FirstReadingDeadline::default();
        d.begin(0.0);
        d.resume(10_000.0);
        assert!(!d.observe(PollKind::Waiting, 10_000.0 + NO_READING_TIMEOUT_MS));
        assert!(d.observe(PollKind::Waiting, 10_001.0 + NO_READING_TIMEOUT_MS));
        let mut d = FirstReadingDeadline::default();
        d.begin(0.0);
        d.observe(PollKind::Reading, 1.0);
        d.resume(10_000.0);
        assert!(!d.observe(PollKind::Waiting, 99_999.0));
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedActivityRecognizer;
        assert_eq!(u.start(), Err(ActivityFailure::Unsupported));
        assert_eq!(u.read(), Err(ActivityFailure::Unsupported));
        u.stop();
    }
}
