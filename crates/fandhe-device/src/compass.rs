//! 値が届かない時間の追跡（ar・compass-heading・environment-sensors が共有する純関数・定数）。
//!
//! 方位の契約（`Compass`・`Heading`・`Accuracy`・`CompassFailure`・`READ_INTERVAL`・`normalize_heading` など）は
//! `fandhe-cap-compass-heading` が持つ。ここに残すのは、2 機能以上が使う OS に触れない部品だけ
//! （`docs/architecture/consumer-app-structure.md` §8）。`fandhe-cap-compass-heading` の `contract` が再公開する。

/// 購読を始めてから（または値なしに戻ってから）この時間、値が届かなければ失敗とする。
pub const NO_READING_TIMEOUT_MS: f64 = 3000.0;

/// 値が届かない時間の追跡（時刻はミリ秒の `f64`。wasm32 で `Instant::now()` が panic するため
/// 呼び出し側が単調時計から作って渡す）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WaitTracker {
    since: Option<f64>,
}

impl WaitTracker {
    /// 起点を置く（購読の開始時）。
    pub fn begin(&mut self, now_ms: f64) {
        self.since = Some(now_ms);
    }

    /// 1 回の読み取り結果を記録する。値があれば起点を消し、値なしで起点が無ければ今を起点にする。
    /// 起点から [`NO_READING_TIMEOUT_MS`] を超えたら `true`（期限切れ）。
    pub fn observe(&mut self, has_value: bool, now_ms: f64) -> bool {
        self.observe_within(has_value, now_ms, NO_READING_TIMEOUT_MS)
    }

    /// [`WaitTracker::observe`] の期限を呼び出し側が決める版（`environment_sensors` が自分の期限で共用する）。
    /// 起点から `timeout_ms` を超えたら `true`。
    pub fn observe_within(&mut self, has_value: bool, now_ms: f64, timeout_ms: f64) -> bool {
        if has_value {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now_ms);
        now_ms - since > timeout_ms
    }

    /// 起点を捨てる（停止時）。
    pub fn reset(&mut self) {
        self.since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_expires_only_after_timeout() {
        let mut t = WaitTracker::default();
        t.begin(0.0);
        assert!(!t.observe(false, NO_READING_TIMEOUT_MS));
        assert!(t.observe(false, NO_READING_TIMEOUT_MS + 1.0));
    }

    #[test]
    fn tracker_restarts_after_live_and_clears_on_value() {
        let mut t = WaitTracker::default();
        t.begin(0.0);
        assert!(!t.observe(true, 10_000.0));
        // Live から値なしに戻った時刻が新しい起点になる。
        assert!(!t.observe(false, 20_000.0));
        assert!(!t.observe(false, 22_000.0));
        assert!(t.observe(false, 23_001.0));
        t.reset();
        assert!(!t.observe(false, 99_999.0));
    }

    #[test]
    fn observe_within_uses_given_timeout() {
        let mut t = WaitTracker::default();
        t.begin(0.0);
        assert!(!t.observe_within(false, 5000.0, 5000.0));
        assert!(t.observe_within(false, 5000.1, 5000.0));
        // 既存の observe は 3000 ms のまま。
        let mut u = WaitTracker::default();
        u.begin(0.0);
        assert!(u.observe(false, 3000.1));
    }
}
