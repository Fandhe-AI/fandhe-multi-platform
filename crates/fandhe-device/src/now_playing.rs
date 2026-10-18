//! ロック画面・OS のメディア操作の契約（Issue #381、親 #98）。再生中の曲名と再生状態を OS の
//! 「再生中」表示（macOS はコントロールセンターとメディアキー）へ渡し、OS から来た再生・一時停止を
//! 画面へ返す橋渡し。`fandhe-egui-proxy` の `device/now_playing.rs` が `audio_play` 画面
//! （media-playback）の再生・一時停止の間だけ [`NowPlaying::publish`] を呼び、OS 側の操作は
//! [`SharedOsCommand`] 経由で拾う。
//!
//! # 契約
//!
//! - `publish` の結果は 3 種類: `Ok`（OS へ渡した。Apple は結果が返らないため「表示中」とは言わない）/
//!   `Err(Unsupported("now_playing"))` / `Err(Internal | InvalidInput("now_playing"))`。権限は要らないので
//!   `PermissionDenied` は返さない
//! - OS からの一時停止・再生は失敗ではなく、画面の `Paused` / `Playing` への遷移として扱う（契約表 10）
//! - 位置は `position_ms <= duration_ms` のときだけ渡す。範囲外は変換の前に `None` へ落とす（[`NowPlayingInfo::new`]）
//! - `clear` / `shutdown` は冪等。登録したコマンドの解除と表示の取り下げを済ませてから戻る
//! - 曲名・位置・OS の生エラーをログへ出さない
//!
//! # 並行経路の集約（契約表 11）
//!
//! Rust 側の受付は [`SharedOsCommand`] の Mutex 1 本。OS 側のコールバック（macOS の main queue、
//! Android の main Looper、Windows の SMTC のスレッド）は [`SharedOsCommand::push`] するだけで、
//! 画面の状態には触れない。画面側の effect ドライバが毎フレーム [`SharedOsCommand::take`] で取り出す。
//! 停止を確定したあとは [`SharedOsCommand::disarm`] が保留中の操作も捨てるので、古い操作で再開しない。
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内の順序、停止を確定した後の古い操作で再開しないこと、嘘の成功を出さないこと、
//! 登録の解放。
//! 守らない: ポーリング間隔ぶんの反映の遅れ、macOS で窓を最小化している間の遅れ、PiP との
//! `AVAudioSession` の取り合い、複数タブ・複数プロセス、`publish` と同じフレームに届いた操作の取りこぼし。

use std::sync::{Arc, Mutex, MutexGuard};

use crate::DeviceError;

/// OS の「再生中」に出す曲名（SSOT）。Apple には FFI の引数で渡す。
pub const DEMO_TITLE: &str = "Fandhe のサンプル音声";

/// OS から届く操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsMediaCommand {
    /// 再生。
    Play,
    /// 一時停止。
    Pause,
    /// 再生と一時停止の切り替え（メディアキー）。
    Toggle,
}

/// OS へ渡す「いまの再生」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NowPlayingInfo {
    /// 再生中なら true、一時停止中なら false。
    pub playing: bool,
    /// 再生位置（ミリ秒）。取れなければ `None`。
    pub position_ms: Option<u64>,
    /// 長さ（ミリ秒）。取れなければ `None`。
    pub duration_ms: Option<u64>,
}

impl NowPlayingInfo {
    /// 位置が長さを超えるなど矛盾する値は `None` に落として作る。OS の値へ変換する前にここを通す。
    pub fn new(playing: bool, position_ms: Option<u64>, duration_ms: Option<u64>) -> Self {
        let position_ms = match (position_ms, duration_ms) {
            (Some(p), Some(d)) if p > d => None,
            (p, _) => p,
        };
        Self {
            playing,
            position_ms,
            duration_ms,
        }
    }
}

/// OS のメディア操作 UI への橋渡し（モジュール doc の「契約」節）。UI スレッドから同期で呼ばれ、すぐ戻ること。
pub trait NowPlaying: Send + Sync {
    /// 曲名（[`DEMO_TITLE`]）と再生状態を OS へ渡し、OS 側の再生・一時停止の受け口を有効にする。
    fn publish(&self, info: NowPlayingInfo) -> Result<(), DeviceError>;
    /// 表示を取り下げ、受け口を外す（冪等）。
    fn clear(&self);
    /// アプリ終了時の後片付け（冪等）。
    fn shutdown(&self);
}

/// 非対応 OS・未実装 OS の dispatch 先。ダミーの成功実装へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedNowPlaying;

impl NowPlaying for UnsupportedNowPlaying {
    fn publish(&self, _info: NowPlayingInfo) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("now_playing"))
    }
    fn clear(&self) {}
    fn shutdown(&self) {}
}

#[derive(Debug, Default)]
struct Inner {
    armed: bool,
    pending: Option<OsMediaCommand>,
}

/// OS 側のコールバックが操作を置く共有状態。判定と更新は同じ Mutex の内側で行う。
#[derive(Debug, Clone, Default)]
pub struct SharedOsCommand {
    inner: Arc<Mutex<Inner>>,
}

impl SharedOsCommand {
    /// 未 arm（操作を受け付けない）で作る。
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 操作の受付を始める。
    pub fn arm(&self) {
        self.lock().armed = true;
    }

    /// 受付を止め、保留中の操作も捨てる。
    pub fn disarm(&self) {
        let mut g = self.lock();
        g.armed = false;
        g.pending = None;
    }

    /// armed のときだけ最新値で上書きして受け付ける。戻り値は受け付けたかどうか。
    pub fn push(&self, cmd: OsMediaCommand) -> bool {
        let mut g = self.lock();
        if g.armed {
            g.pending = Some(cmd);
        }
        g.armed
    }

    /// 保留中の操作を取り出す。
    pub fn take(&self) -> Option<OsMediaCommand> {
        self.lock().pending.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_before_arm_is_dropped() {
        let c = SharedOsCommand::new();
        assert!(!c.push(OsMediaCommand::Play));
        assert_eq!(c.take(), None);
    }

    #[test]
    fn latest_push_wins() {
        let c = SharedOsCommand::new();
        c.arm();
        assert!(c.push(OsMediaCommand::Play));
        assert!(c.push(OsMediaCommand::Pause));
        assert_eq!(c.take(), Some(OsMediaCommand::Pause));
        assert_eq!(c.take(), None);
    }

    #[test]
    fn disarm_drops_pending_and_rejects_later_pushes() {
        let c = SharedOsCommand::new();
        c.arm();
        c.push(OsMediaCommand::Play);
        c.disarm();
        assert_eq!(c.take(), None);
        assert!(!c.push(OsMediaCommand::Toggle));
        assert_eq!(c.take(), None);
    }

    #[test]
    fn clone_shares_state() {
        let a = SharedOsCommand::new();
        let b = a.clone();
        a.arm();
        assert!(b.push(OsMediaCommand::Toggle));
        assert_eq!(a.take(), Some(OsMediaCommand::Toggle));
    }

    #[test]
    fn unsupported_publish_is_not_success() {
        let r = UnsupportedNowPlaying.publish(NowPlayingInfo::new(true, None, None));
        assert!(matches!(r, Err(DeviceError::Unsupported("now_playing"))));
    }

    #[test]
    fn out_of_range_position_is_dropped() {
        assert_eq!(
            NowPlayingInfo::new(true, Some(10), Some(5)).position_ms,
            None
        );
        assert_eq!(
            NowPlayingInfo::new(true, Some(5), Some(5)).position_ms,
            Some(5)
        );
        assert_eq!(
            NowPlayingInfo::new(true, Some(5), None).position_ms,
            Some(5)
        );
    }
}
