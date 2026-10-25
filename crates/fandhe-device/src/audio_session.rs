//! Phase 11 の拡充ループ（親 #172、Issue #840）: 音声セッションの種別（他の音を止める / 下げる /
//! 混ぜる）を選んで OS に有効化し、割り込み・フォーカスの出来事を受け取る契約。
//! `fandhe-egui-proxy` の `device/audio_session.rs`（effect ドライバ）が [`AudioSession::start`] で
//! セッションを開始し、各 OS 実装（iOS: #841、Android: #842）が本トレイトを実装する。
//! macOS は対象外、Windows は非対応、Web は対応表で保留（いずれも `UnsupportedAudioSession`）。
//!
//! # 方式: 出来事は OS の通知、今の状態は OS へ問い直す（OS を正にする）
//!
//! - 種別は OS から読み戻した値を出す。選んだ値で上書きしない
//! - 割り込み中か・フォーカスを持っているかをアプリ内に覚えない
//! - 事前判定をせず、`setActive` / `requestAudioFocus` の結果をそのまま写す
//! - 割り込み終了やフォーカスの再獲得で、勝手に有効にし直さない
//! - タイマー・経過時間を使わない
//!
//! 何を選べて何を知らせ何を読めるかは OS ごとに違い、[`Support`] で宣言する（画面は OS 名で分岐しない）。
//!
//! # 結果の種類（契約表 10）
//!
//! - 受信中: [`AudioSession::start`] が `Ok` で、guard が生きている間
//! - 停止（利用者の操作・画面を離れた）= 正常。[`AudioSessionGuard::stop`] が `Ok`
//! - 停止したが元の設定に戻せなかった: `stop` が `Err(RestoreFailed)`。失敗として見せるが成功に畳まない
//! - OS が受け付けなかった: [`StartFailure::Rejected`]（Android の `AUDIOFOCUS_REQUEST_FAILED` 等）
//! - 開始失敗: [`StartFailure::Failed`]。`restored: false` は元の設定に戻せなかった場合
//! - 実行中の失敗: [`Signal::Failed`]（iOS のメディアサービスのリセット等）。成功を装わない
//! - 非対応: [`StartFailure::Unsupported`] / `Err(Unsupported("audio_session"))`
//!
//! # 解放（契約表 9）
//!
//! iOS は変更前の category・mode・options・routeSharingPolicy を保存し、全経路（止める・画面を離れる
//! （プロセス終了 = `shutdown`、Android の再生成）・開始途中の失敗・実行中の失敗）で無効化と復元を
//! 独立に試す。購読の解除も同じ全経路で行い、解除後に遅れて届いた通知はドライバの登録表で捨てる。
//! [`AudioSessionGuard::stop`] を呼ばずに drop した場合も、実装は同じ後片付けを一度だけ試す。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の順序（開始・停止・取り出しは UI スレッドの 1 本の effect に集約し、OS
//! コールバックは受信順の列へ積むだけ）、嘘の成功の禁止（受け付けられない開始・戻せない停止・読めない
//! 状態を成功値へ畳まない）、資源の解放。
//!
//! 守らない範囲: 画面を開く前・止めた後の出来事、他画面と同時に音声セッションを使うこと、他機能の復元
//! 失敗の上に重ねた場合の戻り先、iOS の `appWasSuspended` 由来の遅れた通知の順序、Android 8 以降の
//! 自動ダッキング（コールバックが来ない）、複数プロセス、OS 側の通知の遅れ。
//!
//! # 後続 OS の要点
//!
//! - OS 資源は `id` で持ち、生ポインタを渡さない
//! - 停止は「登録解除 → OS の購読解除・無効化・復元 → 解放」の順で行い、ロックの内側では待たない
//! - Cocoa 呼び出しは `@autoreleasepool` の内側で行う。iOS の effect は main thread で走るため、
//!   `.m` は main から main へ `dispatch_sync` しない
//! - JNI の署名は Java 側・no-op スタブ・実体の 3 か所で一致させる
//! - Android の API 24・25 は `Unsupported` にする
//! - 録音を伴う category・新しい権限は使わない
//! - 背景への遷移では止めない（OS からの出来事は背景でも受ける）。Android 15 以上で背景から要求して
//!   FAILED になるのは [`StartFailure::Rejected`] として写す
//!
//! エラー文言・ログに OS のコード・メッセージを出さない。

use std::sync::Arc;

use crate::DeviceError;

/// 選べるセッション種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// 他の音を止める。
    StopOthers,
    /// 他の音を下げる。
    DuckOthers,
    /// 他の音と混ぜる。
    MixWithOthers,
}

/// 表示順の全種別。
pub const ALL_KINDS: [SessionKind; 3] = [
    SessionKind::StopOthers,
    SessionKind::DuckOthers,
    SessionKind::MixWithOthers,
];

impl SessionKind {
    /// [`ALL_KINDS`] 内の位置（選べる種別の配列の添字）。
    pub fn index(self) -> usize {
        match self {
            Self::StopOthers => 0,
            Self::DuckOthers => 1,
            Self::MixWithOthers => 2,
        }
    }

    /// FFI 用の整数コード（0 / 1 / 2）。
    pub fn code(self) -> i32 {
        self.index() as i32
    }
}

/// 整数コード → 種別。未知の値は `None`（成功値へ寄せない）。
pub fn kind_from_code(code: i32) -> Option<SessionKind> {
    match code {
        0 => Some(SessionKind::StopOthers),
        1 => Some(SessionKind::DuckOthers),
        2 => Some(SessionKind::MixWithOthers),
        _ => None,
    }
}

/// OS から読み戻した種別。本機能の 3 種に当たらない OS の値は `Other`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadKind {
    /// 他の音を止める。
    StopOthers,
    /// 他の音を下げる。
    DuckOthers,
    /// 他の音と混ぜる。
    MixWithOthers,
    /// 上の 3 種に当たらない OS の値。
    Other,
}

/// 整数コード → 読み戻した種別（0〜2 は対応する種別、3 とそれ以外は `Other`）。
pub fn read_kind_from_code(code: i32) -> ReadKind {
    match code {
        0 => ReadKind::StopOthers,
        1 => ReadKind::DuckOthers,
        2 => ReadKind::MixWithOthers,
        _ => ReadKind::Other,
    }
}

/// 受け取る出来事の種類（表示と回数の添字）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// iOS: 割り込みの開始。
    InterruptionBegan,
    /// iOS: 割り込みの終了。
    InterruptionEnded,
    /// iOS: 経路の変更。
    RouteChanged,
    /// Android: フォーカスの獲得。
    FocusGained,
    /// Android: フォーカスの喪失。
    FocusLost,
    /// Android: 一時的なフォーカスの喪失。
    FocusLostTransient,
    /// Android: 下げてよい一時的なフォーカスの喪失。
    FocusLostTransientCanDuck,
    /// Android: 出力経路が失われる（イヤホンを抜いた等）。
    BecomingNoisy,
}

/// 表示順の全種類。
pub const ALL_EVENTS: [EventKind; 8] = [
    EventKind::InterruptionBegan,
    EventKind::InterruptionEnded,
    EventKind::RouteChanged,
    EventKind::FocusGained,
    EventKind::FocusLost,
    EventKind::FocusLostTransient,
    EventKind::FocusLostTransientCanDuck,
    EventKind::BecomingNoisy,
];

impl EventKind {
    /// [`ALL_EVENTS`] 内の位置（回数の配列の添字）。
    pub fn index(self) -> usize {
        match self {
            Self::InterruptionBegan => 0,
            Self::InterruptionEnded => 1,
            Self::RouteChanged => 2,
            Self::FocusGained => 3,
            Self::FocusLost => 4,
            Self::FocusLostTransient => 5,
            Self::FocusLostTransientCanDuck => 6,
            Self::BecomingNoisy => 7,
        }
    }
}

/// 届いた出来事 1 件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSessionEvent {
    /// 種類。
    pub kind: EventKind,
    /// 再開してよいか。iOS の割り込み終了だけが `Some`。
    pub should_resume: Option<bool>,
}

impl AudioSessionEvent {
    fn plain(kind: EventKind) -> Self {
        Self {
            kind,
            should_resume: None,
        }
    }
}

/// OS が何を扱えるかの宣言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Support {
    /// 選べる種別（[`SessionKind::index`] の順）。
    pub kinds: [bool; 3],
    /// OS が知らせる出来事の種類（[`EventKind::index`] の順）。
    pub events: [bool; 8],
    /// 今の種別を読み戻せるか。
    pub kind_readable: bool,
    /// 他アプリの再生中を読めるか。
    pub other_playing_readable: bool,
    /// 音声モードを読めるか。
    pub audio_mode_readable: bool,
    /// 音楽の再生中を読めるか。
    pub music_active_readable: bool,
}

impl Support {
    /// 何も扱えない（非対応）。
    pub const NONE: Support = Support {
        kinds: [false; 3],
        events: [false; 8],
        kind_readable: false,
        other_playing_readable: false,
        audio_mode_readable: false,
        music_active_readable: false,
    };

    /// 何か 1 つでも選べる種別があるか。
    pub fn any_kind(&self) -> bool {
        self.kinds.iter().any(|k| *k)
    }
}

/// Android の音声モード（`AudioManager.getMode`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioMode {
    /// 通常。
    Normal,
    /// 着信音。
    Ringtone,
    /// 通話中。
    InCall,
    /// 通信中（VoIP 等）。
    InCommunication,
    /// 通話スクリーニング。
    CallScreening,
    /// 通話のリダイレクト。
    Redirect,
    /// 不明（`MODE_INVALID` / `MODE_CURRENT` と未知の値）。
    Unknown,
}

/// `AudioManager.getMode` の値 → 音声モード。-1・-2 と未知の値は `Unknown`。
pub fn audio_mode_from_android(code: i32) -> AudioMode {
    match code {
        0 => AudioMode::Normal,
        1 => AudioMode::Ringtone,
        2 => AudioMode::InCall,
        3 => AudioMode::InCommunication,
        4 => AudioMode::CallScreening,
        5 | 6 => AudioMode::Redirect,
        _ => AudioMode::Unknown,
    }
}

/// 今の状態。`None` は「この OS では読めない」（失敗とも偽とも違う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentState {
    /// OS が返した種別（iOS の category 等）。
    pub kind: Option<ReadKind>,
    /// 他アプリが再生中か（iOS の `isOtherAudioPlaying`）。
    pub other_audio_playing: Option<bool>,
    /// 音声モード（Android の `getMode`）。
    pub audio_mode: Option<AudioMode>,
    /// 音楽が再生中か（Android の `isMusicActive`）。
    pub music_active: Option<bool>,
}

/// OS コールバックからドライバへ渡す合図。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// 出来事が届いた。
    Event(AudioSessionEvent),
    /// 実行中の失敗（iOS のメディアサービスのリセット等）。
    Failed,
}

/// OS コールバックから呼ばれる通知先。呼び出しスレッドは任意で、すぐ戻ること。
pub type OnSignal = Arc<dyn Fn(Signal) + Send + Sync>;

/// 開始の失敗の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartFailure {
    /// 非対応。
    Unsupported,
    /// この OS では選べない種別。
    KindUnsupported,
    /// OS が受け付けなかった。
    Rejected,
    /// 開始の途中で失敗した。`restored` は元の設定へ戻せたか。
    Failed {
        /// 元の設定へ戻せたか。
        restored: bool,
    },
}

/// 停止で無効化または復元のどちらかが失敗した。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreFailed;

/// 開始したセッションの所有者。
pub trait AudioSessionGuard: Send {
    /// 無効化（iOS: `setActive(false, .notifyOthersOnDeactivation)` / Android:
    /// `abandonAudioFocusRequest`）と、変更前の値の復元を独立に試す。片方でも失敗したら `Err`。
    /// 戻った時点で以降の呼び出しはドライバの登録表で捨てられる。
    fn stop(self: Box<Self>) -> Result<(), RestoreFailed>;
}

/// 音声セッションの契約。
pub trait AudioSession: Send + Sync {
    /// OS が扱える範囲。
    fn support(&self) -> Support;

    /// 種別を指定してセッションを開始する。失敗時は購読・変更した OS の状態を残さない。
    fn start(
        &self,
        kind: SessionKind,
        on_signal: OnSignal,
    ) -> Result<Box<dyn AudioSessionGuard>, StartFailure>;

    /// 今の状態を OS へ問う（UI スレッドから同期で呼ぶ）。アプリ内の記憶から作らない。
    fn read_current(&self) -> Result<CurrentState, DeviceError>;
}

/// 非対応環境の実装。成功へ倒さない。
pub struct UnsupportedAudioSession;

impl AudioSession for UnsupportedAudioSession {
    fn support(&self) -> Support {
        Support::NONE
    }

    fn start(
        &self,
        _kind: SessionKind,
        _on_signal: OnSignal,
    ) -> Result<Box<dyn AudioSessionGuard>, StartFailure> {
        Err(StartFailure::Unsupported)
    }

    fn read_current(&self) -> Result<CurrentState, DeviceError> {
        Err(DeviceError::Unsupported("audio_session"))
    }
}

/// 直近に保持する出来事の件数。
pub const MAX_RECENT: usize = 8;

/// 受信記録。番号は受信順に 1 から振る。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventLog {
    next_seq: u64,
    recent: Vec<(u64, AudioSessionEvent)>,
    counts: [u64; 8],
}

impl EventLog {
    /// 空の記録。
    pub fn new() -> Self {
        Self::default()
    }

    /// 出来事を 1 件記録する。直近 [`MAX_RECENT`] 件を超えた古いものは捨てる（回数には残す）。
    pub fn record(&mut self, event: AudioSessionEvent) {
        self.next_seq += 1;
        self.counts[event.kind.index()] += 1;
        self.recent.insert(0, (self.next_seq, event));
        self.recent.truncate(MAX_RECENT);
    }

    /// `(新しい順の直近、種類ごとの回数)`。
    pub fn snapshot(&self) -> (Vec<(u64, AudioSessionEvent)>, [u64; 8]) {
        (self.recent.clone(), self.counts)
    }
}

/// iOS の受け口のコード → 合図（0 = 割り込み開始、1 = 割り込み終了（再開しない）、
/// 2 = 割り込み終了（再開してよい）、3 = 経路の変更、4 = メディアサービスのリセット）。
/// 未知は `None`（捏造しない）。
pub fn signal_from_ios_code(code: i32) -> Option<Signal> {
    let event = |kind, should_resume| {
        Some(Signal::Event(AudioSessionEvent {
            kind,
            should_resume,
        }))
    };
    match code {
        0 => event(EventKind::InterruptionBegan, None),
        1 => event(EventKind::InterruptionEnded, Some(false)),
        2 => event(EventKind::InterruptionEnded, Some(true)),
        3 => event(EventKind::RouteChanged, None),
        4 => Some(Signal::Failed),
        _ => None,
    }
}

/// `AUDIOFOCUS_GAIN`。
pub const ANDROID_FOCUS_GAIN: i32 = 1;
/// `AUDIOFOCUS_LOSS`。
pub const ANDROID_FOCUS_LOSS: i32 = -1;
/// `AUDIOFOCUS_LOSS_TRANSIENT`。
pub const ANDROID_FOCUS_LOSS_TRANSIENT: i32 = -2;
/// `AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK`。
pub const ANDROID_FOCUS_LOSS_TRANSIENT_CAN_DUCK: i32 = -3;
/// 自前定数: `ACTION_AUDIO_BECOMING_NOISY` を受けた。
pub const ANDROID_BECOMING_NOISY: i32 = 1000;

/// Android の受け口のコード → 出来事。2〜4（GAIN_TRANSIENT 系）と未知は `None`。
pub fn event_from_android_code(code: i32) -> Option<AudioSessionEvent> {
    let kind = match code {
        ANDROID_FOCUS_GAIN => EventKind::FocusGained,
        ANDROID_FOCUS_LOSS => EventKind::FocusLost,
        ANDROID_FOCUS_LOSS_TRANSIENT => EventKind::FocusLostTransient,
        ANDROID_FOCUS_LOSS_TRANSIENT_CAN_DUCK => EventKind::FocusLostTransientCanDuck,
        ANDROID_BECOMING_NOISY => EventKind::BecomingNoisy,
        _ => return None,
    };
    Some(AudioSessionEvent::plain(kind))
}

/// `requestAudioFocus` の結果 → 開始結果。1（GRANTED）だけ `Ok`、0（FAILED）は `Rejected`、
/// 2（DELAYED）は遅延獲得を要求しないので成功にしない、-1（API 26 未満）は `Unsupported`、
/// -2（JNI 例外等）と未知の値は `Failed { restored: true }`。
pub fn start_result_from_android(code: i32) -> Result<(), StartFailure> {
    match code {
        1 => Ok(()),
        0 => Err(StartFailure::Rejected),
        -1 => Err(StartFailure::Unsupported),
        _ => Err(StartFailure::Failed { restored: true }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: EventKind) -> AudioSessionEvent {
        AudioSessionEvent::plain(kind)
    }

    #[test]
    fn log_numbers_from_one_and_keeps_latest_eight() {
        let mut log = EventLog::new();
        for _ in 0..9 {
            log.record(ev(EventKind::FocusGained));
        }
        let (recent, counts) = log.snapshot();
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0].0, 9);
        assert_eq!(recent[7].0, 2);
        assert_eq!(counts[EventKind::FocusGained.index()], 9);
    }

    #[test]
    fn kind_codes_round_trip_and_reject_unknown() {
        for k in ALL_KINDS {
            assert_eq!(kind_from_code(k.code()), Some(k));
        }
        assert_eq!(kind_from_code(3), None);
        assert_eq!(kind_from_code(-1), None);
    }

    #[test]
    fn read_kind_unknown_is_other() {
        assert_eq!(read_kind_from_code(0), ReadKind::StopOthers);
        assert_eq!(read_kind_from_code(2), ReadKind::MixWithOthers);
        assert_eq!(read_kind_from_code(3), ReadKind::Other);
        assert_eq!(read_kind_from_code(99), ReadKind::Other);
        assert_eq!(read_kind_from_code(-1), ReadKind::Other);
    }

    #[test]
    fn audio_mode_boundaries() {
        assert_eq!(audio_mode_from_android(0), AudioMode::Normal);
        assert_eq!(audio_mode_from_android(4), AudioMode::CallScreening);
        assert_eq!(audio_mode_from_android(5), AudioMode::Redirect);
        assert_eq!(audio_mode_from_android(6), AudioMode::Redirect);
        assert_eq!(audio_mode_from_android(7), AudioMode::Unknown);
        assert_eq!(audio_mode_from_android(-1), AudioMode::Unknown);
        assert_eq!(audio_mode_from_android(-2), AudioMode::Unknown);
    }

    #[test]
    fn ios_codes_map_and_unknown_is_none() {
        assert_eq!(
            signal_from_ios_code(1),
            Some(Signal::Event(AudioSessionEvent {
                kind: EventKind::InterruptionEnded,
                should_resume: Some(false)
            }))
        );
        assert_eq!(
            signal_from_ios_code(2),
            Some(Signal::Event(AudioSessionEvent {
                kind: EventKind::InterruptionEnded,
                should_resume: Some(true)
            }))
        );
        assert_eq!(signal_from_ios_code(4), Some(Signal::Failed));
        assert_eq!(signal_from_ios_code(5), None);
        assert_eq!(signal_from_ios_code(-1), None);
    }

    #[test]
    fn android_codes_map_and_unknown_is_none() {
        assert_eq!(event_from_android_code(1), Some(ev(EventKind::FocusGained)));
        assert_eq!(event_from_android_code(-1), Some(ev(EventKind::FocusLost)));
        assert_eq!(
            event_from_android_code(-3),
            Some(ev(EventKind::FocusLostTransientCanDuck))
        );
        assert_eq!(
            event_from_android_code(ANDROID_BECOMING_NOISY),
            Some(ev(EventKind::BecomingNoisy))
        );
        for c in [0, 2, 3, 4, -4, 999] {
            assert_eq!(event_from_android_code(c), None);
        }
    }

    #[test]
    fn android_start_result_never_fakes_success() {
        assert_eq!(start_result_from_android(1), Ok(()));
        assert_eq!(start_result_from_android(0), Err(StartFailure::Rejected));
        assert_eq!(
            start_result_from_android(-1),
            Err(StartFailure::Unsupported)
        );
        for c in [2, -2, 3, 99] {
            assert_eq!(
                start_result_from_android(c),
                Err(StartFailure::Failed { restored: true })
            );
        }
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedAudioSession;
        assert_eq!(u.support(), Support::NONE);
        assert!(!u.support().any_kind());
        assert!(matches!(
            u.start(SessionKind::StopOthers, Arc::new(|_| {})),
            Err(StartFailure::Unsupported)
        ));
        assert!(matches!(
            u.read_current(),
            Err(DeviceError::Unsupported("audio_session"))
        ));
    }
}
