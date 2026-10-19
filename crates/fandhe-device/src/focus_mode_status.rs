//! 集中モード・おやすみモードの状態の契約（Issue #438、親 #104）。「いま集中モード中か」を
//! 1 つの `bool` で取得するための橋渡しで、`fandhe-egui-proxy` の
//! `device/focus_mode_status.rs` が間隔つきで [`FocusStatusSource::read`] を呼ぶ
//! （pull 型。OS の変更通知は購読しない）。
//!
//! # 契約
//!
//! - 結果は `bool` か `DeviceError`（`Unsupported` / `Internal` のみ）。失敗・非対応で
//!   「オフ」を返さない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **三値**（契約表 10）: 「読めた」「非対応（`Unsupported`）」「失敗（`Internal`）」。
//!   利用者のキャンセル・権限ダイアログは本契約に無いので、キャンセルと権限拒否は該当なし
//! - 購読・保留中の要求・世代を持たないので、古い世代の完了・期限・キャンセルの問題は
//!   構造上起きない（契約表 1・2・3 は該当なし）
//! - 集中モードの状態は在席状況にあたるため、ログへ出さない
//! - 「変更に追従」は最大 [`READ_INTERVAL`] + 1 フレームで反映される、と読み替える
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 嘘の「オフ」を出さないこと（失敗・非対応・前回値の使い回しをしない）、1 プロセス内の取得。
//! 守らない: 反映の遅れ（最大 1 秒 + 1 フレーム）、OS 間で「集中モード」の意味が違うこと
//! （Android は割り込みフィルタ、Windows は Focus セッション）、フィルタの種類の区別、
//! 複数プロセス間の食い違い。

use std::time::Duration;

use crate::DeviceError;

/// 画面側が OS を読み直す間隔。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);

/// 集中モードの状態を 1 回読む。UI スレッドで同期に呼ばれるため短時間で戻ること。
pub trait FocusStatusSource: Send + Sync {
    /// 現在、集中モードがオンなら `true`。失敗後に直前の値を返し続けてはならない。
    fn read(&self) -> Result<bool, DeviceError>;
}

/// 常に非対応を返す実装（未実装 OS の dispatch 先・スタブ）。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedFocusStatus;

impl FocusStatusSource for UnsupportedFocusStatus {
    fn read(&self) -> Result<bool, DeviceError> {
        Err(DeviceError::Unsupported("focus_mode_status"))
    }
}

/// ネイティブ層（Android の JNI）が返す `i32` を契約の値へ写す。SSOT。
///
/// - `0`: オフ / `1`: オン
/// - `-2`: この OS 版では取れない（`Unsupported`）
/// - `-1` とそれ以外（未定義コード）: `Internal`（成功にしない）
pub fn focus_from_code(code: i32) -> Result<bool, DeviceError> {
    match code {
        0 => Ok(false),
        1 => Ok(true),
        -2 => Err(DeviceError::Unsupported("focus_mode_status")),
        _ => Err(DeviceError::Internal("focus_mode_status read")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedFocusStatus.read(),
            Err(DeviceError::Unsupported("focus_mode_status"))
        ));
    }

    #[test]
    fn codes_map_to_bool() {
        assert!(!focus_from_code(0).unwrap());
        assert!(focus_from_code(1).unwrap());
    }

    #[test]
    fn sentinel_and_undefined_codes_never_succeed() {
        assert!(matches!(
            focus_from_code(-2),
            Err(DeviceError::Unsupported("focus_mode_status"))
        ));
        for c in [-1, 2, -3, i32::MIN, i32::MAX] {
            assert!(
                matches!(focus_from_code(c), Err(DeviceError::Internal(_))),
                "{c}"
            );
        }
    }
}
