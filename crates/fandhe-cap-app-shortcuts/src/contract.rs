//! アプリショートカット（長押し・ジャンプリスト・Dock メニュー）の登録（Phase 11 の拡充ループ、
//! 親 #101、Issue #424）。
//!
//! 扱う範囲は「OS にショートカット 1 件を出す側」だけ。選ばれたときの受信は新しく作らず、
//! 各 OS が URL（ネイティブは `fandhe://shortcut/open-demo`、Web は `web+fandhe://…`）として
//! 起動・転送し、既存の `url_scheme::UrlInbox` に入る。呼び出し元は `fandhe-egui-proxy` の
//! `crates/fandhe-cap-app-shortcuts/src/driver.rs`（画面向け effect ドライバ）と各 OS 実装。
//!
//! # 契約（各 OS 実装・呼び出し元が守る不変条件）
//!
//! - **結果は 4 種**: 宣言済み（[`Registration::Declared`]）・登録済み（[`Registration::Installed`]）・
//!   非対応（`Err(Unsupported)`）・失敗（その他の `Err`）。失敗・権限拒否・入力不正・非対応を
//!   成功へ畳まない（`device-bridge-contracts.md`「6. 嘘の成功」）。スタブは [`UnsupportedAppShortcuts`]
//! - **キャンセルはない**: 登録は利用者操作を伴わない。失敗は 1 回限りで、プロセス内では再試行しない
//!   （同「10. 三値」。[`InstallCell`] が結果を保持する）
//! - **登録は 1 プロセスで 1 回だけ起動する**: 判定と更新は [`InstallCell`] の lock の内側で行う
//! - **エラーの中身をログにも画面にも出さない**
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内で登録を 1 回だけ起動すること、古い完了・重複した完了を捨てること、
//! 嘘の成功を出さないこと。
//! 守らない（表示が食い違うだけで、データ消失・資源漏れ・嘘の成功ではないもの）:
//! 他アプリや他ページが同じ URL を送った場合にショートカットからの起動と区別すること
//! （url-scheme と同じ信頼境界）、Dock メニューが起動中だけ出ること、Android と Windows で
//! 画面を一度開くまでショートカットが出ないこと、Windows のタスクが終了後も残ること、
//! ポーリング間隔ぶんの表示遅れ。

use std::sync::{Mutex, MutexGuard};

use crate::error::AppShortcutsError;

/// 登録に成功したときの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registration {
    /// 宣言ファイルで OS に任せた。表示されたかは確かめられない（iOS・Web）。
    Declared,
    /// OS の登録 API が成功を返した（macOS・Android・Windows）。
    Installed,
}

/// 登録するショートカット 1 件の仕様。呼び出し側（driver・外部アプリ）が自分の値を渡し、契約と OS 実装は
/// 画面の定数を持たない。iOS は Info.plist の静的宣言なので、値は宣言側と一致させる（照合は呼び出し側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortcutSpec<'a> {
    /// ランチャー・Dock・ジャンプリストに出すラベル。
    pub title: &'a str,
    /// 選ばれたときに起動する URL（url-scheme の受信箱へ入る）。
    pub url: &'a str,
}

/// ショートカットを OS に登録する抽象。各 OS 実装は本 crate が持つ。
pub trait AppShortcuts: Send + Sync {
    /// ショートカット 1 件を登録する。契約はモジュール doc 参照。
    /// iOS は `spec` を使わず宣言済みとして返す（`Registration::Declared`）。
    fn install(&self, spec: &ShortcutSpec<'_>) -> Result<Registration, AppShortcutsError>;
}

/// 非対応 OS 用。常に `Err(Unsupported)` を返し、成功を装わない。
pub struct UnsupportedAppShortcuts;

impl AppShortcuts for UnsupportedAppShortcuts {
    fn install(&self, _spec: &ShortcutSpec<'_>) -> Result<Registration, AppShortcutsError> {
        Err(AppShortcutsError::Unsupported("app_shortcuts"))
    }
}

/// [`InstallCell`] が保持する確定結果（`AppShortcutsError` は `Clone` でないため写像済みの値で持つ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// [`Registration::Declared`]。
    Declared,
    /// [`Registration::Installed`]。
    Installed,
    /// `Err(Unsupported)`。
    Unsupported,
    /// その他の `Err`。
    Failed,
}

impl From<&Result<Registration, AppShortcutsError>> for InstallOutcome {
    fn from(r: &Result<Registration, AppShortcutsError>) -> Self {
        match r {
            Ok(Registration::Declared) => Self::Declared,
            Ok(Registration::Installed) => Self::Installed,
            Err(AppShortcutsError::Unsupported(_)) => Self::Unsupported,
            Err(_) => Self::Failed,
        }
    }
}

#[derive(Clone, Copy)]
enum InstallState {
    NotStarted,
    Running,
    Done(InstallOutcome),
}

/// 登録を 1 プロセスで 1 回だけ起動し、結果を保持する。static に置ける（[`InstallCell::new`] は `const fn`）。
pub struct InstallCell(Mutex<InstallState>);

impl InstallCell {
    /// 未開始の状態で作る。
    pub const fn new() -> Self {
        Self(Mutex::new(InstallState::NotStarted))
    }

    /// poison していても中身を使う（`url_scheme::UrlInbox` と同じ）。
    fn guard(&self) -> MutexGuard<'_, InstallState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 未開始のときだけ実行中へ進めて `true`。判定と更新は同じ lock の内側で行う。
    pub fn begin(&self) -> bool {
        let mut s = self.guard();
        if matches!(*s, InstallState::NotStarted) {
            *s = InstallState::Running;
            true
        } else {
            false
        }
    }

    /// 実行中のときだけ結果を確定する。それ以外（古い完了・重複した完了）は無視する。
    pub fn finish(&self, result: &Result<Registration, AppShortcutsError>) {
        let mut s = self.guard();
        if matches!(*s, InstallState::Running) {
            *s = InstallState::Done(result.into());
        }
    }

    /// 確定済みの結果。実行前・実行中は `None`。
    pub fn get(&self) -> Option<InstallOutcome> {
        match *self.guard() {
            InstallState::Done(o) => Some(o),
            _ => None,
        }
    }
}

impl Default for InstallCell {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_succeeds_once() {
        let c = InstallCell::new();
        assert!(c.begin());
        assert!(!c.begin());
        c.finish(&Ok(Registration::Installed));
        assert!(!c.begin());
    }

    #[test]
    fn finish_before_begin_is_ignored() {
        let c = InstallCell::new();
        c.finish(&Ok(Registration::Installed));
        assert_eq!(c.get(), None);
    }

    #[test]
    fn finish_does_not_overwrite_done() {
        let c = InstallCell::new();
        assert!(c.begin());
        c.finish(&Err(AppShortcutsError::PermissionDenied("app_shortcuts")));
        c.finish(&Ok(Registration::Installed));
        assert_eq!(c.get(), Some(InstallOutcome::Failed));
    }

    #[test]
    fn outcome_mapping_never_turns_errors_into_success() {
        for (r, want) in [
            (Ok(Registration::Declared), InstallOutcome::Declared),
            (Ok(Registration::Installed), InstallOutcome::Installed),
            (
                Err(AppShortcutsError::Unsupported("x")),
                InstallOutcome::Unsupported,
            ),
            (
                Err(AppShortcutsError::InvalidInput("x")),
                InstallOutcome::Failed,
            ),
            (
                Err(AppShortcutsError::PermissionDenied("x")),
                InstallOutcome::Failed,
            ),
            (
                Err(AppShortcutsError::Internal("x")),
                InstallOutcome::Failed,
            ),
        ] {
            assert_eq!(InstallOutcome::from(&r), want);
        }
    }

    #[test]
    fn unsupported_is_never_ok() {
        assert!(matches!(
            UnsupportedAppShortcuts.install(&ShortcutSpec {
                title: "t",
                url: "x://y"
            }),
            Err(AppShortcutsError::Unsupported(_))
        ));
    }
}
