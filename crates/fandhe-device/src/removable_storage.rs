//! Phase 11 の拡充ループ（親 #175、Issue #858）: 外部ストレージ（ボリューム）の着脱を OS の通知で
//! 受け取り、今のボリューム一覧を OS から読み直す契約。`fandhe-egui-proxy` の
//! `device/removable_storage.rs`（effect ドライバ）が [`RemovableStorage::start`] で購読を開始し、
//! 各 OS 実装（macOS・Windows: #858、Android: #859）が本トレイトを実装する。iOS と Web は対応表で
//! 対象外（`UnsupportedRemovableStorage`）。
//!
//! # 方式: 出来事は OS の通知、一覧は OS へ問い直す
//!
//! 着脱の出来事は OS の通知だけで受け、ボリューム一覧はアプリ内で差分管理しない。通知の中身
//! （Windows の `dbcv_unitmask`、macOS の `NSDevicePath`、Android の `StorageVolume`）から一覧を
//! 組み立てず、画面を開いた時・出来事が届くたび・「再取得」の時に [`RemovableStorage::read_volumes`] で
//! OS から丸ごと読み直す。タイマー・経過時間は扱わない。何を知らせるかは OS ごとに違い、
//! [`EventSupport`] で宣言する（画面は OS 名で分岐しない）。ボリュームの中身は読まない・書かない・
//! 取り出し操作もしない。
//!
//! # 結果の種類（契約表 10）
//!
//! - 受信中: [`RemovableStorage::start`] が `Ok` で、guard が生きている間
//! - 停止（利用者の操作）= 正常。[`SubscriptionGuard`] の drop は失敗を生まない
//! - 開始失敗: `Err(Internal("removable_storage start"))`。失敗時は OS 資源を残さない
//! - 実行中の失敗: [`Signal::Failed`]（OS の通知ループが止まった等）。成功を装わない
//! - 非対応: `Err(Unsupported("removable_storage"))`
//!
//! 一覧の読み取りは 成功（0 件を含む `Ok(vec![])`）/ 失敗 `Err(Internal("removable_storage read"))` /
//! 非対応。失敗を 0 件に畳まない。`PermissionDenied` は返さない（どの OS も権限不要）。
//!
//! # 取らないもの
//!
//! UUID・ボリュームのシリアル番号・マウントパス・ドライブの内部 ID は読まない・保持しない・
//! ログに出さない（フィンガープリント防止）。名前は [`crate::device_info::clean_text`] のみ通す。
//! 種類・状態は OS が返した値をそのまま写し、「外部かどうか」をアプリで判定しない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の順序（開始・停止・取り出し・読み直しは UI スレッドの 1 本の effect に集約し、
//! OS コールバックは受信順の列へ積むだけ）、嘘の成功の禁止（読めない値は `Unknown`、読み取りの失敗は
//! `Err`）、資源の解放（停止・画面の再生成・開始途中の失敗の全経路で購読を解除し、解除後に届いた通知は
//! 捨てる）。
//!
//! 守らない範囲: 画面を開く前・止めた後の出来事、通知と一覧の読み直しの間の時間差（読み直した時点の
//! 一覧が正）、同じ着脱で OS が通知を複数回出すこと、OS 間の「外部」「取り外し可能」の定義の差、
//! macOS の強制抜去で `willUnmount` が届かないこと、一覧の読み取りが I/O で待つ間（ネットワーク
//! ボリューム等）の画面の停止と期限、複数プロセス・複数窓、OS 側の通知の遅れ。
//!
//! # 後続 OS の要点
//!
//! OS 資源は `id` で持ち、生ポインタを渡さない。停止は「登録解除 → OS の解除 → 解放」の順で、ロックの
//! 内側では待たない。Android の写像は [`state_from_android`] / [`event_from_android_state`] /
//! [`kind_from_android`] を使う。

use std::sync::Arc;

use crate::DeviceError;
use crate::device_info::clean_text;

/// 受け取る出来事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeEvent {
    /// マウントされた。
    Mounted,
    /// 取り外しの直前。
    WillUnmount,
    /// 取り外された。
    Unmounted,
    /// その他の変化。
    Other,
}

/// 表示順の全種類。
pub const ALL_EVENTS: [VolumeEvent; 4] = [
    VolumeEvent::Mounted,
    VolumeEvent::WillUnmount,
    VolumeEvent::Unmounted,
    VolumeEvent::Other,
];

impl VolumeEvent {
    /// [`ALL_EVENTS`] 内の位置（回数の配列の添字）。
    pub fn index(self) -> usize {
        match self {
            Self::Mounted => 0,
            Self::WillUnmount => 1,
            Self::Unmounted => 2,
            Self::Other => 3,
        }
    }
}

/// OS が知らせる種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSupport {
    /// 種類ごとに OS が通知を出すか（[`VolumeEvent::index`] の順）。
    pub events: [bool; 4],
}

impl EventSupport {
    /// 何も取れない（非対応）。
    pub const NONE: EventSupport = EventSupport { events: [false; 4] };

    /// 指定した種類を OS が知らせるか。
    pub fn supports(&self, event: VolumeEvent) -> bool {
        self.events[event.index()]
    }
}

/// ボリュームの種類（OS が返した属性の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeKind {
    /// 内蔵。
    Internal,
    /// 取り外し可能。
    Removable,
    /// 固定。
    Fixed,
    /// ネットワーク。
    Network,
    /// 光学ドライブ。
    Optical,
    /// RAM ディスク。
    RamDisk,
    /// 上のどれでもない。
    Other,
    /// OS が判定できないと返した、または値が読めない。
    Unknown,
}

/// ボリュームの状態（OS が返した値の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeState {
    /// マウント中。
    Mounted,
    /// 読み取り専用でマウント中。
    MountedReadOnly,
    /// メディアなし。
    NoMedia,
    /// 確認中。
    Checking,
    /// 取り出し中。
    Ejecting,
    /// マウントされていない。
    Unmounted,
    /// 取り外された。
    Removed,
    /// 読めない形式。
    Unmountable,
    /// OS が判定できないと返した、または値が読めない。
    Unknown,
}

/// ボリューム 1 件。UUID・シリアル番号・マウントパスは持たない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// 表示名（[`clean_text`] 済み）。取れなければ `None`。
    pub name: Option<String>,
    /// 種類。
    pub kind: VolumeKind,
    /// 状態。
    pub state: VolumeState,
}

/// 一覧に残す最大件数。
pub const MAX_VOLUMES: usize = 32;

/// 各 OS 実装が組み立てた一覧を契約の形へ整える（名前を [`clean_text`] に通し、先頭
/// [`MAX_VOLUMES`] 件だけ残す。同名は畳まない）。`name` は生の文字列を受ける。
pub fn volumes<I>(raw: I) -> Vec<Volume>
where
    I: IntoIterator<Item = (Option<String>, VolumeKind, VolumeState)>,
{
    raw.into_iter()
        .take(MAX_VOLUMES)
        .map(|(name, kind, state)| Volume {
            name: name.as_deref().and_then(clean_text),
            kind,
            state,
        })
        .collect()
}

/// OS コールバックからドライバへ渡す合図。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// 出来事が届いた。
    Event(VolumeEvent),
    /// OS の通知ループが止まった（実行中の失敗）。
    Failed,
}

/// OS コールバックから呼ばれる通知先。呼び出しスレッドは任意で、すぐ戻ること。
pub type OnSignal = Arc<dyn Fn(Signal) + Send + Sync>;

/// 購読の所有者。drop で OS の購読を解除する（利用者の停止は失敗を生まない）。戻った時点で
/// 以降の呼び出しはドライバの登録表で捨てられる。
pub trait SubscriptionGuard: Send {}

/// 外部ストレージの着脱の契約。
pub trait RemovableStorage: Send + Sync {
    /// OS が知らせる種類。
    fn support(&self) -> EventSupport;

    /// 購読を開始する。失敗時は OS 資源を残さない。
    fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError>;

    /// 今のボリューム一覧を OS から読む（UI スレッドから同期で呼ぶ）。アプリ内の記憶から作らない。
    /// 0 件は `Ok(vec![])`、失敗は `Err(Internal("removable_storage read"))`。
    fn read_volumes(&self) -> Result<Vec<Volume>, DeviceError>;
}

/// 非対応環境の実装。成功へ倒さない。
pub struct UnsupportedRemovableStorage;

impl RemovableStorage for UnsupportedRemovableStorage {
    fn support(&self) -> EventSupport {
        EventSupport::NONE
    }

    fn start(&self, _on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
        Err(DeviceError::Unsupported("removable_storage"))
    }

    fn read_volumes(&self) -> Result<Vec<Volume>, DeviceError> {
        Err(DeviceError::Unsupported("removable_storage"))
    }
}

/// 直近に保持する出来事の件数。
pub const MAX_RECENT: usize = 8;

/// 受信記録（純関数的に畳む）。番号は受信順に 1 から振る。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventLog {
    next_seq: u64,
    recent: Vec<(u64, VolumeEvent)>,
    counts: [u64; 4],
}

impl EventLog {
    /// 空の記録。
    pub fn new() -> Self {
        Self::default()
    }

    /// 出来事を 1 件記録する。直近 [`MAX_RECENT`] 件を超えた古いものは捨てる（回数には残す）。
    pub fn record(&mut self, event: VolumeEvent) {
        self.next_seq += 1;
        self.counts[event.index()] += 1;
        self.recent.insert(0, (self.next_seq, event));
        self.recent.truncate(MAX_RECENT);
    }

    /// `(新しい順の直近、種類ごとの回数)`。
    pub fn snapshot(&self) -> (Vec<(u64, VolumeEvent)>, [u64; 4]) {
        (self.recent.clone(), self.counts)
    }
}

/// `DBT_DEVICEARRIVAL`。
pub const DBT_DEVICEARRIVAL: usize = 0x8000;
/// `DBT_DEVICEREMOVECOMPLETE`。
pub const DBT_DEVICEREMOVECOMPLETE: usize = 0x8004;
/// `DBT_DEVTYP_VOLUME`。
pub const DBT_DEVTYP_VOLUME: u32 = 2;
/// `WM_DEVICECHANGE`。
pub const WM_DEVICECHANGE: u32 = 0x0219;
/// `ERROR_NOT_READY`。
pub const ERROR_NOT_READY: u32 = 21;

/// `WM_DEVICECHANGE` の wParam とデバイス種別 → 出来事。ボリューム以外の種別、
/// `DBT_DEVICEQUERYREMOVE`・`DBT_DEVNODES_CHANGED` 等の他の wParam は `None`（捏造しない）。
pub fn event_from_windows(wparam: usize, devicetype: u32) -> Option<VolumeEvent> {
    if devicetype != DBT_DEVTYP_VOLUME {
        return None;
    }
    match wparam {
        DBT_DEVICEARRIVAL => Some(VolumeEvent::Mounted),
        DBT_DEVICEREMOVECOMPLETE => Some(VolumeEvent::Unmounted),
        _ => None,
    }
}

/// `GetDriveTypeW` の戻り値 → 種類（2 = 取り外し可能、3 = 固定、4 = ネットワーク、5 = CD-ROM、
/// 6 = RAM ディスク）。0・1・未知は `Unknown`。
pub fn kind_from_windows_drive_type(drive_type: u32) -> VolumeKind {
    match drive_type {
        2 => VolumeKind::Removable,
        3 => VolumeKind::Fixed,
        4 => VolumeKind::Network,
        5 => VolumeKind::Optical,
        6 => VolumeKind::RamDisk,
        _ => VolumeKind::Unknown,
    }
}

/// `GetVolumeInformationW` の成否とエラーコード → 状態。成功はマウント中、`ERROR_NOT_READY` は
/// メディアなし、それ以外の失敗は `Unknown`（もっともらしい値で埋めない）。
pub fn state_from_windows_volume_info(ok: bool, win32_error: u32) -> VolumeState {
    if ok {
        VolumeState::Mounted
    } else if win32_error == ERROR_NOT_READY {
        VolumeState::NoMedia
    } else {
        VolumeState::Unknown
    }
}

/// Apple の受け口のコード → 出来事（0 = Mount、1 = WillUnmount、2 = DidUnmount）。未知は `None`。
pub fn event_from_apple_code(code: i32) -> Option<VolumeEvent> {
    match code {
        0 => Some(VolumeEvent::Mounted),
        1 => Some(VolumeEvent::WillUnmount),
        2 => Some(VolumeEvent::Unmounted),
        _ => None,
    }
}

/// macOS の `URLResourceKey` の値 → 種類。OS が返した属性をそのまま写す。取り外し可能か
/// 取り出し可能なら `Removable`、そうでなく内蔵なら `Internal`、そうでなくローカルでないなら
/// `Network`、全部読めてどれにも当たらなければ `Other`、判定に要る値が読めなければ `Unknown`。
pub fn kind_from_apple(
    internal: Option<bool>,
    removable: Option<bool>,
    ejectable: Option<bool>,
    local: Option<bool>,
) -> VolumeKind {
    if removable == Some(true) || ejectable == Some(true) {
        return VolumeKind::Removable;
    }
    if internal == Some(true) {
        return VolumeKind::Internal;
    }
    if local == Some(false) {
        return VolumeKind::Network;
    }
    if internal.is_some() && removable.is_some() && ejectable.is_some() && local.is_some() {
        VolumeKind::Other
    } else {
        VolumeKind::Unknown
    }
}

/// Android の `StorageVolume.getState()` の文字列 → 状態。未知は `Unknown`。
pub fn state_from_android(state: &str) -> VolumeState {
    match state {
        "mounted" => VolumeState::Mounted,
        "mounted_ro" => VolumeState::MountedReadOnly,
        "checking" => VolumeState::Checking,
        "ejecting" => VolumeState::Ejecting,
        "unmounted" => VolumeState::Unmounted,
        "removed" | "bad_removal" => VolumeState::Removed,
        "nofs" | "unmountable" => VolumeState::Unmountable,
        _ => VolumeState::Unknown,
    }
}

/// Android の状態変化 → 出来事。`checking` と未知の値は「その他の変化」。
pub fn event_from_android_state(state: &str) -> VolumeEvent {
    match state {
        "mounted" | "mounted_ro" => VolumeEvent::Mounted,
        "ejecting" => VolumeEvent::WillUnmount,
        "unmounted" | "removed" | "bad_removal" => VolumeEvent::Unmounted,
        _ => VolumeEvent::Other,
    }
}

/// Android の `isPrimary` / `isRemovable` → 種類。取り外し可能を優先し、次に primary を内蔵とする。
pub fn kind_from_android(is_primary: bool, is_removable: bool) -> VolumeKind {
    if is_removable {
        VolumeKind::Removable
    } else if is_primary {
        VolumeKind::Internal
    } else {
        VolumeKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_numbers_from_one_and_keeps_latest_eight() {
        let mut log = EventLog::new();
        for _ in 0..9 {
            log.record(VolumeEvent::Mounted);
        }
        let (recent, counts) = log.snapshot();
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0].0, 9);
        assert_eq!(recent[7].0, 2);
        assert_eq!(counts[VolumeEvent::Mounted.index()], 9);
    }

    #[test]
    fn windows_events_map_only_volume_arrival_and_removal() {
        assert_eq!(event_from_windows(0x8000, 2), Some(VolumeEvent::Mounted));
        assert_eq!(event_from_windows(0x8004, 2), Some(VolumeEvent::Unmounted));
        // ボリューム以外（DBT_DEVTYP_DEVICEINTERFACE = 5）。
        assert_eq!(event_from_windows(0x8000, 5), None);
        // DBT_DEVICEQUERYREMOVE・DBT_DEVNODES_CHANGED。
        assert_eq!(event_from_windows(0x8001, 2), None);
        assert_eq!(event_from_windows(0x0007, 2), None);
    }

    #[test]
    fn apple_codes_map_and_drop_unknown() {
        assert_eq!(event_from_apple_code(0), Some(VolumeEvent::Mounted));
        assert_eq!(event_from_apple_code(1), Some(VolumeEvent::WillUnmount));
        assert_eq!(event_from_apple_code(2), Some(VolumeEvent::Unmounted));
        assert_eq!(event_from_apple_code(3), None);
        assert_eq!(event_from_apple_code(-1), None);
    }

    #[test]
    fn windows_drive_type_unknown_is_not_guessed() {
        assert_eq!(kind_from_windows_drive_type(2), VolumeKind::Removable);
        assert_eq!(kind_from_windows_drive_type(3), VolumeKind::Fixed);
        assert_eq!(kind_from_windows_drive_type(4), VolumeKind::Network);
        assert_eq!(kind_from_windows_drive_type(5), VolumeKind::Optical);
        assert_eq!(kind_from_windows_drive_type(6), VolumeKind::RamDisk);
        for t in [0, 1, 7, 99] {
            assert_eq!(kind_from_windows_drive_type(t), VolumeKind::Unknown);
        }
    }

    #[test]
    fn windows_volume_info_state() {
        assert_eq!(
            state_from_windows_volume_info(true, 0),
            VolumeState::Mounted
        );
        assert_eq!(
            state_from_windows_volume_info(false, ERROR_NOT_READY),
            VolumeState::NoMedia
        );
        assert_eq!(
            state_from_windows_volume_info(false, 5),
            VolumeState::Unknown
        );
    }

    #[test]
    fn apple_kind_follows_os_attributes() {
        let t = Some(true);
        let f = Some(false);
        assert_eq!(kind_from_apple(f, t, f, t), VolumeKind::Removable);
        assert_eq!(kind_from_apple(f, f, t, t), VolumeKind::Removable);
        assert_eq!(kind_from_apple(t, f, f, t), VolumeKind::Internal);
        assert_eq!(kind_from_apple(f, f, f, f), VolumeKind::Network);
        assert_eq!(kind_from_apple(f, f, f, t), VolumeKind::Other);
        assert_eq!(kind_from_apple(None, f, f, t), VolumeKind::Unknown);
    }

    #[test]
    fn android_mappings_unknown_falls_back() {
        assert_eq!(
            state_from_android("mounted_ro"),
            VolumeState::MountedReadOnly
        );
        assert_eq!(state_from_android("bad_removal"), VolumeState::Removed);
        assert_eq!(state_from_android("unmountable"), VolumeState::Unmountable);
        assert_eq!(state_from_android("???"), VolumeState::Unknown);
        assert_eq!(
            event_from_android_state("ejecting"),
            VolumeEvent::WillUnmount
        );
        assert_eq!(event_from_android_state("mounted"), VolumeEvent::Mounted);
        assert_eq!(event_from_android_state("removed"), VolumeEvent::Unmounted);
        assert_eq!(event_from_android_state("checking"), VolumeEvent::Other);
        assert_eq!(event_from_android_state(""), VolumeEvent::Other);
        assert_eq!(kind_from_android(true, true), VolumeKind::Removable);
        assert_eq!(kind_from_android(true, false), VolumeKind::Internal);
        assert_eq!(kind_from_android(false, false), VolumeKind::Other);
    }

    #[test]
    fn volumes_caps_and_cleans_names() {
        let raw = (0..33).map(|i| {
            (
                Some(format!("V{i}\u{0}")),
                VolumeKind::Other,
                VolumeState::Mounted,
            )
        });
        let v = volumes(raw);
        assert_eq!(v.len(), MAX_VOLUMES);
        assert_eq!(v[0].name.as_deref(), Some("V0"));
        let v = volumes([(
            Some(" \n".into()),
            VolumeKind::Unknown,
            VolumeState::Unknown,
        )]);
        assert_eq!(v[0].name, None);
        assert!(volumes(std::iter::empty()).is_empty());
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedRemovableStorage;
        assert_eq!(u.support(), EventSupport::NONE);
        assert!(matches!(
            u.start(Arc::new(|_| {})),
            Err(DeviceError::Unsupported("removable_storage"))
        ));
        assert!(matches!(
            u.read_volumes(),
            Err(DeviceError::Unsupported("removable_storage"))
        ));
    }
}
