//! Phase 11 の拡充ループ（親 #1141、Issue #1360）: アプリアイコンのバッジ数（Windows はタスクバー
//! 進捗 %）を固定値に設定し、0 で消す契約。`fandhe-egui-proxy` の `device/app_badge.rs` が
//! 「設定する」「消す」ボタン押下時に [`AppBadge::set`] を呼ぶ。macOS（`NSDockTile.badgeLabel`）と
//! Windows（`ITaskbarList3`）が実装し、iOS（#1361）・Android（#1362）は後続、Web（#1363）は
//! `fandhe-web-dom` 側で Badging API を使い本トレイトを経由しない。
//!
//! # 値
//!
//! [`BadgeValue`] は 0〜[`MAX_BADGE`]。**0 は「消す」で番兵ではない**（iOS / Android でも権限
//! ダイアログを出さない。Windows は `SetOverlayIcon` でオーバーレイを外す）。上限を超える値は
//! [`BadgeValue::new`] が入力不正を返す。範囲検証は OS 型へ変換する前に行う。
//!
//! # 結果の三値（契約表 10）
//!
//! - `Ok(Submitted)`: OS へ渡した。**表示されたかは確かめられない**
//! - `Ok(Cancelled)`: iOS で許可が未決定のまま閉じたときだけ。Android・デスクトップは返さない
//! - `Err`: 許可拒否 → `PermissionDenied("app_badge")`、API が無い（Android API 24〜25・Web の
//!   `setAppBadge` 未定義・Linux）→ `Unsupported("app_badge")`、OS 呼び出しの失敗 →
//!   `Internal("app_badge set")`。`Disabled` は使わない。OS のエラー文言はログにも画面にも出さない
//!
//! 初期化に失敗した実装は `Unsupported` か `Internal` を返し、ダミーの成功へ倒さない。
//!
//! # OS ごとの実態
//!
//! Android のバッジは無音の通知 1 件に付くドットと長押しメニューの件数で、アイコン上の数字は
//! 保証しない。Web は PWA としてインストールしたときだけ表示される。Windows は `SetOverlayIcon` で件数
//! （0〜[`MAX_BADGE`]）を数字入りアイコンとしてタスクバーボタンに重ねる。タスクバー進捗
//! （`SetProgressValue`）は本トレイトの範囲外で、`set` は進捗を触らない（併用しない）。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の要求と完了の順序、嘘の成功の禁止、資源の解放。
//! 守らない: 設定後に OS や利用者がバッジを消したときの画面表示値との食い違い、アプリ終了時に
//! バッジが残るか消えるか（終了時に消す処理は持たない）。並行経路は無い（モバイルは
//! `RequestSlot` 1 本、デスクトップは UI スレッドで同期、Web は世代）。

use crate::DeviceError;

/// バッジ数の上限。
pub const MAX_BADGE: u32 = 9999;

/// 検証済みのバッジ値（0〜[`MAX_BADGE`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadgeValue(u32);

impl BadgeValue {
    /// 上限を超える値は `InvalidInput`。OS 型へ変換する前の唯一の検証点。
    pub fn new(value: u32) -> Result<Self, DeviceError> {
        if value > MAX_BADGE {
            return Err(DeviceError::InvalidInput("app_badge value"));
        }
        Ok(Self(value))
    }

    /// 値。
    pub fn get(self) -> u32 {
        self.0
    }

    /// 0（消去）か。
    pub fn is_clear(self) -> bool {
        self.0 == 0
    }
}

/// [`AppBadge::set`] の成功の種類。「表示された」を匂わせない名前にしている。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeOutcome {
    /// OS へ渡した。
    Submitted,
    /// 利用者が許可の確認を閉じた（iOS のみ）。
    Cancelled,
}

/// バッジを設定する。デスクトップでは UI スレッドから同期で、ボタン押下ごとに 1 回呼ばれる
/// （モバイルは非同期の橋渡し側で扱うため本トレイトを経由しない）。
pub trait AppBadge: Send + Sync {
    /// 値を設定する（0 は消去）。契約は本モジュール doc。
    fn set(&self, value: BadgeValue) -> Result<BadgeOutcome, DeviceError>;
}

/// 非対応環境（Linux 等）用。常に `Unsupported`。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedAppBadge;

impl AppBadge for UnsupportedAppBadge {
    fn set(&self, _value: BadgeValue) -> Result<BadgeOutcome, DeviceError> {
        Err(DeviceError::Unsupported("app_badge"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_range() {
        assert!(BadgeValue::new(9999).is_ok());
        assert!(matches!(
            BadgeValue::new(10000),
            Err(DeviceError::InvalidInput("app_badge value"))
        ));
        assert!(BadgeValue::new(0).unwrap().is_clear());
        assert!(!BadgeValue::new(1).unwrap().is_clear());
    }

    #[test]
    fn unsupported_never_succeeds() {
        for v in [0, 5] {
            assert!(matches!(
                UnsupportedAppBadge.set(BadgeValue::new(v).unwrap()),
                Err(DeviceError::Unsupported("app_badge"))
            ));
        }
    }
}
