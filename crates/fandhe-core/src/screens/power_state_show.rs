//! Phase 11 の拡充ループ（親 #1128）型サンプル「電源状態」（Issue #1274）。
//! 電池残量・充電状態・省電力モード・熱状態を 1 画面に並べる。`FANDHE_PROXY_APP=power_state_show`
//! で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod power_state_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。表示用の型は device 側と別に持ち、写像は呼び出し側
//! （proxy の `device/power_state.rs`、Web は #1277 の `fandhe-web-dom`）が行う
//! （`text_scale_preview` と同じ「コアは device に依存できないので複製する」判断）。
//!
//! 操作 UI は無い（1 秒間隔の pull のみ）。結果は [`finish_read`] で注入する。`Read` の後に
//! エラーが来たら前回値を捨てる（最後の値を出し続けない、契約表 1）。
//!
//! 全対象 OS の文言をここで確定済みで、後続の OS 実装は本画面を触らない。

use crate::Screen;

/// 値が取れない項目の表示。
pub const UNKNOWN_TEXT: &str = "取得できません";
/// 電池が無い機種の表示（0% ではない）。
pub const ABSENT_TEXT: &str = "電池なし";
/// 未読み取りの案内。
pub const UNREAD_STATUS: &str = "読み取り中";
/// 読み取り中（正常稼働）の案内。
pub const POLLING_STATUS: &str = "1 秒ごとに読み直しています";
/// 非対応環境の案内。
pub const UNAVAILABLE_STATUS: &str = "この環境では電源状態を取得できません";
/// 失敗時の接頭辞（`{接頭辞}{エラー文字列}`）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 充電状態の表示用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargingView {
    /// 充電中。
    Charging,
    /// 放電中。
    Discharging,
    /// 満充電。
    Full,
    /// 取得できない。
    Unknown,
}

/// 電池の表示用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryView {
    /// 電池なし。
    Absent,
    /// 電池あり。
    Present {
        /// 残量（%）。取れなければ `None`。
        level: Option<u8>,
        /// 充電状態。
        charging: ChargingView,
    },
    /// 電池の有無が取得できない。
    Unknown,
}

/// 熱状態の表示用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalView {
    /// 通常。
    Nominal,
    /// やや高い。
    Fair,
    /// 高い。
    Serious,
    /// 危険。
    Critical,
    /// 取得できない。
    Unknown,
}

/// 1 回の読み取り結果（表示用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerReading {
    /// 電池。
    pub battery: BatteryView,
    /// 省電力モード。
    pub low_power_mode: Option<bool>,
    /// 熱状態。
    pub thermal: ThermalView,
}

/// 読み取りの失敗種別（device の `Unsupported` / その他の写し）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerReadError {
    /// この環境では取得できない（恒久的）。
    Unavailable,
    /// 一時的な失敗（表示用の固定文言）。
    Failed(String),
}

/// 画面全体の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PowerView {
    /// まだ読んでいない。
    #[default]
    Unread,
    /// 読み取れた。
    Read(PowerReading),
    /// 非対応。
    Unavailable,
    /// 失敗。
    Failed(String),
}

/// 電源状態画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PowerStateShowState {
    view: PowerView,
}

/// 現在の表示状態（effect ドライバの「前回と同じなら送らない」判定用）。
pub fn view(state: &PowerStateShowState) -> &PowerView {
    &state.view
}

/// 読み取り結果を反映する。失敗時は前回値を捨てる。
pub fn finish_read(state: &mut PowerStateShowState, result: Result<PowerReading, PowerReadError>) {
    state.view = match result {
        Ok(reading) => PowerView::Read(reading),
        Err(PowerReadError::Unavailable) => PowerView::Unavailable,
        Err(PowerReadError::Failed(message)) => PowerView::Failed(message),
    };
}

fn read_of(state: &PowerStateShowState) -> Option<&PowerReading> {
    match &state.view {
        PowerView::Read(r) => Some(r),
        _ => None,
    }
}

/// 電池残量の行。
pub fn battery_text(state: &PowerStateShowState) -> String {
    let value = match read_of(state).map(|r| r.battery) {
        Some(BatteryView::Absent) => ABSENT_TEXT.to_string(),
        Some(BatteryView::Present { level: Some(p), .. }) => format!("{p}%"),
        _ => UNKNOWN_TEXT.to_string(),
    };
    format!("電池残量: {value}")
}

/// 充電状態の行。
pub fn charging_text(state: &PowerStateShowState) -> String {
    let value = match read_of(state).map(|r| r.battery) {
        Some(BatteryView::Absent) => ABSENT_TEXT,
        Some(BatteryView::Present { charging, .. }) => match charging {
            ChargingView::Charging => "充電中",
            ChargingView::Discharging => "放電中",
            ChargingView::Full => "満充電",
            ChargingView::Unknown => UNKNOWN_TEXT,
        },
        _ => UNKNOWN_TEXT,
    };
    format!("充電: {value}")
}

/// 省電力モードの行。
pub fn low_power_text(state: &PowerStateShowState) -> String {
    let value = match read_of(state).and_then(|r| r.low_power_mode) {
        Some(true) => "オン",
        Some(false) => "オフ",
        None => UNKNOWN_TEXT,
    };
    format!("省電力モード: {value}")
}

/// 熱状態の行。
pub fn thermal_text(state: &PowerStateShowState) -> String {
    let value = match read_of(state).map(|r| r.thermal) {
        Some(ThermalView::Nominal) => "通常",
        Some(ThermalView::Fair) => "やや高い",
        Some(ThermalView::Serious) => "高い",
        Some(ThermalView::Critical) => "危険",
        _ => UNKNOWN_TEXT,
    };
    format!("熱状態: {value}")
}

/// 状態行。
pub fn status_text(state: &PowerStateShowState) -> String {
    match state.view {
        PowerView::Unread => UNREAD_STATUS,
        PowerView::Read(_) => POLLING_STATUS,
        PowerView::Unavailable => UNAVAILABLE_STATUS,
        // 失敗は `error_text` に出す。
        PowerView::Failed(_) => "",
    }
    .to_string()
}

/// 失敗行（`TextRole::Error` で描画される）。
pub fn error_text(state: &PowerStateShowState) -> String {
    match &state.view {
        PowerView::Failed(message) => format!("{FAILED_PREFIX}{message}"),
        _ => String::new(),
    }
}

/// 電源状態画面。固定 1 枚。
pub fn power_state_show_screen() -> Screen<PowerStateShowState> {
    Screen::new("電源状態")
        .dynamic_text(status_text)
        .dynamic_text(battery_text)
        .dynamic_text(charging_text)
        .dynamic_text(low_power_text)
        .dynamic_text(thermal_text)
        .error_text(error_text)
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口。
pub fn current_screen(_state: &PowerStateShowState) -> Screen<PowerStateShowState> {
    power_state_show_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    fn reading(battery: BatteryView) -> PowerReading {
        PowerReading {
            battery,
            low_power_mode: Some(true),
            thermal: ThermalView::Fair,
        }
    }

    fn read_state(r: PowerReading) -> PowerStateShowState {
        let mut s = PowerStateShowState::default();
        finish_read(&mut s, Ok(r));
        s
    }

    #[test]
    fn initial_is_unread() {
        let s = PowerStateShowState::default();
        assert_eq!(status_text(&s), UNREAD_STATUS);
        assert_eq!(battery_text(&s), format!("電池残量: {UNKNOWN_TEXT}"));
    }

    #[test]
    fn absent_battery_is_not_zero_percent() {
        let s = read_state(reading(BatteryView::Absent));
        assert_eq!(battery_text(&s), "電池残量: 電池なし");
        assert!(!battery_text(&s).contains('%'));
        assert_eq!(charging_text(&s), "充電: 電池なし");
    }

    #[test]
    fn unknown_values_are_reported_as_unavailable() {
        let s = read_state(PowerReading {
            battery: BatteryView::Present {
                level: None,
                charging: ChargingView::Unknown,
            },
            low_power_mode: None,
            thermal: ThermalView::Unknown,
        });
        for line in [
            battery_text(&s),
            charging_text(&s),
            low_power_text(&s),
            thermal_text(&s),
        ] {
            assert!(line.ends_with(UNKNOWN_TEXT), "{line}");
        }
    }

    #[test]
    fn present_values_are_shown() {
        let s = read_state(reading(BatteryView::Present {
            level: Some(80),
            charging: ChargingView::Charging,
        }));
        assert_eq!(battery_text(&s), "電池残量: 80%");
        assert_eq!(charging_text(&s), "充電: 充電中");
        assert_eq!(low_power_text(&s), "省電力モード: オン");
        assert_eq!(thermal_text(&s), "熱状態: やや高い");
        assert_eq!(status_text(&s), POLLING_STATUS);
    }

    #[test]
    fn unavailable_differs_from_success() {
        let mut s = PowerStateShowState::default();
        finish_read(&mut s, Err(PowerReadError::Unavailable));
        assert_eq!(status_text(&s), UNAVAILABLE_STATUS);
        assert_ne!(status_text(&s), POLLING_STATUS);
    }

    #[test]
    fn failed_shows_prefix_and_no_level() {
        let mut s = PowerStateShowState::default();
        finish_read(&mut s, Err(PowerReadError::Failed("x".into())));
        assert_eq!(error_text(&s), format!("{FAILED_PREFIX}x"));
        assert!(battery_text(&s).ends_with(UNKNOWN_TEXT));
    }

    #[test]
    fn error_after_read_drops_previous_value() {
        let mut s = read_state(reading(BatteryView::Present {
            level: Some(80),
            charging: ChargingView::Full,
        }));
        finish_read(&mut s, Err(PowerReadError::Failed("x".into())));
        assert!(!battery_text(&s).contains("80"));
        assert!(thermal_text(&s).ends_with(UNKNOWN_TEXT));
        assert!(matches!(view(&s), PowerView::Failed(_)));
    }

    #[test]
    fn headless_render_lists_four_items() {
        let s = read_state(reading(BatteryView::Absent));
        let lines = render_headless(&power_state_show_screen(), &s);
        for key in ["電池残量", "充電", "省電力モード", "熱状態"] {
            assert!(lines.iter().any(|l| l.starts_with(key)), "{key}");
        }
    }
}
