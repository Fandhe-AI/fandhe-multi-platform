//! Phase 11 の拡充ループ（親 #99、Issue #386）: 外部機器への再生（AirPlay・Cast 等）の契約。
//! `fandhe-egui-proxy` の `device/media_cast.rs`（effect ドライバ）が [`MediaCast`] を呼び、
//! 各 OS 実装（macOS: AVKit `AVRoutePickerView` + `AVAudioPlayer` = #386、iOS・Windows・Web は後続の子）が
//! 本トレイトを実装する。送る音は [`crate::media_playback::demo_wav`] の実行時生成 WAV で、
//! URL・ファイルパスは受けない。出力先の選択 UI は OS のものを使い、アプリは作らない。
//!
//! 範囲は「ループ再生」「出力先を選ぶ UI を出す」「選ばれた出力先の名前（OS が教えるときだけ）と接続状態」。
//! 画面ミラーリング・動画・シーク・アプリからの切断・Cast SDK・Presentation API は扱わない。
//! Android は Cast SDK が GMS 前提のため保留（`UnsupportedMediaCast` に入る）。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: [`CastPlayback::Playing`]。「外部で再生中」([`RouteState::Connected`]) は OS が通知したときだけ出す。
//!   macOS で読めなければ [`RouteState::NotReported`]（接続中とは言わない）
//! - キャンセル: ピッカーを閉じた・Web の `NotAllowedError` などは失敗にせず、元の経路へ戻す
//!   （[`RouteState::Choosing`] → [`RouteState::Local`] 等）。再生の停止も失敗ではなく [`CastPlayback::Stopped`]
//! - 失敗: [`CastFailure`]。非対応は `Err(Unsupported("media_cast"))`。成功値へ倒さない。
//!   OS の生コードは持たない（security.md A09）
//!
//! # ライフサイクル（契約表 1）
//!
//! - macOS: 窓が背面・最小化でも続ける。終了時は `device::shutdown` から [`MediaCast::release`] を呼ぶ
//! - iOS: 背景遷移で止めて [`CastPlayback::Interrupted`] にする
//! - Web: `pagehide` で [`CastPlayback::Interrupted`] にする
//!
//! # 世代（契約表 2）
//!
//! `prepare` ごとに世代を進める。OS のイベントは自分の世代を照合し、判定と更新は同じロックの内側で行う。
//! 古い世代の `play` / `stop` / `show_picker` は `InvalidInput("media_cast generation")`。
//!
//! # 期限（契約表 3）
//!
//! 期限は単調時計で測る。準備・再生・停止は [`START_DEADLINE`]、[`RouteState::Connecting`] は
//! [`CONNECT_DEADLINE`]（Windows・Web が使う）。期限後に届いた `Connected` は成功にしない。
//! [`RouteState::Choosing`] は利用者が操作している間なので期限を付けない。
//!
//! # 出力先の名前
//!
//! 画面にだけ出す（ログ・保存・ID の取得はしない）。OS が返した文字列は
//! [`crate::device_info::clean_text`] を通す。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の操作順、失敗を成功値へ畳まないこと、資源の解放、古い世代の通知を捨てること。
//! 守らない: 他画面との `AVAudioSession` の同時利用、OS の UI で切り替えた反映がポーリング間隔
//! （[`POLL_INTERVAL`]）ぶん遅れること、複数窓・複数プロセス・複数タブ、受信機側の品質や遅延。

use std::sync::Arc;
use std::time::Duration;

use crate::DeviceError;

/// 準備・再生・停止の各遷移の期限。
pub const START_DEADLINE: Duration = Duration::from_secs(5);
/// [`RouteState::Connecting`] の期限（Windows・Web）。
pub const CONNECT_DEADLINE: Duration = Duration::from_secs(15);
/// 画面が状態を取り直す間隔。
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// 再生の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastPlayback {
    /// 未準備。
    Idle,
    /// 準備済み（再生できる）。
    Ready,
    /// 再生中。
    Playing,
    /// 停止した。失敗ではない。
    Stopped,
    /// 背景遷移・`pagehide` で止まった。失敗ではない。
    Interrupted,
    /// 失敗。
    Failed(CastFailure),
}

/// 出力先の経路の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteState {
    /// この端末で鳴らしている。
    Local,
    /// 出力先を選んでいる（OS のピッカーが開いている）。
    Choosing,
    /// 接続中（Windows・Web）。
    Connecting,
    /// 外部機器へ出ている。名前は OS が教えたときだけ `Some`（[`crate::device_info::clean_text`] 済み）。
    Connected {
        /// 出力先の名前。
        name: Option<String>,
    },
    /// 外部機器から外れた。
    Disconnected,
    /// 出力先が見つからない。
    NoDevices,
    /// 経路の状態を OS から読めない（接続中とは言わない）。
    NotReported,
}

/// 失敗の種別。OS の生コードは持たない（security.md A09）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastFailure {
    /// 非対応。
    Unsupported,
    /// ユーザー操作の外・拒否・OS の制限で止められた。
    Blocked,
    /// この音源は送れない。
    SourceNotSupported,
    /// OS の失敗。
    Os,
    /// 期限切れ。
    Timeout,
    /// それ以外。
    Internal,
}

/// 出力先の選び方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    /// OS のボタンを画面に重ねる（Apple）。アプリからは開かない。
    Native,
    /// アプリのボタンからピッカーを出す（Windows・Web）。
    App,
}

/// 最新状態のスナップショット。`generation` は `prepare` が返した世代。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastSnapshot {
    /// 状態が属する世代。
    pub generation: u64,
    /// 再生の状態。
    pub playback: CastPlayback,
    /// 出力先の経路。
    pub route: RouteState,
    /// 出力先の選び方。
    pub picker: PickerKind,
}

/// 外部機器への再生。
pub trait MediaCast: Send + Sync {
    /// 音源（WAV）を受けて資源を作り、世代を進めて返す。前の資源を解放してから作り、失敗経路でも
    /// 作りかけを解放する。0 バイトは `InvalidInput("media_cast wav")`。非対応は `Unsupported`。
    fn prepare(&self, wav: Arc<[u8]>) -> Result<u64, DeviceError>;
    /// ループ再生を始める。結果は非同期で、[`Self::snapshot`] で確定する。古い世代は `InvalidInput`。
    fn play(&self, generation: u64) -> Result<(), DeviceError>;
    /// 再生を止める。再生中でなければ何もしない。古い世代は `InvalidInput`。
    fn stop(&self, generation: u64) -> Result<(), DeviceError>;
    /// 出力先のピッカーを出す。[`PickerKind::Native`] の実装は `Unsupported`（OS のボタンから選ぶ）。
    fn show_picker(&self, generation: u64) -> Result<(), DeviceError>;
    /// 最新状態。
    fn snapshot(&self) -> CastSnapshot;
    /// 資源の解放。冪等。失敗経路・期限切れ・アプリ終了のすべてで呼ぶ。
    fn release(&self);
}

/// 非対応 OS 向けの実装。常に `Unsupported` を返し、成功値へは倒さない。
pub struct UnsupportedMediaCast;

impl MediaCast for UnsupportedMediaCast {
    fn prepare(&self, _wav: Arc<[u8]>) -> Result<u64, DeviceError> {
        Err(DeviceError::Unsupported("media_cast"))
    }
    fn play(&self, _generation: u64) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("media_cast"))
    }
    fn stop(&self, _generation: u64) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("media_cast"))
    }
    fn show_picker(&self, _generation: u64) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("media_cast"))
    }
    fn snapshot(&self) -> CastSnapshot {
        CastSnapshot {
            generation: 0,
            playback: CastPlayback::Idle,
            route: RouteState::Local,
            picker: PickerKind::App,
        }
    }
    fn release(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        let c = UnsupportedMediaCast;
        let wav: Arc<[u8]> = Arc::from(vec![1u8]);
        assert!(matches!(
            c.prepare(wav),
            Err(DeviceError::Unsupported("media_cast"))
        ));
        assert!(c.play(1).is_err() && c.stop(1).is_err() && c.show_picker(1).is_err());
        let s = c.snapshot();
        assert_eq!(s.playback, CastPlayback::Idle);
        assert_eq!(s.route, RouteState::Local);
        c.release();
    }
}
