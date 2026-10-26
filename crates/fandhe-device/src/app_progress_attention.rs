//! Phase 11 の拡充ループ（親 #174、Issue #860）: Dock・タスクバーの進捗表示と注意喚起の契約。
//! `fandhe-egui-proxy` の `device/app_progress_attention.rs` が画面の要求を観測して
//! [`AppProgressAttention`] の 3 メソッドを UI スレッドから同期に呼ぶ。macOS
//! （`NSDockTile.contentView` / `requestUserAttention`）と Windows（`ITaskbarList3` /
//! `FlashWindowEx`）が実装し、それ以外は [`UnsupportedAppProgressAttention`]。
//!
//! # 値
//!
//! [`ProgressValue`] は 0〜[`MAX_PROGRESS`]（%）の確定値だけ。**0 は「0%」で消去の番兵ではない**
//! （消すのは [`AppProgressAttention::clear_progress`]）。不確定・一時停止・エラー色は範囲外。
//! 範囲検証は [`ProgressValue::new`] が OS 型へ変換する前に行う。
//!
//! # 結果の種類（契約表 10）
//!
//! - `Ok(())`: OS へ渡した（進捗の設定・注意喚起）／消去を依頼した。**表示中かは確かめられない**
//!   （両 OS に読み戻す API が無い）
//! - 待機の取り消し: 画面側の操作で OS は呼ばず、失敗にもしない（本トレイトを経由しない）
//! - `Err(InvalidInput("app_progress_attention value"))`: 範囲外。OS は呼ばない
//! - `Err(Unsupported("app_progress_attention"))`: Linux 等
//! - `Err(Internal("app_progress_attention progress" | "... clear" | "... attention" | "... window"))`:
//!   OS 呼び出しの失敗。OS の文言・`HRESULT` はログにも画面にも出さない。`PermissionDenied` と
//!   `Disabled` は使わない
//!
//! 初期化に失敗した実装は `Unsupported` か `Internal` を返し、ダミーの成功へ倒さない。
//!
//! # OS を正にする
//!
//! 進捗の現在値・注意喚起中かをアプリ内に保持しない。前面か・ハイコントラストかも事前に判定しない。
//! `FlashWindowEx` の戻り値（直前のキャプション状態）と `requestUserAttention` の戻り値（取り消し
//! 用 id）は結果に使わない。注意喚起は前面のままでは OS が何もしないため、呼び出し側（effect
//! ドライバ）がウィンドウのフォーカス喪失を観測した最初のフレームで 1 回だけ呼ぶ。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の要求と完了の順序、嘘の成功の禁止、資源（COM ポインタ・view）の解放。
//! 守らない: 他画面・複数ウィンドウ（Windows のグループ表示の優先度を含む）、画面を離れたときの
//! 表示の後始末（プロセス終了で OS が片付けるため終了処理に解除を足さない）、設定後に OS が表示を
//! 変えた・出さないときの食い違い（ハイコントラスト、Dock のリサイズでの描き直し等）、前面のままの
//! 注意喚起が何も起こさないこと、不確定・一時停止・エラーの進捗、1 回だけ跳ねる注意喚起と注意喚起の
//! 取り消し、`TaskbarButtonCreated` 前の呼び出し、アプリが実行中でないときの Dock タイル。
//! 並行経路は無い（デスクトップの UI スレッドで同期）。

use crate::DeviceError;

/// 進捗（%）の上限。
pub const MAX_PROGRESS: u32 = 100;

/// 検証済みの進捗（0〜[`MAX_PROGRESS`] %）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressValue(u32);

impl ProgressValue {
    /// 上限を超える値は `InvalidInput`。OS 型へ変換する前の唯一の検証点。
    pub fn new(value: u32) -> Result<Self, DeviceError> {
        if value > MAX_PROGRESS {
            return Err(DeviceError::InvalidInput("app_progress_attention value"));
        }
        Ok(Self(value))
    }

    /// 値（%）。
    pub fn get(self) -> u32 {
        self.0
    }
}

/// 進捗表示と注意喚起。デスクトップの UI スレッドから同期で呼ばれる。契約は本モジュール doc。
pub trait AppProgressAttention: Send + Sync {
    /// 進捗（%）を OS へ渡す。`Ok` は「渡した」までで表示は保証しない。
    fn set_progress(&self, value: ProgressValue) -> Result<(), DeviceError>;
    /// 進捗の消去を OS へ依頼する。
    fn clear_progress(&self) -> Result<(), DeviceError>;
    /// 利用者がアプリへ戻るまで続く注意喚起を OS へ依頼する。
    fn request_attention(&self) -> Result<(), DeviceError>;
}

/// 非対応環境（Linux 等）用。常に `Unsupported`。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedAppProgressAttention;

impl AppProgressAttention for UnsupportedAppProgressAttention {
    fn set_progress(&self, _value: ProgressValue) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("app_progress_attention"))
    }

    fn clear_progress(&self) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("app_progress_attention"))
    }

    fn request_attention(&self) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("app_progress_attention"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_range() {
        assert_eq!(ProgressValue::new(0).unwrap().get(), 0);
        assert_eq!(ProgressValue::new(100).unwrap().get(), 100);
        assert!(matches!(
            ProgressValue::new(101),
            Err(DeviceError::InvalidInput("app_progress_attention value"))
        ));
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedAppProgressAttention;
        let is_unsupported = |r: Result<(), DeviceError>| {
            matches!(r, Err(DeviceError::Unsupported("app_progress_attention")))
        };
        assert!(is_unsupported(
            u.set_progress(ProgressValue::new(5).unwrap())
        ));
        assert!(is_unsupported(u.clear_progress()));
        assert!(is_unsupported(u.request_attention()));
    }
}
