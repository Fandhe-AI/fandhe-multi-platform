//! Phase 11 の拡充ループ（親 #97、Issue #364）: ピクチャ・イン・ピクチャ（映像の小窓表示）の契約。
//! `fandhe-egui-proxy` の `device/picture_in_picture.rs`（effect ドライバ）が [`PictureInPicture`] を
//! 呼び、各 OS 実装（macOS: AVKit の sample buffer 経路 = #364、iOS・Android・Web は後続の子）が
//! 本トレイトを実装する。小窓に出す映像は実行時に生成する単色フレーム（[`FRAME_COLORS`] を
//! [`FRAME_INTERVAL`] ごとに切り替え）で、URL・ファイルパス・バイナリ素材は受けない。
//!
//! 範囲は「小窓にする」「小窓をやめる」のみ。音声、シーク、自動開始、Document PiP、egui 画面そのものの
//! 小窓化は扱わない（Android は Activity ごと小窓になるが、それは OS の挙動）。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: [`PipPhase::Active`]
//! - 「小窓をやめる」と、OS の小窓側の閉じる・戻す操作は失敗ではなく [`PipPhase::Exited`]（小窓から戻った）。
//!   開始中のキャンセル（`Starting` 中の `stop`）も失敗にせず `Exited`
//! - 非対応は `Err(Unsupported("picture_in_picture"))`。macOS 12 未満と `isPictureInPictureSupported == NO`
//!   もここに含める。成功値へ倒さない
//! - 失敗の写し方: `DeviceError::Disabled` → [`PipFailure::NotPossible`]、`PermissionDenied` →
//!   [`PipFailure::NotAllowed`]、期限切れ（[`START_DEADLINE`]）はドライバが `Timeout` にして `stop` を呼ぶ
//!
//! # ライフサイクル（契約表 1）
//!
//! - macOS: 窓が背面・最小化でも小窓は続く。終了時は `device::shutdown` から止めて解放する
//! - iOS: 小窓中の背景遷移は正常（中断にしない）
//! - Android: 小窓に入るときの `onPause` を中断と扱わない。Activity の作り直しでは `reset_pending_tracking`
//! - Web: `pagehide` で小窓を閉じる
//!
//! # 世代（契約表 2）
//!
//! `prepare` ごとに世代を進める。OS コールバックは自分の世代を照合し、判定と更新は同じロックの内側で行う。
//! `Starting` 中に `stop` が呼ばれたら、開始の完了後に必ず止める（期限切れの後始末も同じ経路）。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の操作順、失敗を成功値へ畳まないこと、資源の解放。
//! 守らない範囲: ポーリング間隔ぶんの表示の遅れ、画面内プレビューと OS 側の映像の色替えの時刻ずれ、
//! 小窓の位置・大きさ（OS が決める）、複数窓・複数プロセス・複数タブ、Android で小窓の間に画面が静止すること。

use std::time::Duration;

use crate::DeviceError;

/// 小窓に出す単色フレームの RGB。[`frame_color`] がこの表を巡回する（OS 側の映像と画面内プレビューで共有）。
pub const FRAME_COLORS: [[u8; 3]; 5] = [
    [0xE5, 0x39, 0x35],
    [0xFB, 0x8C, 0x00],
    [0x43, 0xA0, 0x47],
    [0x1E, 0x88, 0xE5],
    [0x8E, 0x24, 0xAA],
];
/// 色を切り替える間隔。
pub const FRAME_INTERVAL: Duration = Duration::from_secs(1);
/// 映像の幅（ピクセル、16:9）。
pub const FRAME_WIDTH: u32 = 320;
/// 映像の高さ（ピクセル）。
pub const FRAME_HEIGHT: u32 = 180;
/// 準備・開始・終了の各遷移の期限。
pub const START_DEADLINE: Duration = Duration::from_secs(5);
/// 画面が状態を取り直す間隔。
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// 準備開始からの経過時間に対する色。`(elapsed / FRAME_INTERVAL) % 色数` で引く純関数。
pub fn frame_color(elapsed: Duration) -> [u8; 3] {
    let idx = (elapsed.as_millis() / FRAME_INTERVAL.as_millis()) as usize % FRAME_COLORS.len();
    FRAME_COLORS[idx]
}

/// 小窓の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipPhase {
    /// 未準備。
    Idle,
    /// 映像源と controller を作成中（可否の確定待ち）。
    Preparing,
    /// 小窓にできる。
    Ready,
    /// 小窓への切り替え中。
    Starting,
    /// 小窓に出ている。
    Active,
    /// 小窓から戻している最中。
    Stopping,
    /// 小窓から戻った。失敗ではない。
    Exited,
    /// 失敗。
    Failed(PipFailure),
}

/// 失敗の種別。OS の生コードは持たない（security.md A09）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipFailure {
    /// 非対応。
    Unsupported,
    /// OS がいまは小窓にすることを許さない（Android の設定で無効など）。
    NotPossible,
    /// ユーザー操作の外・拒否で止められた（Web）。
    NotAllowed,
    /// それ以外。
    Internal,
}

/// 最新状態のスナップショット。`generation` は `prepare` が返した世代。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipSnapshot {
    /// 状態が属する世代。
    pub generation: u64,
    /// 進行状態。
    pub phase: PipPhase,
}

/// ピクチャ・イン・ピクチャ。
pub trait PictureInPicture: Send + Sync {
    /// 映像源と controller を作り、世代を進めて返す。前の資源を解放してから作り、失敗経路でも
    /// 作りかけを解放する。非対応は `Unsupported`。
    fn prepare(&self) -> Result<u64, DeviceError>;
    /// 小窓にする。受付は `Ready` / `Exited` のときだけ。結果は非同期で、[`Self::snapshot`] で確定する。
    /// 古い世代は `InvalidInput("picture_in_picture generation")`。
    fn start(&self, generation: u64) -> Result<(), DeviceError>;
    /// 小窓をやめる。`Starting` 中なら開始の完了後に必ず止める。古い世代は `InvalidInput`。
    fn stop(&self, generation: u64) -> Result<(), DeviceError>;
    /// 最新状態。
    fn snapshot(&self) -> PipSnapshot;
    /// アプリ終了時の解放。冪等。
    fn shutdown(&self);
}

/// 非対応 OS 向けの実装。常に `Unsupported` を返し、成功値へは倒さない。
pub struct UnsupportedPictureInPicture;

impl PictureInPicture for UnsupportedPictureInPicture {
    fn prepare(&self) -> Result<u64, DeviceError> {
        Err(DeviceError::Unsupported("picture_in_picture"))
    }
    fn start(&self, _generation: u64) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("picture_in_picture"))
    }
    fn stop(&self, _generation: u64) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("picture_in_picture"))
    }
    fn snapshot(&self) -> PipSnapshot {
        PipSnapshot {
            generation: 0,
            phase: PipPhase::Idle,
        }
    }
    fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_color_cycles() {
        assert_eq!(FRAME_COLORS.len(), 5);
        assert_eq!(frame_color(Duration::ZERO), FRAME_COLORS[0]);
        assert_eq!(frame_color(Duration::from_secs(1)), FRAME_COLORS[1]);
        assert_eq!(frame_color(Duration::from_millis(4900)), FRAME_COLORS[4]);
        assert_eq!(frame_color(Duration::from_secs(5)), FRAME_COLORS[0]);
    }

    #[test]
    fn unsupported_never_succeeds() {
        let p = UnsupportedPictureInPicture;
        assert!(matches!(
            p.prepare(),
            Err(DeviceError::Unsupported("picture_in_picture"))
        ));
        assert!(p.start(1).is_err() && p.stop(1).is_err());
        assert_eq!(p.snapshot().phase, PipPhase::Idle);
    }
}
