//! iOS 向けアクセシビリティ設定実装（Issue #1309、親 #1132。#1308 のスタブを置き換えた）。
//! `device/accessibility_prefs.rs::implementation` の iOS 経路から呼ばれ、macOS と共用の
//! `apple_accessibility_prefs::AppleAccessibilityPrefs` を返す。`.m`
//! （`apple/fandhe_apple_accessibility_prefs.m`）の iOS 分岐が
//! `UIAccessibilityIsReduceMotionEnabled` / `UIAccessibilityDarkerSystemColorsEnabled`
//! （コントラストは「コントラストを上げる」設定で近似）を読む。UIKit は main スレッド前提のため、
//! `.m` の先頭で main 以外を失敗（負値）にし、Rust 側で `Internal` に写す。
//!
//! # 再描画
//!
//! OS コールバックを持たず結果を共有状態へ書く経路が無いため、`crate::ios::request_repaint` は
//! 呼ばない。再描画は `drive` の完了通知と 1 秒間隔の再描画予約が起こす。
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 1 プロセス内の取得、main 以外・不正値を「オフ」へ畳まないこと
//! - 守らない: 反映の遅れ（最大 1 秒 + 1 フレーム）、OS 間の意味の一致、複数プロセス
//!
//! 変化通知は購読しない（trait 契約の変更で先行 docs PR が要る）。実機確認は #1302。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::accessibility_prefs::AccessibilityPrefsSource;

/// iOS の実装を返す。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn AccessibilityPrefsSource> {
    Arc::new(super::apple_accessibility_prefs::AppleAccessibilityPrefs)
}
