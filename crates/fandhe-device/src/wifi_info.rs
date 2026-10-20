//! Wi-Fi 情報（今つないでいる Wi-Fi の名前と電波強度、OS が持つ直近のスキャン結果の上位数件）の抽象
//! （Phase 11 の拡充ループ、対応表 `docs/device/capabilities/wifi-info.json`、親 #111、Issue #514）。
//!
//! 扱う範囲は「ボタンを押すと現在の接続の SSID と dBm、周辺ネットワークの上位 [`MAX_NEARBY`] 件を
//! 読む」ことだけ。**自分からスキャンを起こさない**・接続操作・BSSID は扱わない。
//! 対象は macOS（#514）・Windows・Android。iOS は対応表で `deferred`、Web は `no`。
//! `fandhe-egui-proxy` の `device/wifi_info.rs` がワーカースレッドで [`WifiInfo::read`] を呼ぶ
//! （ブロッキングの契約）。全 OS が [`Ssid::from_bytes`]・[`rssi_from_dbm`]・[`normalize_nearby`]
//! を通して整形する。
//!
//! # 契約
//!
//! - 結果は成功か [`DeviceError`]。種類は次の 5 つ
//!
//! | `DeviceError` | 意味 |
//! | --- | --- |
//! | `Unsupported(`[`FEATURE`]`)` | 端末に Wi-Fi アダプターが無い・OS が非対応 |
//! | `PermissionDenied(`[`FEATURE`]`)` | 位置情報の許可が無い（SSID が読めない） |
//! | `Disabled(`[`RADIO_OFF`]`)` | Wi-Fi の電源がオフ |
//! | `Disabled(`[`LOCATION_OFF`]`)` | 位置情報サービスがオフ |
//! | `Internal(`[`READ_FAILED`]`)` | 上記以外の OS 呼び出し失敗（壊れた値を含む） |
//!
//! - **成功は 0 件を含む**。未接続（`current == None`）かつ周辺 0 件は正当な結果。一方、位置情報
//!   サービスオフ・許可なしで OS が空を返すのは嘘の成功になるため、実装は読む前に判定して
//!   上の失敗を返す（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **三値**: 利用者が取り消す UI は無い。許可ダイアログでの拒否は `PermissionDenied` で、
//!   成功にもキャンセルにもせず失敗として表示する（類型 10）
//! - SSID は位置を推定できる情報として扱う。[`Ssid`] の `Debug` は伏せ字で `Display` は無い。
//!   SSID・OS 固有のエラー値をログ・エラー・`Debug` に出さない（類型 8）
//! - 電波強度は [`rssi_from_dbm`] の値域（`-126..=-1` dBm）だけを `Some` にする。OS の番兵値
//!   （macOS の 0、Android の -127 等）は `None`（不明）
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 嘘の成功を出さないこと（0 件と失敗の区別）、資源を漏らさないこと、SSID を件数以外で
//! 出力しないこと。これらは守らない範囲にできない。
//!
//! 守らない（表示が一時的に食い違うだけのもの）:
//! - スキャン結果の鮮度と dBm の揺れ（OS が持つ直近のキャッシュをそのまま使う）
//! - 読んだ後の変化と、複数プロセスから同時に読んだときの差
//! - 許可ダイアログの応答待ちの期限（読み取りは同期で、期限を持たない）
//! - Windows で初回の許可ダイアログの後に押し直しが要ること
//! - OS 呼び出しが戻らない場合
//! - 非 UTF-8 の SSID（`None` として「非公開」と同じ表示になる）

use crate::DeviceError;
use crate::device_info::clean_text;

/// 周辺ネットワークとして返す上限。
pub const MAX_NEARBY: usize = 5;
/// `Unsupported` / `PermissionDenied` の機能名。
pub const FEATURE: &str = "wifi_info";
/// `Disabled` の識別子（Wi-Fi の電源オフ）。
pub const RADIO_OFF: &str = "wifi_info radio";
/// `Disabled` の識別子（位置情報サービスオフ）。`device/wifi_info.rs` がこの値と照合する。
pub const LOCATION_OFF: &str = "wifi_info location";
/// `Internal` の識別子。
pub const READ_FAILED: &str = "wifi_info read";

/// Wi-Fi の名前。位置を推定できるため `Debug` は伏せ字、`Display` は実装しない。
/// 生成経路は [`Ssid::from_bytes`] のみ。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ssid {
    /// 同一性判定用の生の文字列（整形しない。`home` と ` home ` は別の SSID）。
    raw: String,
    /// 表示用（制御文字・前後の空白を除いたもの）。
    shown: String,
}

impl std::fmt::Debug for Ssid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ssid(<redacted>)")
    }
}

impl Ssid {
    /// OS の生バイトから作る。UTF-8 でない・制御文字を除くと空なら `None`。
    pub fn from_bytes(raw: &[u8]) -> Option<Ssid> {
        let s = std::str::from_utf8(raw).ok()?;
        let shown = clean_text(s)?;
        Some(Ssid {
            raw: s.to_string(),
            shown,
        })
    }

    /// 表示用の文字列。画面への受け渡し以外で使わない。
    pub fn expose(&self) -> &str {
        &self.shown
    }
}

/// ネットワーク 1 件。`ssid` は非公開・取得不可で `None`、`rssi_dbm` は不明で `None`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiNetwork {
    /// 名前。
    pub ssid: Option<Ssid>,
    /// 電波強度（dBm、`-126..=-1`）。
    pub rssi_dbm: Option<i16>,
}

/// 読み取り結果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WifiReport {
    /// 現在の接続。未接続は `None`。
    pub current: Option<WifiNetwork>,
    /// 周辺（強い順、[`MAX_NEARBY`] 件まで）。
    pub nearby: Vec<WifiNetwork>,
}

/// Wi-Fi 情報読み取りの契約（モジュール doc の「契約」節）。
pub trait WifiInfo: Send + Sync {
    /// 現在の接続と周辺を読む。呼び出し側がワーカースレッドで呼ぶブロッキング呼び出し。
    fn read(&self) -> Result<WifiReport, DeviceError>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedWifiInfo;

impl WifiInfo for UnsupportedWifiInfo {
    fn read(&self) -> Result<WifiReport, DeviceError> {
        Err(DeviceError::Unsupported(FEATURE))
    }
}

/// OS の電波強度を契約の値域へ揃える。`-126..=-1` 以外（0・-127・正の値）は `None`。
pub fn rssi_from_dbm(raw: i32) -> Option<i16> {
    (-126..=-1).contains(&raw).then_some(raw as i16)
}

/// 周辺のネットワークを SSID ごとに最強の dBm へまとめ、dBm の降順（不明は末尾、同順位は SSID の
/// 昇順で `None` は末尾）に並べて先頭 [`MAX_NEARBY`] 件を返す。`None` 同士は 1 件にまとめる。
pub fn normalize_nearby(raw: impl IntoIterator<Item = WifiNetwork>) -> Vec<WifiNetwork> {
    let mut merged: Vec<WifiNetwork> = Vec::new();
    for n in raw {
        match merged.iter_mut().find(|m| m.ssid == n.ssid) {
            Some(m) => m.rssi_dbm = m.rssi_dbm.max(n.rssi_dbm),
            None => merged.push(n),
        }
    }
    // `Option` の順序は `None < Some`。dBm は大きいほど強いので降順で不明が末尾になる。
    merged.sort_by(|a, b| {
        b.rssi_dbm
            .cmp(&a.rssi_dbm)
            .then_with(|| match (&a.ssid, &b.ssid) {
                (Some(x), Some(y)) => x.cmp(y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
    });
    merged.truncate(MAX_NEARBY);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(name: Option<&str>, rssi: Option<i16>) -> WifiNetwork {
        WifiNetwork {
            ssid: name.and_then(|s| Ssid::from_bytes(s.as_bytes())),
            rssi_dbm: rssi,
        }
    }

    #[test]
    fn many_aps_of_one_ssid_do_not_hide_others() {
        let mut raw: Vec<_> = (0..64).map(|_| net(Some("A"), Some(-40))).collect();
        raw.extend(["B", "C", "D", "E"].map(|s| net(Some(s), Some(-50))));
        assert_eq!(normalize_nearby(raw).len(), 5);
    }

    #[test]
    fn many_control_only_ssids_collapse_before_cap() {
        // 制御文字のみの名前は `None` へ変換され 1 件に集約される。後続の通常 SSID を隠さない。
        let mut raw: Vec<_> = (0..64u8)
            .map(|i| net(Some(&format!("\u{1}{}", char::from(i % 2))), Some(-40)))
            .collect();
        raw.extend(["B", "C", "D", "E"].map(|s| net(Some(s), Some(-50))));
        assert_eq!(normalize_nearby(raw).len(), 5);
    }

    #[test]
    fn unsupported_is_error_not_empty_success() {
        assert!(matches!(
            UnsupportedWifiInfo.read(),
            Err(DeviceError::Unsupported(FEATURE))
        ));
    }

    #[test]
    fn ssid_from_bytes_rejects_bad_input() {
        assert_eq!(Ssid::from_bytes(&[0xff, 0xfe]), None);
        assert_eq!(Ssid::from_bytes(b""), None);
        assert_eq!(Ssid::from_bytes(b"  \t "), None);
        assert_eq!(Ssid::from_bytes(b"\x01\x02"), None);
        assert_eq!(Ssid::from_bytes(b"a\x01b").unwrap().expose(), "ab");
    }

    #[test]
    fn debug_never_shows_ssid() {
        let r = WifiReport {
            current: Some(net(Some("secret-home"), Some(-50))),
            nearby: vec![net(Some("neighbor-x"), Some(-70))],
        };
        let out = format!("{r:?}");
        assert!(
            !out.contains("secret") && !out.contains("neighbor"),
            "{out}"
        );
    }

    #[test]
    fn rssi_range_edges() {
        for bad in [0, -127, 1, i32::MIN, i32::MAX] {
            assert_eq!(rssi_from_dbm(bad), None, "{bad}");
        }
        assert_eq!(rssi_from_dbm(-1), Some(-1));
        assert_eq!(rssi_from_dbm(-126), Some(-126));
    }

    #[test]
    fn nearby_merges_sorts_and_caps() {
        let out = normalize_nearby([
            net(Some("b"), Some(-60)),
            net(Some("a"), Some(-60)),
            net(Some("b"), Some(-40)),
            net(None, None),
            net(None, Some(-80)),
            net(Some("c"), None),
            net(Some("d"), Some(-90)),
            net(Some("e"), Some(-95)),
        ]);
        assert_eq!(out.len(), MAX_NEARBY);
        let names: Vec<_> = out
            .iter()
            .map(|n| n.ssid.as_ref().map(|s| s.expose().to_string()))
            .collect();
        // b(-40) a(-60) None(-80) d(-90) e(-95)。c(不明) は上限で落ちる。
        assert_eq!(
            names,
            vec![
                Some("b".into()),
                Some("a".into()),
                None,
                Some("d".into()),
                Some("e".into())
            ]
        );
    }

    #[test]
    fn nearby_keeps_ssids_differing_only_by_padding() {
        let out = normalize_nearby([net(Some("home"), Some(-50)), net(Some(" home "), Some(-60))]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn nearby_empty_is_empty() {
        assert!(normalize_nearby([]).is_empty());
    }
}
