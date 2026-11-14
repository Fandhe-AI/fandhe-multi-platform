//! Phase 11 の拡充ループ（親 #130、Issue #659）型サンプル「Bluetooth Classic」の effect ドライバ。
//! `fandhe_core::screens::bluetooth_classic_show`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `crate::contract`（12 秒の窓・上限 50 件・`normalize_devices`・
//! `decode_code`）。
//!
//! 一覧と検索は別の経路で駆動する。
//!
//! - **ペアリング済み一覧**: `usb_device` と同型の同期読み（[`drive_paired_sync`]）。UI スレッドから
//!   `BluetoothClassic::paired` を 1 回呼ぶ。OS 実装は [`platform_bluetooth_classic`] の cfg 分岐が
//!   最終形で、各 OS は `implementation()` を持つモジュールだけを埋める
//! - **検索**: `lan_discovery` と同型の pull 型 `bridge`。OS 側が [`OBSERVED`] へ
//!   [`Observer::observe`] で機器を足し、窓が閉じたら [`finish`] を 1 回呼ぶ。バッファの照合・取り出しは
//!   1 つのロックの内側で行い（`device-bridge-contracts.md` 2 節）、成功のときだけ
//!   `normalize_devices` を通す。失敗時は部分結果を捨てる（同 6 節）
//!
//! # OS ごとの `present`
//!
//! 3 OS とも `present(request_id, duration) -> Result<(), BluetoothClassicError>` の同じ形で、後続の OS 実装は
//! 自分のモジュールの中身だけを埋め、本ファイルの dispatch には触らない。
//! - macOS: `macos`（IOBluetooth の `IOBluetoothDeviceInquiry`。main queue で動く）
//! - Windows: `windows`（#660。Win32 `BluetoothFindFirstDevice`。専用ワーカー）
//! - Android: `android`（#661。`BluetoothAdapter.startDiscovery`。窓は main Handler が閉じる）
//!
//! # 期限・終了
//!
//! 期限は `DISCOVERY_WINDOW_MS` だけ（OS 側へ渡して一致させる、同 3 節）。画面離脱はプロセス終了と
//! 同義のため、[`stop_shared_discovery`] を `device::shutdown` から呼んで検索を止める。再要求は
//! 古いセッションを置き換え、古い request_id の結果は照合して捨てる。Android は停止が
//! Activity のライフサイクルで完結するため分岐を持たない。
//!
//! # 上記 3 OS 以外
//!
//! `UnsupportedBluetoothClassic` を同期で呼び、`Unsupported` を画面へ出す（iOS・Web は対象外）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序、失敗を成功と見せないこと、検索の停止と資源の解放。
//! 守らない範囲は `crate::contract` のモジュール doc を参照。
//! 機器名・アドレスはログへ出さない（security.md A09）。

// 検索の経路（Observer・bridge・finish）は 3 OS とテストからだけ呼ばれる。
#![cfg_attr(
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "android",
        test
    )),
    allow(dead_code)
)]

use std::collections::HashMap;
use std::sync::Mutex;

use crate::contract::{BluetoothClassic, ClassicDevice, DeviceKey, DeviceKind, normalize_devices};
use crate::error::BluetoothClassicError;
use fandhe_core::screens::bluetooth_classic_show::{
    BluetoothClassicShowState, ClassicError, ClassicKind, ClassicRow, finish_discover,
    finish_paired, mark_discovering, mark_paired_reading, take_discover_request,
    take_paired_request,
};

use fandhe_runtime::CompletionSender;

/// 観測バッファに溜める機器数の上限。応答の洪水でメモリが膨らむのを防ぐ
/// （最終的な件数上限は `finish` 時の `normalize_devices` が `MAX_DEVICES` で切る）。
// ponytail: 超えた新しい機器は捨てる。上限に当たる環境が現れたら窓の中での入れ替え方を検討する。
// Windows・Android は骨格のスタブで、#660 / #661 が使い始めるまで未使用（そのとき本 allow を外す）。
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
const MAX_OBSERVED: usize = 256;

/// 現在のプラットフォームに応じた `BluetoothClassic` 実装を返す（状態を持たないため呼び出しごとに構築）。
/// Windows・Android は骨格のスタブ（`Unsupported`）で、#660 / #661 が実装に置き換える。
pub fn platform_bluetooth_classic() -> Box<dyn BluetoothClassic> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::implementation()
    }
    #[cfg(target_os = "windows")]
    {
        crate::windows::implementation()
    }
    #[cfg(target_os = "android")]
    {
        crate::android::implementation()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
    {
        Box::new(crate::contract::UnsupportedBluetoothClassic)
    }
}

fn kind_to_core(kind: DeviceKind) -> ClassicKind {
    match kind {
        DeviceKind::Computer => ClassicKind::Computer,
        DeviceKind::Phone => ClassicKind::Phone,
        DeviceKind::Network => ClassicKind::Network,
        DeviceKind::AudioVideo => ClassicKind::AudioVideo,
        DeviceKind::Peripheral => ClassicKind::Peripheral,
        DeviceKind::Imaging => ClassicKind::Imaging,
        DeviceKind::Wearable => ClassicKind::Wearable,
        DeviceKind::Toy => ClassicKind::Toy,
        DeviceKind::Health => ClassicKind::Health,
        DeviceKind::Misc => ClassicKind::Misc,
        DeviceKind::Uncategorized => ClassicKind::Uncategorized,
        DeviceKind::Unknown => ClassicKind::Unknown,
    }
}

/// 契約の `Result<Vec<ClassicDevice>, BluetoothClassicError>` を画面用へ写す（種別は 1 対 1、
/// その他は `Display` 文言のみ。内部構造は出さない、security.md A09）。
/// `Cancelled` は契約上返らないため `Other` に写す。
pub(crate) fn to_core(
    result: Result<Vec<ClassicDevice>, BluetoothClassicError>,
) -> Result<Vec<ClassicRow>, ClassicError> {
    match result {
        Ok(devices) => Ok(devices
            .into_iter()
            .map(|d| ClassicRow {
                name: d.name,
                kind: kind_to_core(d.kind),
                paired: d.paired,
                rssi: d.rssi,
            })
            .collect()),
        Err(BluetoothClassicError::PermissionDenied(_)) => Err(ClassicError::PermissionDenied),
        Err(BluetoothClassicError::Disabled(_)) => Err(ClassicError::Disabled),
        Err(BluetoothClassicError::Unsupported(_)) => Err(ClassicError::Unsupported),
        Err(other) => Err(ClassicError::Other(other.to_string())),
    }
}

type Buffer = Option<(u64, HashMap<DeviceKey, ClassicDevice>)>;

/// 観測バッファ。request_id ごとに作り直し、照合と取り出しを同じロックで行う。
pub(crate) struct Observer {
    inner: Mutex<Buffer>,
}

impl Observer {
    pub(crate) const fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Buffer> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 新しい要求の観測を始める（前の要求のバッファは捨てる）。
    pub(crate) fn begin(&self, request_id: u64) {
        *self.lock() = Some((request_id, HashMap::new()));
    }

    /// 機器 1 件を足す。現在の要求のときだけ。同じキーの合流規則は `normalize_devices` を
    /// 再利用する（規則を二重に書かない）。
    ///
    /// 呼び出し元は macOS の `macos`、Windows の `windows`、
    /// Android の `android`。
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub(crate) fn observe(&self, request_id: u64, key: DeviceKey, device: ClassicDevice) {
        let mut guard = self.lock();
        let Some((id, map)) = guard.as_mut() else {
            return;
        };
        if *id != request_id {
            return;
        }
        let merged = match map.remove(&key) {
            Some(prev) => normalize_devices([(key.clone(), prev), (key.clone(), device)]).remove(0),
            None if map.len() >= MAX_OBSERVED => return,
            None => device,
        };
        map.insert(key, merged);
    }

    /// 現在の要求の結果を確定して取り出す。要求が古い（置き換え・停止済み）なら `None`。
    /// 成功は契約の形、失敗は部分結果を捨てて `Err`。
    pub(crate) fn finish(
        &self,
        request_id: u64,
        result: Result<(), BluetoothClassicError>,
    ) -> Option<Result<Vec<ClassicDevice>, BluetoothClassicError>> {
        let mut guard = self.lock();
        if guard.as_ref().is_none_or(|(id, _)| *id != request_id) {
            return None;
        }
        let (_, map) = guard.take()?;
        drop(guard);
        Some(result.map(|()| normalize_devices(map)))
    }

    /// バッファを空にする（終了時）。
    pub(crate) fn clear(&self) {
        *self.lock() = None;
    }
}

/// macOS / Windows / Android 共通の観測バッファ（OS 側コールバックが書く）。
pub(crate) static OBSERVED: Observer = Observer::new();

/// OS 側の完了通知。バッファを確定して [`bridge`] へ置く（古い要求は捨てる）。
/// 呼び出し元（検索ドライバ・OS 実装）と同じ OS 集合にだけ置く（契約表 7 節）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
pub(crate) fn finish(request_id: u64, result: Result<(), BluetoothClassicError>) {
    if let Some(r) = OBSERVED.finish(request_id, result) {
        // 画面を介さない公開入口（[`start_discovery`]）の完了通知があれば、生の結果をそちらへ渡す。
        if let Some(cb) = take_callback(request_id) {
            bridge::SLOT.clear(request_id);
            cb(r);
        } else {
            bridge::deliver(request_id, r);
        }
    }
}

/// 検索結果の受け取り先（[`start_discovery`] が渡す完了通知）。
pub type DiscoverCallback =
    Box<dyn FnOnce(Result<Vec<ClassicDevice>, BluetoothClassicError>) + Send + 'static>;

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
/// 要求の登録（採番・完了通知の置き換え・OS への開始依頼）を 1 本に直列化するロック。
/// 並行する呼び出しで開始順が逆転し、古い開始依頼が新しい要求の観測を置き換えるのを防ぐ。
/// OS 側コールバックはこのロックを取らない。
static START: Mutex<()> = Mutex::new(());

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
/// 公開入口の完了通知（要求は 1 本だけ。新しい要求は古い通知を `Cancelled` で完了させて置き換える）。
static CALLBACK: Mutex<Option<(u64, DiscoverCallback)>> = Mutex::new(None);

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
fn take_callback(request_id: u64) -> Option<DiscoverCallback> {
    let mut g = CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_ref().is_some_and(|(id, _)| *id == request_id) {
        g.take().map(|(_, cb)| cb)
    } else {
        None
    }
}

/// 保留中の完了通知を `Cancelled` で完了させる（ロックの外で呼ぶ）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
fn cancel_callback() {
    let old = CALLBACK.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some((_, cb)) = old {
        cb(Err(BluetoothClassicError::Cancelled("bluetooth_classic")));
    }
}

/// 画面側の要求の持ち主印（公開入口の要求と同じ [`bridge::SLOT`] を共有するため、自分の分だけを追う）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
static SCREEN_OWNER: fandhe_runtime::request_slot::SlotOwner =
    fandhe_runtime::request_slot::SlotOwner::new();

/// 要求を採番して OS へ検索の開始を依頼する（画面の effect と公開入口で共通。[`START`] で直列化）。
/// `cb` が `None` なら画面側の要求として [`SCREEN_OWNER`] へ登録する。ユーザー完了通知（置き換えられた
/// 古い通知の `Cancelled`、同期失敗の完了）は [`START`] を解放してから呼ぶ（通知内からの再試行で自己デッドロックしない）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
fn begin_and_present(cb: Option<DiscoverCallback>) {
    use std::time::Duration;
    let screen = cb.is_none();
    let (request_id, old, started) = {
        let _guard = START.lock().unwrap_or_else(|e| e.into_inner());
        let request_id = bridge::begin();
        if screen {
            SCREEN_OWNER.adopt(request_id);
        }
        let old = {
            let mut g = CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::replace(&mut *g, cb.map(|cb| (request_id, cb)))
        };
        OBSERVED.begin(request_id);
        let duration = Duration::from_millis(crate::contract::DISCOVERY_WINDOW_MS);
        #[cfg(target_os = "macos")]
        let started = crate::macos::present(request_id, duration);
        #[cfg(target_os = "windows")]
        let started = crate::windows::present(request_id, duration);
        #[cfg(target_os = "android")]
        let started = crate::android::present(request_id, duration);
        (request_id, old, started)
    };
    if let Some((_, old)) = old {
        old(Err(BluetoothClassicError::Cancelled("bluetooth_classic")));
    }
    if let Err(e) = started {
        // 依頼自体が同期的に失敗した（コールバックは二度と来ない）。
        finish(request_id, Err(e));
    }
}

/// 画面を介さない検索の公開入口。検索を開始して即戻り、結果（窓が閉じたとき、または失敗）を `on_complete` へ
/// 1 回だけ渡す。`on_complete` は OS 側のスレッドから呼ばれうるため、重い処理や長い待ちを入れない。
/// 新しい要求（画面側を含む）・[`stop_shared_discovery`]・Activity 再生成で置き換わった要求は `Cancelled` で完了する。
/// 非対応 OS は即座に `Unsupported` で完了する。
pub fn start_discovery(on_complete: DiscoverCallback) {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
    begin_and_present(Some(on_complete));
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
    on_complete(Err(BluetoothClassicError::Unsupported("bluetooth_classic")));
}

/// OS からのコールバックとの橋渡し（pull 型、`RequestSlot` の facade）。
pub(crate) mod bridge {
    use crate::contract::ClassicDevice;
    use crate::error::BluetoothClassicError;
    use fandhe_core::screens::bluetooth_classic_show::{ClassicError, ClassicRow};

    use fandhe_runtime::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<Vec<ClassicRow>, ClassicError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// `request_id` 不一致・既に結果ありは捨てて `false`。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<Vec<ClassicDevice>, BluetoothClassicError>,
    ) -> bool {
        SLOT.deliver(request_id, super::to_core(result))
    }
}

/// ペアリング済み一覧の要求を UI スレッドで同期処理して完了を送る
/// （テストから任意の実装を差し込めるよう切り出す）。失敗は空の成功へ畳まない。
fn drive_paired_sync(
    state: &BluetoothClassicShowState,
    sender: &CompletionSender<BluetoothClassicShowState>,
    classic: &dyn BluetoothClassic,
) {
    let Some(request) = take_paired_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_paired_reading));
    let outcome = to_core(classic.paired());
    let generation = request.generation;
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut BluetoothClassicShowState| {
        finish_paired(s, generation, outcome);
    }));
}

/// macOS / Windows / Android の検索ドライバ（`lan_discovery::drive_callback` と同型）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
fn drive_discover_callback(
    state: &BluetoothClassicShowState,
    sender: &CompletionSender<BluetoothClassicShowState>,
) {
    use fandhe_core::screens::bluetooth_classic_show::DiscoverPhase;

    if take_discover_request(state).is_some() {
        let _ = sender.send(Box::new(mark_discovering));

        begin_and_present(None);
        return;
    }

    if state.discover_phase != DiscoverPhase::Discovering {
        return;
    }
    let generation = state.discover_generation;
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &SCREEN_OWNER,
        sender,
        |_request_id| {},
        move |s: &mut BluetoothClassicShowState, outcome| {
            // 反映できる状態か確かめてから `finish_discover` し、反映できたときだけ
            // `true` を返す（`RequestSlot` 側がそれを見て `clear` する）。
            if s.discover_phase != DiscoverPhase::Idle {
                finish_discover(s, generation, outcome);
                true
            } else {
                false
            }
        },
        // 公開入口・Activity 再生成で自分の要求が置き換わった／失われた。処理中のまま止めない。
        move |s: &mut BluetoothClassicShowState| {
            if s.discover_phase != DiscoverPhase::Idle {
                finish_discover(
                    s,
                    generation,
                    Err(ClassicError::Other("bluetooth_classic cancelled".into())),
                );
            }
        },
    );
}

/// 上記 3 OS 以外向け: `BluetoothClassic` トレイト経由の同期呼び出し。
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
fn drive_discover_sync(
    state: &BluetoothClassicShowState,
    sender: &CompletionSender<BluetoothClassicShowState>,
    classic: &dyn BluetoothClassic,
) {
    let Some(request) = take_discover_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_discovering));
    let outcome = to_core(classic.discover());
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut BluetoothClassicShowState| {
        finish_discover(s, generation, outcome);
    }));
}

/// `registry.rs` の `effect` 区画から `bluetooth_classic_show` 画面用に配線される effect ドライバ。
/// 一覧と検索の両方を駆動する。
pub fn bluetooth_classic_show_effect(
    state: &BluetoothClassicShowState,
    sender: &CompletionSender<BluetoothClassicShowState>,
) {
    drive_paired_sync(state, sender, platform_bluetooth_classic().as_ref());
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
    drive_discover_callback(state, sender);
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
    drive_discover_sync(state, sender, platform_bluetooth_classic().as_ref());
}

/// Activity 再生成時に旧要求の保留を捨てる（`fandhe-gallery` の `android_main` から呼ばれる。
/// Java 側の追跡は Activity インスタンスのフィールドで、再生成で一緒に消える）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    cancel_callback();
    bridge::SLOT.reset();
    OBSERVED.clear();
}

/// アプリ終了時に進行中の検索を止める（`fandhe-gallery` の終了処理から呼ばれる）。
/// Rust 側のロックを握ったまま OS 側の停止を呼ばない（macOS の停止は main queue へ同期ディスパッチし、
/// main 側のコールバックが同じロックを取るため、`device-bridge-contracts.md` 11 節）。
/// `OBSERVED.clear()` はロックを解放してから OS 側を止める。
pub fn stop_shared_discovery() {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
    cancel_callback();
    OBSERVED.clear();
    #[cfg(target_os = "macos")]
    crate::macos::stop();
    #[cfg(target_os = "windows")]
    crate::windows::stop_shared();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }
    use crate::contract::{MAX_DEVICES, UnsupportedBluetoothClassic, decode_code};
    use fandhe_core::screens::bluetooth_classic_show::{
        DiscoverPhase, PairedPhase, UNSUPPORTED_ERROR, start_discover,
    };
    use std::sync::mpsc;

    fn key(s: &str) -> DeviceKey {
        DeviceKey::new(s).unwrap()
    }

    fn dev(name: Option<&str>, paired: bool, rssi: Option<i8>) -> ClassicDevice {
        ClassicDevice {
            name: name.map(Into::into),
            kind: DeviceKind::Phone,
            paired,
            rssi,
        }
    }

    #[test]
    fn to_core_maps_each_error_kind_distinctly() {
        assert_eq!(
            to_core(Err(BluetoothClassicError::PermissionDenied("x"))),
            Err(ClassicError::PermissionDenied)
        );
        assert_eq!(
            to_core(Err(BluetoothClassicError::Disabled("x"))),
            Err(ClassicError::Disabled)
        );
        assert_eq!(
            to_core(Err(BluetoothClassicError::Unsupported("x"))),
            Err(ClassicError::Unsupported)
        );
        assert!(matches!(
            to_core(Err(BluetoothClassicError::Cancelled("x"))),
            Err(ClassicError::Other(_))
        ));
        assert!(matches!(
            to_core(Err(BluetoothClassicError::Internal("x"))),
            Err(ClassicError::Other(_))
        ));
        assert_eq!(to_core(Ok(vec![])), Ok(vec![]));
    }

    #[test]
    fn code_1_becomes_permission_denied_screen_error() {
        let r = to_core(decode_code(1).map(|()| vec![]));
        assert_eq!(r, Err(ClassicError::PermissionDenied));
    }

    #[test]
    fn observer_ignores_stale_request_ids_and_double_finish() {
        let o = Observer::new();
        o.begin(2);
        o.observe(1, key("old"), dev(Some("old"), false, None));
        assert!(o.finish(1, Ok(())).is_none());
        let out = o.finish(2, Ok(())).unwrap().unwrap();
        assert!(out.is_empty(), "古い request_id の observe は捨てる");
        assert!(o.finish(2, Ok(())).is_none());
    }

    #[test]
    fn observer_failure_drops_partial_result() {
        let o = Observer::new();
        o.begin(1);
        o.observe(1, key("a"), dev(Some("a"), false, None));
        let r = o
            .finish(1, Err(BluetoothClassicError::Disabled("x")))
            .unwrap();
        assert!(matches!(r, Err(BluetoothClassicError::Disabled(_))));
        assert_eq!(
            to_core(r),
            Err(ClassicError::Disabled),
            "失敗の完了で部分結果は残らない"
        );
    }

    #[test]
    fn observer_merges_and_does_not_erase_with_none() {
        let o = Observer::new();
        o.begin(1);
        o.observe(1, key("a"), dev(Some("A"), false, Some(-50)));
        o.observe(1, key("a"), dev(None, true, None));
        let out = o.finish(1, Ok(())).unwrap().unwrap();
        assert_eq!(out, vec![dev(Some("A"), true, Some(-50))]);
    }

    #[test]
    fn observer_caps_buffer_and_result() {
        let o = Observer::new();
        o.begin(1);
        for i in 0..MAX_OBSERVED + 10 {
            o.observe(1, key(&format!("{i:04}")), dev(Some("x"), false, None));
        }
        let out = o.finish(1, Ok(())).unwrap().unwrap();
        assert_eq!(out.len(), MAX_DEVICES);
    }

    #[test]
    fn stale_bridge_delivery_is_rejected() {
        let slot = bridge::SLOT.begin();
        assert!(!bridge::deliver(slot.wrapping_add(1), Ok(vec![])));
        bridge::SLOT.reset();
    }

    struct Failing(BluetoothClassicError);
    impl BluetoothClassic for Failing {
        fn paired(&self) -> Result<Vec<ClassicDevice>, BluetoothClassicError> {
            Err(match &self.0 {
                BluetoothClassicError::Disabled(m) => BluetoothClassicError::Disabled(m),
                BluetoothClassicError::PermissionDenied(m) => {
                    BluetoothClassicError::PermissionDenied(m)
                }
                _ => BluetoothClassicError::Internal("x"),
            })
        }
        fn discover(&self) -> Result<Vec<ClassicDevice>, BluetoothClassicError> {
            self.paired()
        }
    }

    fn run_paired(classic: &dyn BluetoothClassic) -> BluetoothClassicShowState {
        let (tx, rx) = mpsc::channel();
        let sender = fandhe_runtime::CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        let mut state = BluetoothClassicShowState::default();
        drive_paired_sync(&state, &sender, classic);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        state
    }

    #[test]
    fn paired_failures_never_become_empty_success() {
        let s = run_paired(&Failing(BluetoothClassicError::Disabled("x")));
        assert_eq!(s.paired_phase, PairedPhase::Idle);
        assert_eq!(
            s.paired_error,
            fandhe_core::screens::bluetooth_classic_show::DISABLED_ERROR
        );
        assert_eq!(s.paired_status, "");

        let s = run_paired(&Failing(BluetoothClassicError::PermissionDenied("x")));
        assert_eq!(
            s.paired_error,
            fandhe_core::screens::bluetooth_classic_show::PERMISSION_DENIED_ERROR
        );
        assert!(s.paired_rows.is_empty());
    }

    #[test]
    fn paired_unsupported_shows_unsupported_not_empty() {
        let s = run_paired(&UnsupportedBluetoothClassic);
        assert_eq!(s.paired_error, UNSUPPORTED_ERROR);
        assert!(s.paired_rows.is_empty());
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
    #[test]
    fn fallback_discover_completes_with_unsupported() {
        let (tx, rx) = mpsc::channel();
        let sender = fandhe_runtime::CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        let mut state = BluetoothClassicShowState::default();
        start_discover(&mut state);
        drive_discover_sync(&state, &sender, &UnsupportedBluetoothClassic);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(state.discover_phase, DiscoverPhase::Idle);
        assert_eq!(state.discover_error, UNSUPPORTED_ERROR);
        assert!(state.found_rows.is_empty());
    }

    #[test]
    fn discover_state_untouched_by_paired_driver() {
        let mut state = BluetoothClassicShowState::default();
        start_discover(&mut state);
        let (tx, rx) = mpsc::channel();
        let sender = fandhe_runtime::CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        drive_paired_sync(&state, &sender, &UnsupportedBluetoothClassic);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(state.discover_phase, DiscoverPhase::Requested);
    }
}
