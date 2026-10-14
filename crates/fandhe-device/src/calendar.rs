//! 予定を 1 件追加する機能の抽象（Phase 11 の拡充ループ、親 #1118、Issue #1224）。
//!
//! 範囲は「予定を 1 件追加するだけ」。読み取り・一覧・リマインダーは扱わない。
//! macOS は `EKEventStore` の write-only 権限（macOS 14 以上）で保存し、iOS（#1225）は
//! 権限なしで `EKEventEditViewController` を出す（iOS 17 以上）。Android（#1226、
//! `Intent.ACTION_INSERT`）は後から同じ契約に乗せる。macOS / iOS は OS がコールバック型の
//! ため [`CalendarEvents`] を通らず、`fandhe_core::screens::calendar_add`
//! （端末依存ゼロの状態機械）へ pull 型 `bridge` で書き戻す（`biometric_auth` と同じ判断）。
//! それ以外は [`UnsupportedCalendar`]。
//!
//! # 結果の種類（[`AddEventOutcome`]）
//!
//! - `Saved`: OS のストアへの保存が確定した（macOS の `saveEvent:` 成功、iOS の
//!   `EKEventEditViewActionSaved`）
//! - `HandedToOs`: OS の編集画面へ渡しただけで、保存されたかは OS が決める
//!   （Android）。「保存しました」と見せない
//! - `Cancelled`: 利用者が OS の編集画面でキャンセルした（iOS）。失敗として見せない
//!
//! macOS で到達するのは `Saved`・権限拒否・内部失敗・非対応・入力不正のみ。
//! iOS で到達するのは `Saved`・`Cancelled`・内部失敗（提示失敗）・入力不正・
//! 非対応（iOS 17 未満）。Android は権限を使わず、`HandedToOs` と内部失敗
//! （アプリ無し・非前景・起動失敗）のみ。
//!
//! | 状況 | 返す値 |
//! |---|---|
//! | 権限拒否・制限 | `PermissionDenied("calendar")` |
//! | 入力不正 | `InvalidInput("calendar title" / "calendar time" / "calendar range")` |
//! | 内部失敗 | `Internal("calendar ...")` |
//! | 非対応（Linux・Windows・Web・macOS 14 / iOS 17 未満） | `Unsupported("calendar")` |
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内での要求と完了の順序（`request_id` と画面の世代の照合）、
//! 権限拒否・入力不正・失敗・非対応・「OS に渡しただけ」を「保存した」と見せないこと、
//! 入力不正を OS へ渡さないこと、完了を 1 回だけ反映すること。
//!
//! 守らない: 複数プロセスからの同時操作、保存後に利用者がカレンダー App で予定を
//! 編集・削除すること、夏時間の切り替えで存在しない／重なる時刻（OS の正規化に任せる）、
//! 保存中のアプリ終了（OS が片付ける）。
//!
//! 固定タイムアウトは掛けない（TCC の許可ダイアログは利用者の応答を待つため）。
//! 既知の天井: 件名 200 文字、ローカル時刻のみ、終日・繰り返し・通知は扱わない。
//! 件名・日時はログへ出力しない（security.md A09）。

use crate::DeviceError;

/// 追加の結果。`Saved` 以外を「保存した」と見せない（モジュール doc の契約）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddEventOutcome {
    /// OS のストアへの保存が確定した。
    Saved,
    /// OS の編集画面へ渡しただけで、保存されたかは OS 側が決める。
    HandedToOs,
    /// 利用者が OS の編集画面でキャンセルした。
    Cancelled,
}

/// 端末のローカル時刻（分精度）。時差の計算は OS 側が行う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalDateTime {
    /// 年。
    pub year: i32,
    /// 月（1〜12）。
    pub month: u8,
    /// 日（実在する日）。
    pub day: u8,
    /// 時（0〜23）。
    pub hour: u8,
    /// 分（0〜59）。
    pub minute: u8,
}

/// 検査済みの追加要求。[`EventDraft::parse`] 以外では作れない（検査の SSOT、
/// `notification::NotificationRequest::new` と同じ方式）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDraft {
    title: String,
    start: LocalDateTime,
    end: LocalDateTime,
}

/// 件名の最大文字数。
const MAX_TITLE_CHARS: usize = 200;

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i32, m: u8) -> u8 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `YYYY-MM-DD HH:MM`（桁数固定・ASCII のみ）を解釈する。
fn parse_local(s: &str) -> Result<LocalDateTime, DeviceError> {
    let err = || DeviceError::InvalidInput("calendar time");
    let b = s.as_bytes();
    if !s.is_ascii()
        || b.len() != 16
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b' '
        || b[13] != b':'
    {
        return Err(err());
    }
    let num = |r: std::ops::Range<usize>| -> Result<u32, DeviceError> {
        let t = &s[r];
        if t.bytes().all(|c| c.is_ascii_digit()) {
            t.parse().map_err(|_| err())
        } else {
            Err(err())
        }
    };
    let (year, month, day) = (num(0..4)? as i32, num(5..7)? as u8, num(8..10)? as u8);
    let (hour, minute) = (num(11..13)? as u8, num(14..16)? as u8);
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
    {
        return Err(err());
    }
    Ok(LocalDateTime {
        year,
        month,
        day,
        hour,
        minute,
    })
}

impl EventDraft {
    /// 件名・開始・終了を検査する。件名は trim 後 1〜200 文字・NUL なし、日時は
    /// `YYYY-MM-DD HH:MM`、終了は開始以降（同時刻は許す）。
    pub fn parse(title: &str, start: &str, end: &str) -> Result<Self, DeviceError> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS || title.contains('\0') {
            return Err(DeviceError::InvalidInput("calendar title"));
        }
        let (start, end) = (parse_local(start)?, parse_local(end)?);
        if end < start {
            return Err(DeviceError::InvalidInput("calendar range"));
        }
        Ok(Self {
            title: title.to_string(),
            start,
            end,
        })
    }

    /// 件名（trim 済み）。
    pub fn title(&self) -> &str {
        &self.title
    }

    /// 開始。
    pub fn start(&self) -> LocalDateTime {
        self.start
    }

    /// 終了。
    pub fn end(&self) -> LocalDateTime {
        self.end
    }
}

/// 予定追加の契約。
pub trait CalendarEvents: Send + Sync {
    /// 予定を 1 件追加する。固定タイムアウトは掛けない。
    fn add_event(&self, draft: &EventDraft) -> Result<AddEventOutcome, DeviceError>;
}

/// 対応不可プラットフォーム用。常に `Unsupported("calendar")`。
pub struct UnsupportedCalendar;

impl CalendarEvents for UnsupportedCalendar {
    fn add_event(&self, _draft: &EventDraft) -> Result<AddEventOutcome, DeviceError> {
        Err(DeviceError::Unsupported("calendar"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "2026-10-10 10:00";
    const E: &str = "2026-10-10 11:00";

    #[test]
    fn title_rules() {
        for bad in ["", "   ", "a\0b"] {
            assert!(matches!(
                EventDraft::parse(bad, S, E),
                Err(DeviceError::InvalidInput("calendar title"))
            ));
        }
        assert!(EventDraft::parse(&"あ".repeat(201), S, E).is_err());
        assert!(EventDraft::parse(&"あ".repeat(200), S, E).is_ok());
        assert_eq!(EventDraft::parse("  会議 ", S, E).unwrap().title(), "会議");
    }

    #[test]
    fn time_rules() {
        for bad in [
            "2026-1-1 9:00",
            "2026-02-30 10:00",
            "2026-13-01 10:00",
            "2026-10-10 24:00",
            "2026-10-10 10:60",
            "",
            "２０２６-10-10 10:00",
            "2026-10-10T10:00",
            "2026-10-1+ 10:00",
        ] {
            assert!(
                matches!(
                    EventDraft::parse("a", bad, "2030-01-01 00:00"),
                    Err(DeviceError::InvalidInput("calendar time"))
                ),
                "{bad}"
            );
        }
        assert!(EventDraft::parse("a", "2024-02-29 10:00", "2024-02-29 10:00").is_ok());
        assert!(EventDraft::parse("a", "2026-02-29 10:00", "2026-03-01 10:00").is_err());
    }

    #[test]
    fn range_rules() {
        assert!(matches!(
            EventDraft::parse("a", "2026-10-10 11:00", "2026-10-10 10:59"),
            Err(DeviceError::InvalidInput("calendar range"))
        ));
        assert!(EventDraft::parse("a", E, E).is_ok());
    }

    #[test]
    fn unsupported_never_succeeds() {
        let d = EventDraft::parse("a", S, E).unwrap();
        assert!(matches!(
            UnsupportedCalendar.add_event(&d),
            Err(DeviceError::Unsupported("calendar"))
        ));
    }

    #[test]
    fn outcomes_are_distinct() {
        assert_ne!(AddEventOutcome::HandedToOs, AddEventOutcome::Saved);
        assert_ne!(AddEventOutcome::Cancelled, AddEventOutcome::Saved);
    }
}
