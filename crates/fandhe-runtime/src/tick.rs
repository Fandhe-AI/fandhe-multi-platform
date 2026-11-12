//! フレームごとの経過時間の実測。画面が時間で進む端末機能（録音・音声認識・画面録画）の effect ドライバが共用する。
//!
//! 経過時間は単調時計（`Instant`）で測る。システム時刻の変更で 0 や負の値にならない（契約表 3）。
//! 呼び出し元は各機能 crate の driver（フレームごとに同期で呼ぶ）と、`fandhe-egui-proxy` の録音・音声認識。

use std::sync::Mutex;
use std::time::Instant;

/// 1 回の呼び出しで進める経過ミリ秒の上限（バックグラウンド復帰などの飛び対策）。
pub const MAX_TICK_MS: u64 = 1_000;

/// 前回呼び出しからの経過ミリ秒を実測する。`last` は呼び出し元が持つ「前回の時刻」の保管所。
/// `active` が `false`（計測対象の状態でない）なら実測せず `last` をリセットして `0` を返す。
/// 初回（`last` が空）は `0`。戻り値は [`MAX_TICK_MS`] を超えない。
pub fn measure_dt_ms(last: &Mutex<Option<Instant>>, active: bool) -> u64 {
    let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
    if !active {
        *last = None;
        return 0;
    }
    let now = Instant::now();
    let dt_ms = match *last {
        Some(prev) => now.duration_since(prev).as_millis() as u64,
        None => 0,
    };
    *last = Some(now);
    dt_ms.min(MAX_TICK_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inactive_resets_and_returns_zero() {
        let last = Mutex::new(Some(Instant::now()));
        assert_eq!(measure_dt_ms(&last, false), 0);
        assert!(last.lock().unwrap().is_none());
    }

    #[test]
    fn first_call_is_zero_then_bounded() {
        let last = Mutex::new(None);
        assert_eq!(measure_dt_ms(&last, true), 0);
        // 前回が十分過去でも上限で頭打ちになる。
        *last.lock().unwrap() = Instant::now().checked_sub(std::time::Duration::from_secs(10));
        assert_eq!(measure_dt_ms(&last, true), MAX_TICK_MS);
    }
}
