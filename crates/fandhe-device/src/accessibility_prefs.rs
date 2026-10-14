//! アクセシビリティ設定（視差軽減・コントラスト増加）の契約（Issue #1308、親 #1132）。
//! 2 つの OS 設定を画面へ反映するための橋渡しで、`fandhe-egui-proxy` の
//! `device/accessibility_prefs.rs` が間隔つきで [`AccessibilityPrefsSource::read`] を呼ぶ
//! （pull 型。OS の変化通知は購読しない）。Web は `fandhe-web-dom` が `matchMedia` を読む。
//!
//! # 契約
//!
//! - 結果は [`AccessibilityPrefs`]（2 設定を 1 回の読み取りで返す）か `DeviceError`
//!   （`Unsupported` / `Internal` のみ。新しい失敗 enum は作らない）
//! - 各設定の `None` は「この OS・この版では取得できない（不明）」で、「オフ」とは偽らない
//!   （`device-bridge-contracts.md`「6. 嘘の成功」）。失敗・非対応も「オン」「オフ」にしない
//! - 意味の写像: 視差軽減 = アニメーションを減らす設定がオン、コントラスト = 高コントラスト
//!   （増加）がオン。OS 間で完全には一致せず近似になる（Windows は `AnimationsEnabled` の逆と
//!   ハイコントラストテーマ、Android はアニメ倍率 0 を視差軽減とみなす）
//! - 「即時反映」は最大 [`READ_INTERVAL`] + 1 フレームで反映される、と読み替える
//! - **三値**（契約表 10）: 本機能には利用者操作のキャンセル・権限ダイアログ・入力が無い
//!   （キャンセル・権限拒否は該当なし）。結果は「読めた」「非対応（`Unsupported`）」
//!   「失敗（`Internal`）」の 3 種
//! - 値・OS の生コード（HRESULT 等）をログへ出さない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 嘘の「オフ」「オン」を出さないこと、1 プロセス内での取得、`Some(true)` のあいだ
//! 動きを止めること。守らない: 反映の遅れ（間隔 + 1 フレーム）、OS 間の意味の一致、
//! 複数タブ・複数プロセス、バッテリーセーバー等の別経路によるアニメ無効化、
//! 「不明」のときに動きが止まること。

use std::time::Duration;

use crate::DeviceError;

/// 画面側が OS を読み直す間隔。`fandhe-egui-proxy` と `fandhe-web-dom` が共有する唯一の値。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);

/// ある時点のアクセシビリティ設定。`None` は取得できない（不明）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccessibilityPrefs {
    /// 視差軽減（動きを減らす）がオンか。
    pub reduce_motion: Option<bool>,
    /// コントラスト増加がオンか。
    pub increase_contrast: Option<bool>,
}

/// 設定を 1 回読む。UI スレッドで同期に呼ばれるため短時間で戻ること。
pub trait AccessibilityPrefsSource: Send + Sync {
    /// 現在の設定を返す。失敗後に直前の値を返し続けてはならない。
    fn read(&self) -> Result<AccessibilityPrefs, DeviceError>;
}

/// 常に非対応を返す実装（未実装 OS の dispatch 先・スタブ）。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedAccessibilityPrefs;

impl AccessibilityPrefsSource for UnsupportedAccessibilityPrefs {
    fn read(&self) -> Result<AccessibilityPrefs, DeviceError> {
        Err(DeviceError::Unsupported("accessibility_prefs"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        let r = UnsupportedAccessibilityPrefs.read();
        assert!(matches!(
            r,
            Err(DeviceError::Unsupported("accessibility_prefs"))
        ));
    }
}
