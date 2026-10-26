//! Phase 11 の拡充ループ（親 #177、Issue #870）: アプリの窓がある画面 1 枚の表示能力（今の
//! リフレッシュレート・最大のリフレッシュレート・HDR 対応・広色域）を 1 回読む契約。
//! `fandhe-egui-proxy` の `device/display_refresh_hdr.rs` が「画面の表示能力を読む」ボタン押下時に
//! [`DisplayRefreshHdr::read`] を 1 回だけ呼び、各 OS 実装（macOS / Windows: #870、iOS: #871、
//! Android: #872）が本トレイトを実装する。Web は `fandhe-web-dom` 側（#873）で `matchMedia` を使い、
//! 本トレイトは経由しない。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 読めた: [`DisplayCapabilities`]。項目ごとに値か「取れない」（`None`）。HDR「いいえ」は
//!   `Some(false)` で `None` と区別する
//! - 非対応: `Err(DeviceError::Unsupported("display_refresh_hdr"))`
//! - 失敗: `Err(DeviceError::Internal("display_refresh_hdr read"))`（OS 呼び出しの失敗・窓が無い・
//!   全項目が取れなかった場合。[`require_any`]）
//!
//! キャンセル・権限ダイアログの経路は無い（権限不要の同期読み取りのみ）。
//!
//! # 呼び出し契約
//!
//! UI スレッドから同期で、ボタン押下ごとに 1 回だけ呼ぶ。定期 pull・変化通知はしない。前回値や
//! アプリ内の記憶は出さず、毎回 OS から読み直す（OS を正にする）。
//!
//! # 番兵の扱い
//!
//! Hz の 0・負・非有限は「取れない」（[`hz_from_raw`]、検証は丸めの前）。Windows の
//! `dmDisplayFrequency` の 1 も「取れない」。headroom が 1.0 以下なら HDR は「いいえ」
//! （[`hdr_from_headroom`]）。
//!
//! # 取らないもの
//!
//! 個体識別子（EDID・`DeviceName` のパス・`NSScreenNumber`）は画面の照合に内部で使うだけで、
//! 出さず・保持せず・ログに残さない。
//!
//! # OS ごとの写像（`docs/device/capabilities/display-refresh-hdr.json` の api が SSOT）
//!
//! - macOS: 最大 Hz `maximumFramesPerSecond`（12.0+）、今の Hz `CGDisplayModeGetRefreshRate`、
//!   HDR `maximumPotentialExtendedDynamicRangeColorComponentValue`、広色域 `canRepresentDisplayGamut:`
//! - Windows: Hz は `EnumDisplaySettingsW`、HDR は `IDXGIOutput6` の色空間が G2084 のときだけ
//!   `Some(true)`（オフと非対応は区別できないため `None`）、広色域は常に `None`
//! - iOS（#871）: 今の Hz は常に `None`、HDR は `potentialEDRHeadroom`、広色域は `displayGamut`
//!   （`.unspecified` は `None`）
//! - Android（#872）: `Display` の Mode と `isHdr`、広色域は API 26+ のみ
//! - Web（#873）: 本トレイトを経由せず `matchMedia`。Hz は `None`
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 嘘の値を出さない（番兵・失敗・非対応を成功値へ畳まない）、1 プロセス内の取得、
//! 古い世代の完了を捨てる（画面側）。
//! 守らない: 読んだ後の変化の追従、窓が複数画面にまたがるときの画面の選び方、VRR / ProMotion の
//! 瞬間値、OS 間で「HDR 対応」「広色域」の意味が揃うこと、複数タブ・複数プロセス、OS 呼び出しが
//! 戻らない場合の期限。
//!
//! 並行性: 読み取りは UI スレッドの effect 1 本だけで、ワーカー・コールバック・購読を持たない。
//! 集約すべき並行経路は無い。

use crate::DeviceError;

/// 画面の表示能力。`None` は「取れなかった」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisplayCapabilities {
    /// 今のリフレッシュレート（Hz、整数に丸めた値）。
    pub current_hz: Option<u32>,
    /// 最大のリフレッシュレート（Hz）。
    pub max_hz: Option<u32>,
    /// HDR 対応。`Some(false)` は「いいえ」。
    pub hdr: Option<bool>,
    /// 広色域（P3）を表せるか。
    pub wide_gamut: Option<bool>,
}

/// 画面の表示能力を 1 回読む。
pub trait DisplayRefreshHdr: Send + Sync {
    /// 契約はモジュール doc 参照。
    fn read(&self) -> Result<DisplayCapabilities, DeviceError>;
}

/// 非対応 OS 向けの実装。常に `Unsupported` を返し、成功値へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedDisplayRefreshHdr;

impl DisplayRefreshHdr for UnsupportedDisplayRefreshHdr {
    fn read(&self) -> Result<DisplayCapabilities, DeviceError> {
        Err(DeviceError::Unsupported("display_refresh_hdr"))
    }
}

/// OS が返した Hz の生値を検証して整数にする。非有限・0 以下・丸めて 0 になる値は `None`。
/// 検証は丸めの前に行う。全 OS 実装が共用する。
pub fn hz_from_raw(raw: f64) -> Option<u32> {
    if !raw.is_finite() || raw <= 0.0 || raw >= f64::from(u32::MAX) {
        return None;
    }
    let rounded = raw.round();
    (rounded >= 1.0).then_some(rounded as u32)
}

/// EDR の headroom から HDR 対応を決める。1.0 超なら対応、1.0 以下は「いいえ」、非有限は `None`。
pub fn hdr_from_headroom(headroom: f64) -> Option<bool> {
    headroom.is_finite().then_some(headroom > 1.0)
}

/// 4 項目すべて `None` なら `Internal`（嘘の成功の禁止）。
pub fn require_any(c: DisplayCapabilities) -> Result<DisplayCapabilities, DeviceError> {
    if c == DisplayCapabilities::default() {
        return Err(DeviceError::Internal("display_refresh_hdr read"));
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedDisplayRefreshHdr.read(),
            Err(DeviceError::Unsupported("display_refresh_hdr"))
        ));
    }

    #[test]
    fn hz_sentinels_are_none() {
        for raw in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.4] {
            assert_eq!(hz_from_raw(raw), None, "{raw}");
        }
        assert_eq!(hz_from_raw(59.94), Some(60));
        assert_eq!(hz_from_raw(120.0), Some(120));
        assert_eq!(hz_from_raw(1e20), None);
    }

    #[test]
    fn headroom_threshold() {
        assert_eq!(hdr_from_headroom(1.0), Some(false));
        assert_eq!(hdr_from_headroom(0.0), Some(false));
        assert_eq!(hdr_from_headroom(1.5), Some(true));
        assert_eq!(hdr_from_headroom(f64::NAN), None);
        assert_eq!(hdr_from_headroom(f64::INFINITY), None);
    }

    #[test]
    fn require_any_rejects_all_none_only() {
        assert!(matches!(
            require_any(DisplayCapabilities::default()),
            Err(DeviceError::Internal("display_refresh_hdr read"))
        ));
        let no = DisplayCapabilities {
            hdr: Some(false),
            ..Default::default()
        };
        assert!(require_any(no).is_ok());
    }
}
