//! Phase 11 の拡充ループ（親 #1130）型サンプル「ネットワーク接続状態」（Issue #1282）。
//! `FANDHE_PROXY_APP=network_status_show` で選ばれ、`fandhe-egui-proxy` の
//! `screens/registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の
//! `pub mod network_status_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。
//! OS 値の取得は `fandhe-egui-proxy` の `device/network_status.rs`（Web は `fandhe-web-dom` の
//! `network_status.rs`）が一定間隔で行い、
//! [`apply_read`] で結果を注入する。`fandhe_device::network_status` の型は
//! 依存方向の都合でここに写し取る。
//!
//! 嘘の「接続あり」を出さない: 初期状態・不明・非対応・失敗は接続ありにならず、
//! `Online` 以外へ遷移したときは種別・従量制を「不明」へ戻す（最後の値を残さない）。
//! 「接続あり」は通信経路があるという意味でインターネット到達性は保証しない（注記に明記）。

use crate::Screen;

/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 接続種別（`fandhe_device::network_status::ConnectionKind` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindView {
    /// Wi-Fi。
    Wifi,
    /// 携帯回線。
    Cellular,
    /// 有線。
    Ethernet,
    /// その他。
    Other,
    /// 不明。
    Unknown,
}

/// OS から読めた値（`NetworkSnapshot` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// 接続あり。
    Online {
        /// 種別。
        kind: KindView,
        /// 従量制か（`None` は不明）。
        metered: Option<bool>,
    },
    /// 接続なし。
    Offline,
    /// 不明。
    Unknown,
}

/// 読めなかった理由。`DeviceError` からの写像は呼び出し元（proxy）の責務。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// この環境では読めない。
    Unsupported,
    /// 内部エラー（固定文言のみ。OS 固有値を持たない）。
    Failed(String),
}

/// 画面に出す接続状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NetworkView {
    /// 最初の値が届く前。
    #[default]
    Reading,
    /// 接続あり。
    Online {
        /// 種別。
        kind: KindView,
        /// 従量制か。
        metered: Option<bool>,
    },
    /// 接続なし。
    Offline,
    /// 不明。
    Unknown,
    /// 非対応。
    Unsupported,
    /// 失敗。
    Failed(String),
}

/// 「ネットワーク接続状態」画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkStatusShowState {
    /// 現在の表示状態。
    pub view: NetworkView,
}

/// 読み取り結果を反映する。毎回 view を置き換えるため、`Online` から他へ変わると
/// 種別・従量制は残らない。
pub fn apply_read(state: &mut NetworkStatusShowState, result: Result<Reading, ReadFailure>) {
    state.view = match result {
        Ok(Reading::Online { kind, metered }) => NetworkView::Online { kind, metered },
        Ok(Reading::Offline) => NetworkView::Offline,
        Ok(Reading::Unknown) => NetworkView::Unknown,
        Err(ReadFailure::Unsupported) => NetworkView::Unsupported,
        Err(ReadFailure::Failed(m)) => NetworkView::Failed(m),
    };
}

/// 接続の文言。
pub fn connection_text(state: &NetworkStatusShowState) -> String {
    let s = match &state.view {
        NetworkView::Reading => "接続: 読み取り中",
        NetworkView::Online { .. } => "接続: 接続あり",
        NetworkView::Offline => "接続: 接続なし",
        NetworkView::Unknown | NetworkView::Failed(_) => "接続: 不明",
        NetworkView::Unsupported => "接続: この環境では接続状態を読めません",
    };
    s.to_string()
}

/// 種別の文言（`Online` 以外は「不明」）。
pub fn kind_text(state: &NetworkStatusShowState) -> String {
    let k = match &state.view {
        NetworkView::Online { kind, .. } => match kind {
            KindView::Wifi => "Wi-Fi",
            KindView::Cellular => "携帯回線",
            KindView::Ethernet => "有線",
            KindView::Other => "その他",
            KindView::Unknown => "不明",
        },
        _ => "不明",
    };
    format!("種別: {k}")
}

/// 従量制の文言（`Online` 以外・取得不可は「不明」）。
pub fn metered_text(state: &NetworkStatusShowState) -> String {
    let m = match &state.view {
        NetworkView::Online {
            metered: Some(true),
            ..
        } => "はい",
        NetworkView::Online {
            metered: Some(false),
            ..
        } => "いいえ",
        _ => "不明",
    };
    format!("従量制: {m}")
}

/// 失敗時のみ非空。
pub fn error_text(state: &NetworkStatusShowState) -> String {
    match &state.view {
        NetworkView::Failed(m) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

/// 注記（「接続あり」の意味と種別が取れない環境の扱い）。
pub const NOTE: &str = "「接続あり」は通信経路があるという意味で、インターネットに届くことは保証しません。種別が取れない環境では「不明」と表示します。";

/// 「ネットワーク接続状態」画面。
pub fn network_status_show_screen() -> Screen<NetworkStatusShowState> {
    Screen::new("ネットワーク接続状態")
        .dynamic_text(connection_text)
        .dynamic_text(kind_text)
        .dynamic_text(metered_text)
        .error_text(error_text)
        .text(NOTE)
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。
pub fn current_screen(_state: &NetworkStatusShowState) -> Screen<NetworkStatusShowState> {
    network_status_show_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    fn online() -> Reading {
        Reading::Online {
            kind: KindView::Wifi,
            metered: Some(true),
        }
    }

    #[test]
    fn initial_is_not_online() {
        let s = NetworkStatusShowState::default();
        assert_eq!(s.view, NetworkView::Reading);
        assert!(!connection_text(&s).contains("接続あり"));
        assert!(connection_text(&s).contains("読み取り中"));
    }

    #[test]
    fn online_shows_kind_and_metered() {
        let mut s = NetworkStatusShowState::default();
        apply_read(&mut s, Ok(online()));
        assert_eq!(connection_text(&s), "接続: 接続あり");
        assert_eq!(kind_text(&s), "種別: Wi-Fi");
        assert_eq!(metered_text(&s), "従量制: はい");
        apply_read(
            &mut s,
            Ok(Reading::Online {
                kind: KindView::Unknown,
                metered: None,
            }),
        );
        assert_eq!(kind_text(&s), "種別: 不明");
        assert_eq!(metered_text(&s), "従量制: 不明");
    }

    #[test]
    fn leaving_online_drops_kind_metered_and_online() {
        let others = [
            Ok(Reading::Offline),
            Ok(Reading::Unknown),
            Err(ReadFailure::Unsupported),
            Err(ReadFailure::Failed("接続状態の取得".to_string())),
        ];
        for r in others {
            let mut s = NetworkStatusShowState::default();
            apply_read(&mut s, Ok(online()));
            apply_read(&mut s, r);
            assert!(!connection_text(&s).contains("接続あり"));
            assert_eq!(kind_text(&s), "種別: 不明");
            assert_eq!(metered_text(&s), "従量制: 不明");
        }
    }

    #[test]
    fn failed_sets_error_text() {
        let mut s = NetworkStatusShowState::default();
        apply_read(
            &mut s,
            Err(ReadFailure::Failed("接続状態の取得".to_string())),
        );
        assert_eq!(error_text(&s), "失敗しました: 接続状態の取得");
        apply_read(&mut s, Ok(Reading::Offline));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn render_headless_does_not_panic() {
        let s = NetworkStatusShowState::default();
        let joined = render_headless(&network_status_show_screen(), &s).join("\n");
        assert!(joined.contains("読み取り中"));
        assert!(joined.contains("インターネットに届くことは保証しません"));
    }
}
