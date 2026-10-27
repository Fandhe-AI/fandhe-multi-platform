//! 無線（Wi-Fi・Bluetooth）の状態取得と切り替え（状態を 1 回読む・オン/オフを 1 回要求する）の抽象
//! （Phase 11 の拡充ループ、対応表 `docs/device/capabilities/radio-control.json`、親 #189、Issue #972）。
//!
//! `fandhe-egui-proxy` の `device/radio_control.rs` がワーカースレッドで [`RadioControl::read`] と
//! [`RadioControl::set`] を呼ぶ（ブロッキングの契約）。対象は macOS（#972）・Windows（#973）・
//! Android（#974）。iOS・Web は対応表で `no`。`network-status`（接続の有無・種別）や `wifi-info`
//! （SSID・スキャン）とは別で、本機能は無線そのものの on/off。
//!
//! # 契約
//!
//! - 読み取りの結果は [`RadioState`]、切り替えの結果は [`SwitchOutcome`]、失敗は [`DeviceError`]。
//!   失敗の写像は次のとおり
//!
//! | 失敗 | 意味 |
//! | --- | --- |
//! | `PermissionDenied(`[`FEATURE`]`)` | 許可がない |
//! | `Internal(`[`DENIED_BY_SYSTEM`]`)` | OS が拒否した（ポリシー等） |
//! | `Unsupported(`[`FEATURE`]`)` | アプリからは切り替えられない・OS が非対応・`NotPresent` の無線への `set` |
//! | `Internal(`[`FAILED`]`)` | 上記以外の失敗（管理者パスワードの取り消しを含む） |
//!
//! - **OS を正にする**。事前判定をしない。切り替えの後の状態は、呼び出し側が読み直した値だけで出す。
//!   [`SwitchOutcome::Accepted`]・[`SwitchOutcome::Opened`]・[`SwitchOutcome::NotChanged`] を
//!   「オン/オフになった」と言わない
//! - [`RadioState::NotPresent`]・[`RadioState::Unknown`]・読み取りの失敗を [`RadioState::Off`] に畳まない
//! - 期限・ポーリング・推測タイマーを持たない。[`RadioState::Transitioning`] でも自動で読み直さない
//! - **三値**（類型 10）: Android の `RESULT_CANCELED` は [`SwitchOutcome::NotChanged`] で、失敗にしない。
//!   macOS の管理者パスワードの取り消しは、識別できる値が見つかるまで [`FAILED`]（Mac 目視 #965）
//! - OS のエラー値（`NSError`・HRESULT）とアダプター名をログ・画面・`Debug` に出さない
//! - 初期化に失敗したときも [`UnsupportedRadioControl`] へ倒し、ダミーの成功へは倒さない
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序（世代照合）、嘘の成功を出さないこと、資源を漏らさないこと、
//! Android の保留中の要求を Activity の再生成で Rust 側と揃えて破棄すること。
//!
//! 守らない（表示が一時的に食い違うだけのもの）:
//! - 読み直しの時点で切り替わりの途中であること
//! - 要求の後にハードウェアスイッチ・ポリシー・他アプリが状態を変えること
//! - OS 呼び出しが戻らない場合
//! - macOS の管理者パスワードのダイアログ、Windows の同意 UI で待つ時間
//! - 複数アダプターで先頭の 1 つだけを扱うこと、複数プロセス
//!
//! # OS ごとの写像
//!
//! - macOS（#972）: Wi-Fi は `CWWiFiClient.interface` の `powerOn` と `setPower:error:`。Bluetooth は
//!   `IOBluetoothHostController.powerState` の読み取りのみで、`set` は `Unsupported`
//!   （切り替えの公開 API なし）
//! - Windows（#973 への約束）: `Radio::GetRadiosAsync` を Kind で絞った先頭 1 つを使い、無ければ
//!   [`RadioState::NotPresent`]。`RequestAccessAsync` の後に `SetStateAsync`。`Allowed` は
//!   [`SwitchOutcome::Accepted`]、`DeniedByUser` は `PermissionDenied`、`DeniedBySystem` と
//!   `Unspecified` は [`DENIED_BY_SYSTEM`]
//! - Android（#974）: Wi-Fi の切り替えと Bluetooth のオフは設定画面を開いて
//!   [`SwitchOutcome::Opened`]。Bluetooth のオンは `ACTION_REQUEST_ENABLE` で、`RESULT_OK` は
//!   [`SwitchOutcome::Applied`]、`RESULT_CANCELED` は [`SwitchOutcome::NotChanged`]。[`RadioState::Disabled`] は返さない（相当する OS 値がない）。
//!   読み取りの `SecurityException` は `PermissionDenied`

use crate::DeviceError;

/// `Unsupported` / `PermissionDenied` の機能名。
pub const FEATURE: &str = "radio_control";
/// `Internal` の識別子（OS が拒否した）。
pub const DENIED_BY_SYSTEM: &str = "radio_control denied_by_system";
/// `Internal` の識別子（上記以外の失敗）。
pub const FAILED: &str = "radio_control failed";

/// 対象の無線。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioKind {
    /// Wi-Fi。
    Wifi,
    /// Bluetooth。
    Bluetooth,
}

/// 無線の状態。OS が答えた値だけを写し、取れない値を `Off` に畳まない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioState {
    /// オン。
    On,
    /// オフ。
    Off,
    /// OS が止めている（アプリからは変えられない）。
    Disabled,
    /// 切り替わりの途中。
    Transitioning,
    /// OS が判定できない。
    Unknown,
    /// この端末に無線がない。
    NotPresent,
}

/// 切り替え要求の結果。いずれも「その状態になった」ことを意味せず、状態は読み直しの値で出す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchOutcome {
    /// OS がその場で切り替えた。
    Applied,
    /// OS が要求を受け付けた（最終状態は非同期）。
    Accepted,
    /// 設定画面を開いた（切り替えは利用者が行う）。
    Opened,
    /// 利用者の操作などで変わらなかった。
    NotChanged,
}

/// 無線の状態取得と切り替えの契約（モジュール doc の「契約」節）。
pub trait RadioControl: Send + Sync {
    /// 状態を 1 回読む。呼び出し側がワーカースレッドで呼ぶブロッキング呼び出し。
    fn read(&self, kind: RadioKind) -> Result<RadioState, DeviceError>;
    /// オン（`true`）/オフ（`false`）を 1 回要求する。呼び出し側がワーカースレッドで呼ぶブロッキング呼び出し。
    fn set(&self, kind: RadioKind, on: bool) -> Result<SwitchOutcome, DeviceError>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedRadioControl;

impl RadioControl for UnsupportedRadioControl {
    fn read(&self, _kind: RadioKind) -> Result<RadioState, DeviceError> {
        Err(DeviceError::Unsupported(FEATURE))
    }

    fn set(&self, _kind: RadioKind, _on: bool) -> Result<SwitchOutcome, DeviceError> {
        Err(DeviceError::Unsupported(FEATURE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        for kind in [RadioKind::Wifi, RadioKind::Bluetooth] {
            assert!(matches!(
                UnsupportedRadioControl.read(kind),
                Err(DeviceError::Unsupported(FEATURE))
            ));
            for on in [true, false] {
                assert!(matches!(
                    UnsupportedRadioControl.set(kind, on),
                    Err(DeviceError::Unsupported(FEATURE))
                ));
            }
        }
    }
}
