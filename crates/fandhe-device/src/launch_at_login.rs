//! Phase 11 の拡充ループ（親 #134、Issue #696）: ログイン時の自動起動（launch-at-login）の契約。
//! `fandhe-egui-proxy` の `device/launch_at_login.rs`（effect ドライバ）が、画面のボタン操作ごとに
//! [`LaunchAtLogin`] を別スレッドで呼ぶ。macOS（`SMAppService.mainApp`）が実装し、Windows
//! （HKCU の Run キー、Issue #697）は骨格のスタブ（`Unsupported`）から後続の子が埋める。
//! iOS・Android・Web は対象外（対応表 `launch-at-login.json`）。
//!
//! # 結果の種類（契約表 10）
//!
//! - 利用者のキャンセル操作はない。`Cancelled` は返さない
//! - 承認待ち・利用者による無効化は失敗でも成功（有効）でもない第 3 の結果で、
//!   `Ok(RequiresApproval | DisabledByUser)` として返す。[`LoginItemStatus::is_enabled`] は
//!   `Enabled` だけが真
//! - 失敗は `Err`:
//!
//! | 状況 | 返す値 |
//! | --- | --- |
//! | 非対応（macOS 13 未満・.app 外・対象外 OS） | `Unsupported("launch_at_login")` |
//! | OS が拒否した | `PermissionDenied("launch_at_login")` |
//! | 署名不正・その他の OS エラー・未知のコード | `Internal(..)` |
//!
//! # 期限なし（契約表 3）
//!
//! OS 呼び出しは中断できない同期呼び出しのため期限は置かない。処理中の再押下は画面側が無視する。
//! [`LaunchAtLogin::enable`] / [`LaunchAtLogin::disable`] は完了時に OS から読み戻した状態を返す。
//!
//! # 各 OS の実装方針
//!
//! - macOS: `SMAppService.mainApp`。13 未満と .app 外は `Unsupported`
//! - Windows: HKCU の Run キー。値データは引用符付きの実行体パス + `--fandhe-launched-at-login`
//!   （260 文字以内）。StartupApproved は読むだけ。設定画面は `ms-settings:startupapps`
//!
//! # 守る範囲 / 守らない範囲（契約表 11、`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内の操作順、失敗・承認待ち・利用者による無効化を成功と表示しないこと、
//! 未登録の解除のべき等性、別の場所の登録を解除で消さないこと、実行体のパスをログ・エラー文言に
//! 出さないこと（契約表 8）。
//! 守らない: OS 設定で変えた状態が画面に出るまでの時間差、開発中の実行体のパス変更の自動追従、
//! 複数インストール・複数プロセスの同時操作、StartupApproved の非公開形式の変更、
//! ログイン時の起動順・遅延、macOS で起動の由来を判別すること。

use crate::DeviceError;

/// OS から読み戻した自動起動の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItemStatus {
    /// 登録済みで有効（有効扱いはこれだけ）。
    Enabled,
    /// 未登録。
    NotRegistered,
    /// macOS の承認待ち、または利用者がシステム設定で無効化した。
    RequiresApproval,
    /// Windows の StartupApproved が無効（0x03）。
    DisabledByUser,
    /// Windows で同じ値名に別の場所の実行体が登録されている。
    RegisteredElsewhere,
    /// 判別できない（macOS の `notFound`・未知の生値、Windows の StartupApproved の判別不能値）。
    Unknown,
}

impl LoginItemStatus {
    /// ログイン時に実際に起動される状態か。`Enabled` だけが真（嘘の成功を作らない）。
    pub fn is_enabled(self) -> bool {
        self == Self::Enabled
    }
}

/// ログイン時の自動起動を操作する。デスクトップのワーカースレッドから同期で呼ばれる。
pub trait LaunchAtLogin: Send + Sync {
    /// 現在の状態を OS から読む。
    fn status(&self) -> Result<LoginItemStatus, DeviceError>;

    /// 登録し、OS から読み戻した状態を返す。登録済みならべき等に読み戻す。
    fn enable(&self) -> Result<LoginItemStatus, DeviceError>;

    /// 解除し、OS から読み戻した状態を返す。未登録なら OS を呼ばず `Ok(NotRegistered)`。
    /// 別の場所の登録しかなければ消さずに `Ok(RegisteredElsewhere)`（Windows）。
    fn disable(&self) -> Result<LoginItemStatus, DeviceError>;

    /// OS の設定画面を開くよう依頼する。`Ok` は「依頼した」であり、開いたことは確かめられない。
    fn open_settings(&self) -> Result<(), DeviceError>;
}

/// 非対応環境用。全メソッドが `Unsupported`（ダミーの成功を返さない）。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedLaunchAtLogin;

impl LaunchAtLogin for UnsupportedLaunchAtLogin {
    fn status(&self) -> Result<LoginItemStatus, DeviceError> {
        Err(DeviceError::Unsupported("launch_at_login"))
    }
    fn enable(&self) -> Result<LoginItemStatus, DeviceError> {
        Err(DeviceError::Unsupported("launch_at_login"))
    }
    fn disable(&self) -> Result<LoginItemStatus, DeviceError> {
        Err(DeviceError::Unsupported("launch_at_login"))
    }
    fn open_settings(&self) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("launch_at_login"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_enabled_is_enabled() {
        use LoginItemStatus::*;
        assert!(Enabled.is_enabled());
        for s in [
            NotRegistered,
            RequiresApproval,
            DisabledByUser,
            RegisteredElsewhere,
            Unknown,
        ] {
            assert!(!s.is_enabled(), "{s:?}");
        }
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedLaunchAtLogin;
        assert!(matches!(
            u.status(),
            Err(DeviceError::Unsupported("launch_at_login"))
        ));
        assert!(matches!(
            u.enable(),
            Err(DeviceError::Unsupported("launch_at_login"))
        ));
        assert!(matches!(
            u.disable(),
            Err(DeviceError::Unsupported("launch_at_login"))
        ));
        assert!(matches!(
            u.open_settings(),
            Err(DeviceError::Unsupported("launch_at_login"))
        ));
    }
}
