//! Phase 11 の拡充ループ（親 #88、Issue #294）: 数値 1 つ（0〜[`MAX_LIVE_VALUE`]）を OS の
//! ライブ表示として出す契約。iOS はライブアクティビティ（ActivityKit）、Android は進行中通知。
//! `fandhe-egui-proxy` の `device/live_activity.rs` が「開始」「更新」「終了」ボタン押下時に
//! [`LiveActivity::submit`] を呼ぶ。macOS は非対応、Windows・Web は対象外で、いずれも
//! [`UnsupportedLiveActivity`] を返す。同時に出すのは 1 件。push 更新・複数同時は範囲外。
//!
//! # 値
//!
//! [`LiveValue`] は 0〜[`MAX_LIVE_VALUE`]。範囲検証は OS 型へ変換する前に [`LiveValue::new`] で行い、
//! 超過は `InvalidInput("live_activity value")`。
//!
//! # 結果の三値（契約表 10）
//!
//! - 成功: [`LiveOutcome::Started`] / `Updated` / `Ended`。**OS へ渡して受理された**までを表し、
//!   表示されたかは確かめない。`End` は冪等で、既に閉じていても `Ended`
//! - 正常終了: [`LiveOutcome::Dismissed`]。`Update` で利用者やシステムが既に閉じたと分かった
//!   とき。失敗として見せず、再投稿もしない
//! - 失敗: 許可なし（iOS の `areActivitiesEnabled == false`・Android の通知無効や channel の
//!   重要度 `NONE`）→ `PermissionDenied("live_activity")`、非対応（iOS の拡張の未同梱を含む）→
//!   `Unsupported("live_activity")`、OS 呼び出しの失敗（背景からの開始を含む）→ `Internal("live_activity <操作>")`。
//!   成功へ倒さない。OS の生のエラーコード・値はログにも画面にも出さない
//! - キャンセル（`Cancelled`）は返さない。iOS には許可ダイアログが無く、Android は「拒否」と
//!   「閉じる」を区別できないため拒否に寄せる
//!
//! 初期化に失敗した実装は `Unsupported` か `Internal` を返し、ダミーの成功へ倒さない。
//!
//! # 完了の届け方
//!
//! [`LiveActivity::submit`] の `Ok(())` は受理で、結果は同じ `request_id` で 1 回だけ egui-proxy の
//! `bridge::deliver` へ届く。`request_id` が合わない完了は Rust 側と OS 側の両方で捨てる。
//! 期限は持たない（経過時間を足すときは単調時計 `Instant` で測る）。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の要求と完了の順序、嘘の成功の禁止、`Activity` 参照・通知の解放。
//! 守らない: プロセス死の後に残った表示（次回起動時に [`LiveActivity::dismiss_all`]）、利用者が
//! 閉じてから次の操作までの画面表示の食い違い、OS が描画へ反映するまでの時間差、iOS の 8 時間上限、
//! Android の OEM 独自の表示規則。並行経路の集約先は iOS が main actor、Android が main looper。

use crate::DeviceError;

/// 表示する値の上限。
pub const MAX_LIVE_VALUE: u32 = 9999;

/// 検証済みの値（0〜[`MAX_LIVE_VALUE`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveValue(u32);

impl LiveValue {
    /// 上限を超える値は `InvalidInput`。OS 型へ変換する前の唯一の検証点。
    pub fn new(value: u32) -> Result<Self, DeviceError> {
        if value > MAX_LIVE_VALUE {
            return Err(DeviceError::InvalidInput("live_activity value"));
        }
        Ok(Self(value))
    }

    /// 値。
    pub fn get(self) -> u32 {
        self.0
    }
}

/// 依頼する操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveOperation {
    /// 値を指定して開始する。
    Start(LiveValue),
    /// 表示中の値を更新する。
    Update(LiveValue),
    /// 終了する（冪等）。
    End,
}

/// 完了の種類。成功は「OS へ受理された」までで、表示されたかは確かめない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveOutcome {
    /// 開始を受理した。
    Started,
    /// 更新を受理した。
    Updated,
    /// 終了を受理した（既に閉じていても）。
    Ended,
    /// 既に利用者やシステムが閉じていた（正常終了。失敗ではない）。
    Dismissed,
}

/// ライブ表示を操作する。画面のボタン押下ごとに 1 回 [`submit`](Self::submit) が呼ばれる。
pub trait LiveActivity: Send + Sync {
    /// 操作を依頼する。`Ok(())` は受理で、結果は同じ `request_id` で 1 回だけ届く。
    /// `Err` は依頼そのものの失敗で、呼び出し元がその場で確定する。契約は本モジュール doc。
    fn submit(&self, request_id: u64, op: LiveOperation) -> Result<(), DeviceError>;

    /// 表示中のものを全部閉じる（冪等）。Android の Activity 再生成時と、実装の初期化時
    /// （前回プロセスの残り）に使う。
    fn dismiss_all(&self);
}

/// 非対応環境用。`submit` は常に `Unsupported`。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedLiveActivity;

impl LiveActivity for UnsupportedLiveActivity {
    fn submit(&self, _request_id: u64, _op: LiveOperation) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("live_activity"))
    }

    fn dismiss_all(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_range() {
        assert_eq!(LiveValue::new(0).unwrap().get(), 0);
        assert_eq!(LiveValue::new(MAX_LIVE_VALUE).unwrap().get(), 9999);
        assert!(matches!(
            LiveValue::new(10000),
            Err(DeviceError::InvalidInput("live_activity value"))
        ));
    }

    #[test]
    fn unsupported_never_succeeds() {
        let v = LiveValue::new(1).unwrap();
        for op in [
            LiveOperation::Start(v),
            LiveOperation::Update(v),
            LiveOperation::End,
        ] {
            assert!(matches!(
                UnsupportedLiveActivity.submit(1, op),
                Err(DeviceError::Unsupported("live_activity"))
            ));
        }
        UnsupportedLiveActivity.dismiss_all();
    }
}
