//! Phase 11 の拡充ループ（親 #1059、Issue #1060）型サンプル「BLE スキャン」の effect
//! ドライバ。`fandhe_core::screens::ble_scan`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `crate::contract`（10 秒・上限 50 件・`normalize_scan`）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # iOS / macOS / Windows / Android: pull 型 `bridge`
//!
//! 広告は OS のスレッドから非同期に届き、完了も非同期のため、`BluetoothLe` トレイトを
//! 通さずコアの状態機械へ直接書き戻す（`fandhe-egui-proxy` の `device/biometric_auth.rs` /
//! `fandhe-egui-proxy` の `device/speech_recognition.rs` と同型。[`bridge`] は `RequestSlot` の facade）。
//! OS 側は [`OBSERVED`]（観測バッファ）へ [`Observer::observe`] で広告を足し、終了時に
//! [`finish`] を 1 回呼ぶ。バッファの照合・取り出しは 1 つのロックの内側で行い
//! （`device-bridge-contracts.md` 2 節）、成功のときだけ `normalize_scan` を通して
//! 契約の形（同一 id 統合・RSSI 降順・上限）にする。失敗時は部分結果を捨てる
//! （同 6 節「嘘の成功」）。
//!
//! # 期限・終了
//!
//! 期限は `SCAN_DURATION_MS` だけ（OS 側へ渡して一致させる、同 3 節）。画面離脱は
//! プロセス終了と同義（`FANDHE_PROXY_APP` は 1 プロセス 1 画面）のため、
//! [`stop_shared_scan`] を `fandhe-gallery` の終了処理から呼んでスキャンを止める。
//! 再要求は古いセッションを置き換え、古い request_id の結果は照合して捨てる。
//!
//! # Android（#1062）
//!
//! iOS / macOS / Windows と同じ pull 型 [`bridge`] / [`OBSERVED`] を使う。Java 側
//! （`BluetoothLeHelper.requestBleScan`）が `BluetoothLeScanner` を UI スレッド 1 本に集約して
//! 動かし、広告ごとに `nativeOnBleScanResult`、終了時に 1 回 `nativeOnBleScanFinished`
//! を呼ぶ（`crate::android`）。期限は `SCAN_DURATION_MS` を JNI 引数で渡し Java は
//! それをそのまま使う。停止は Activity のライフサイクル（`onPause` / `onDestroy`）で
//! 完結するため [`stop_shared_scan`] に Android 分岐は無い。
//!
//! # 上記 4 OS 以外
//!
//! `UnsupportedBluetoothLe` を同期で呼び、`Unsupported` を画面へ出す
//! （Web は別経路 `fandhe-web-dom`）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序、失敗を成功と見せないこと、スキャンの停止。
//! 守らない: 複数プロセスが同時にスキャンした場合の結果差、OS が広告を間引く影響、
//! 許可ダイアログの回答待ち（期限なし。終了で止まる）。
//! 周辺機器の名前・識別子はログへ出さない（security.md A09）。

use std::collections::HashMap;
use std::sync::Mutex;

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "android"
)))]
use crate::contract::BluetoothLe;
use crate::contract::{BleDevice, normalize_scan};
use crate::error::BluetoothLeError;
use fandhe_core::screens::ble_scan::{BleRow, ScanError};
use fandhe_core::screens::ble_scan::{BleScanState, finish_scan, mark_scanning, take_request};

use fandhe_runtime::CompletionSender;

const FEATURE: &str = "bluetooth_le";

/// 契約の `Result<Vec<BleDevice>, BluetoothLeError>` を画面用へ写す（種別は 1 対 1、
/// その他は `Display` 文言のみ。内部構造は出さない、security.md A09）。
pub(crate) fn to_core(
    result: Result<Vec<BleDevice>, BluetoothLeError>,
) -> Result<Vec<BleRow>, ScanError> {
    match result {
        Ok(devices) => Ok(devices
            .into_iter()
            .map(|d| BleRow {
                id: d.id,
                name: d.name,
                rssi_dbm: d.rssi_dbm,
            })
            .collect()),
        Err(BluetoothLeError::Disabled(_)) => Err(ScanError::Disabled),
        Err(BluetoothLeError::PermissionDenied(_)) => Err(ScanError::PermissionDenied),
        Err(BluetoothLeError::Unsupported(_)) => Err(ScanError::Unsupported),
        Err(BluetoothLeError::Cancelled(_)) => Err(ScanError::Cancelled),
        Err(other) => Err(ScanError::Other(other.to_string())),
    }
}

/// `.m` 側の完了コード → 契約のエラー（SSOT。`.m` 側で二重に写像しない）。
/// 0（成功）は呼び出し側が分岐する。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 1 | Bluetooth オフ | `Disabled` |
/// | 2 | 権限拒否 | `PermissionDenied` |
/// | 3 | 非対応 | `Unsupported` |
/// | それ以外 | 内部エラー・未知 | `Internal` |
#[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
pub(crate) fn error_from_apple_code(code: i32) -> BluetoothLeError {
    match code {
        1 => BluetoothLeError::Disabled(FEATURE),
        2 => BluetoothLeError::PermissionDenied(FEATURE),
        3 => BluetoothLeError::Unsupported(FEATURE),
        _ => BluetoothLeError::Internal("bluetooth_le outcome"),
    }
}

/// CoreBluetooth / Android `ScanResult.getRssi()` の RSSI → dBm（Android の
/// 「取得不能」番兵も同じ 127 で、Java は生値をそのまま渡す）。`.m` は取れないとき番兵 127 を渡す
/// （CoreBluetooth 自身も 127 を「取得不能」に使う）。実際の値域（負の dBm）と衝突
/// しないので `None` に写す（`device-bridge-contracts.md` 8 節）。`i16` に収まらない
/// 値も `None`。
#[cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "android")),
    allow(dead_code)
)]
pub(crate) fn rssi_from_apple(raw: i32) -> Option<i16> {
    if raw == 127 {
        return None;
    }
    i16::try_from(raw).ok()
}

/// Android `MainActivity` の BLE 完了コード → 契約のエラー（SSOT。Java 側で二重に
/// 写像しない）。0（成功）は呼び出し側が分岐する。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 1 | Bluetooth オフ | `Disabled` |
/// | 2 | 権限拒否 | `PermissionDenied` |
/// | 3 | 非搭載・LE 非対応 | `Unsupported` |
/// | 5 | 中断（`onPause` / `onDestroy` / 非前景での開始） | `Internal` |
/// | 6 | 位置情報サービスがオフ（API 30 以下、0 台成功への化けを防ぐ） | `Internal` |
/// | それ以外（4 含む） | 内部エラー・未知 | `Internal` |
///
/// 6 を `Disabled` にしないのは、画面が「Bluetooth がオフです」と原因を取り違えるため。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn error_from_android_code(code: i32) -> BluetoothLeError {
    match code {
        1 => BluetoothLeError::Disabled(FEATURE),
        2 => BluetoothLeError::PermissionDenied(FEATURE),
        3 => BluetoothLeError::Unsupported(FEATURE),
        5 => BluetoothLeError::Internal("bluetooth_le interrupted"),
        6 => BluetoothLeError::Internal("bluetooth_le location off"),
        _ => BluetoothLeError::Internal("bluetooth_le outcome"),
    }
}

/// Windows `BluetoothError` の生コード → 契約のエラー。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 1 | `RadioNotAvailable` | `Disabled` |
/// | 5 | `DisabledByPolicy` | `PermissionDenied` |
/// | 7 | `DisabledByUser` | `PermissionDenied` |
/// | 8 | `ConsentRequired` | `PermissionDenied` |
/// | 6 | `NotSupported` | `Unsupported` |
/// | 9 | `TransportNotSupported` | `Unsupported` |
/// | それ以外 | `ResourceInUse` / `OtherError` / 未知 | `Internal` |
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn error_from_bluetooth_error(code: i32) -> BluetoothLeError {
    match code {
        1 => BluetoothLeError::Disabled(FEATURE),
        5 | 7 | 8 => BluetoothLeError::PermissionDenied(FEATURE),
        6 | 9 => BluetoothLeError::Unsupported(FEATURE),
        _ => BluetoothLeError::Internal("bluetooth_le outcome"),
    }
}

/// 観測バッファ。request_id ごとに作り直し、照合と取り出しを同じロックで行う。
/// HashMap にするのは、Windows が広告パケットごとに通知してくるため Vec だと
/// 際限なく伸びるから。
pub(crate) struct Observer {
    inner: Mutex<Option<(u64, HashMap<String, BleDevice>)>>,
}

impl Observer {
    pub(crate) const fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<(u64, HashMap<String, BleDevice>)>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 新しい要求の観測を始める（前の要求のバッファは捨てる）。
    #[cfg_attr(
        not(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "windows",
            target_os = "android"
        )),
        allow(dead_code)
    )]
    pub(crate) fn begin(&self, request_id: u64) {
        *self.lock() = Some((request_id, HashMap::new()));
    }

    /// 広告 1 件を足す。現在の要求のときだけ。同一 id の合流規則は
    /// `normalize_scan` を再利用する（規則を二重に書かない）。
    #[cfg_attr(
        not(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "windows",
            target_os = "android"
        )),
        allow(dead_code)
    )]
    pub(crate) fn observe(&self, request_id: u64, device: BleDevice) {
        let mut guard = self.lock();
        let Some((id, map)) = guard.as_mut() else {
            return;
        };
        if *id != request_id {
            return;
        }
        let merged = match map.remove(&device.id) {
            Some(prev) => normalize_scan([prev, device]).remove(0),
            None => device,
        };
        map.insert(merged.id.clone(), merged);
    }

    /// 現在の要求の結果を確定して取り出す。要求が古い（置き換え・停止済み）なら
    /// `None`。成功は契約の形、失敗は部分結果を捨てて `Err`。
    pub(crate) fn finish(
        &self,
        request_id: u64,
        result: Result<(), BluetoothLeError>,
    ) -> Option<Result<Vec<BleDevice>, BluetoothLeError>> {
        let mut guard = self.lock();
        if guard.as_ref().is_none_or(|(id, _)| *id != request_id) {
            return None;
        }
        let (_, map) = guard.take()?;
        drop(guard);
        Some(result.map(|()| normalize_scan(map.into_values())))
    }

    /// バッファを空にする（終了時）。
    pub(crate) fn clear(&self) {
        *self.lock() = None;
    }
}

/// iOS / macOS / Windows / Android 共通の観測バッファ（OS 側コールバックが書く）。
pub(crate) static OBSERVED: Observer = Observer::new();

/// OS 側の完了通知。バッファを確定して [`bridge`] へ置く（古い要求は捨てる）。
#[cfg_attr(
    not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "windows",
        target_os = "android"
    )),
    allow(dead_code)
)]
pub(crate) fn finish(request_id: u64, result: Result<(), BluetoothLeError>) {
    if let Some(r) = OBSERVED.finish(request_id, result) {
        bridge::deliver(request_id, r);
    }
}

/// OS からのコールバックとの橋渡し（pull 型、`RequestSlot` の facade）。
#[cfg_attr(
    not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "windows",
        target_os = "android"
    )),
    allow(dead_code)
)]
pub(crate) mod bridge {
    use crate::contract::BleDevice;
    use crate::error::BluetoothLeError;
    use fandhe_core::screens::ble_scan::{BleRow, ScanError};

    use fandhe_runtime::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<Vec<BleRow>, ScanError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// `request_id` 不一致・既に結果ありは捨てて `false`。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<Vec<BleDevice>, BluetoothLeError>,
    ) -> bool {
        SLOT.deliver(request_id, crate::to_core(result))
    }
}

/// iOS / macOS / Windows / Android の effect ドライバ本体（`fandhe-egui-proxy` の `device/biometric_auth.rs::drive_callback`
/// と同型）。
#[cfg(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "android"
))]
fn drive_callback(state: &BleScanState, sender: &CompletionSender<BleScanState>) {
    use fandhe_core::screens::ble_scan::ScanPhase;
    use std::time::Duration;

    if take_request(state).is_some() {
        let _ = sender.send(Box::new(mark_scanning));

        let request_id = bridge::begin();
        OBSERVED.begin(request_id);
        let duration = Duration::from_millis(crate::contract::SCAN_DURATION_MS);
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        if !crate::apple::present(request_id, duration) {
            // 依頼自体が同期的に失敗した（コールバックは二度と来ない）。
            finish(
                request_id,
                Err(BluetoothLeError::Internal("bluetooth_le present")),
            );
        }
        #[cfg(target_os = "windows")]
        if let Err(e) = crate::windows::present(request_id, duration) {
            finish(request_id, Err(e));
        }
        #[cfg(target_os = "android")]
        if !crate::android::request_ble_scan(request_id, duration.as_millis() as i32) {
            finish(
                request_id,
                Err(BluetoothLeError::Internal("bluetooth_le present")),
            );
        }
        return;
    }

    if state.phase != ScanPhase::Scanning {
        return;
    }
    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut BleScanState, outcome| {
            // 反映できる状態か確かめてから `finish_scan` し、反映できたときだけ
            // `true` を返す（`RequestSlot` 側がそれを見て `clear` する）。
            if s.phase != ScanPhase::Idle {
                finish_scan(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// 上記 4 OS 以外向け: `BluetoothLe` トレイト経由の同期呼び出し。
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "android"
)))]
fn drive(state: &BleScanState, sender: &CompletionSender<BleScanState>, ble: &dyn BluetoothLe) {
    let Some(request) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_scanning));
    let outcome = to_core(ble.scan());
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut BleScanState| {
        finish_scan(s, generation, outcome);
    }));
}

/// 同期の [`BluetoothLe`] 実装（`Unsupported` 固定）。iOS・macOS・Windows・Android は権限ダイアログや OS のコールバックの結果を
/// 受ける pull 型の [`bridge`] / [`OBSERVED`] を使い、同期の [`BluetoothLe`] を持たないため、この入口は上記 4 OS 以外にだけ載せる
/// （Web は別経路 `fandhe-web-dom`）。
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "android"
)))]
pub fn platform_bluetooth_le() -> Box<dyn BluetoothLe> {
    Box::new(crate::contract::UnsupportedBluetoothLe)
}

/// `registry.rs` の `effect` 区画から `ble_scan` 画面用に配線される effect ドライバ。
pub fn ble_scan_effect(state: &BleScanState, sender: &CompletionSender<BleScanState>) {
    #[cfg(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "windows",
        target_os = "android"
    ))]
    drive_callback(state, sender);
    #[cfg(not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "windows",
        target_os = "android"
    )))]
    drive(state, sender, &crate::contract::UnsupportedBluetoothLe);
}

/// Activity 再生成時に旧要求の保留を捨てる（`fandhe-gallery` の `android_main` から呼ばれる。
/// Java 側の追跡は Activity インスタンスのフィールドで、再生成で一緒に消える）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::SLOT.reset();
    OBSERVED.clear();
}

/// アプリ終了時に進行中のスキャンを止める（`fandhe-gallery` の終了処理から呼ばれる）。
/// Rust 側のロックを握ったまま OS 側の停止を呼ばない（OS 側コールバックが同じ
/// ロックを取るため、`device-bridge-contracts.md` 11 節）。
pub fn stop_shared_scan() {
    OBSERVED.clear();
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    crate::apple::stop();
    #[cfg(target_os = "windows")]
    crate::windows::stop_shared();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, name: Option<&str>, rssi: Option<i16>) -> BleDevice {
        BleDevice {
            id: id.into(),
            name: name.map(Into::into),
            rssi_dbm: rssi,
        }
    }

    #[test]
    fn to_core_maps_each_error_kind_distinctly() {
        assert_eq!(
            to_core(Err(BluetoothLeError::Disabled("x"))),
            Err(ScanError::Disabled)
        );
        assert_eq!(
            to_core(Err(BluetoothLeError::PermissionDenied("x"))),
            Err(ScanError::PermissionDenied)
        );
        assert_eq!(
            to_core(Err(BluetoothLeError::Unsupported("x"))),
            Err(ScanError::Unsupported)
        );
        assert_eq!(
            to_core(Err(BluetoothLeError::Cancelled("x"))),
            Err(ScanError::Cancelled)
        );
        assert!(matches!(
            to_core(Err(BluetoothLeError::Internal("x"))),
            Err(ScanError::Other(_))
        ));
        assert_eq!(to_core(Ok(vec![])), Ok(vec![]));
    }

    #[test]
    fn apple_code_table() {
        assert!(matches!(
            error_from_apple_code(1),
            BluetoothLeError::Disabled(_)
        ));
        assert!(matches!(
            error_from_apple_code(2),
            BluetoothLeError::PermissionDenied(_)
        ));
        assert!(matches!(
            error_from_apple_code(3),
            BluetoothLeError::Unsupported(_)
        ));
        assert!(matches!(
            error_from_apple_code(4),
            BluetoothLeError::Internal(_)
        ));
        assert!(matches!(
            error_from_apple_code(99),
            BluetoothLeError::Internal(_)
        ));
    }

    #[test]
    fn android_code_table() {
        assert!(matches!(
            error_from_android_code(1),
            BluetoothLeError::Disabled(_)
        ));
        assert!(matches!(
            error_from_android_code(2),
            BluetoothLeError::PermissionDenied(_)
        ));
        assert!(matches!(
            error_from_android_code(3),
            BluetoothLeError::Unsupported(_)
        ));
        for c in [4, 5, 6, 99] {
            assert!(matches!(
                error_from_android_code(c),
                BluetoothLeError::Internal(_)
            ));
        }
    }

    #[test]
    fn rssi_sentinel_is_none() {
        assert_eq!(rssi_from_apple(127), None);
        assert_eq!(rssi_from_apple(-60), Some(-60));
        assert_eq!(rssi_from_apple(100_000), None);
    }

    #[test]
    fn bluetooth_error_table() {
        assert!(matches!(
            error_from_bluetooth_error(1),
            BluetoothLeError::Disabled(_)
        ));
        for c in [5, 7, 8] {
            assert!(matches!(
                error_from_bluetooth_error(c),
                BluetoothLeError::PermissionDenied(_)
            ));
        }
        for c in [6, 9] {
            assert!(matches!(
                error_from_bluetooth_error(c),
                BluetoothLeError::Unsupported(_)
            ));
        }
        for c in [2, 3, 4, 99] {
            assert!(matches!(
                error_from_bluetooth_error(c),
                BluetoothLeError::Internal(_)
            ));
        }
    }

    #[test]
    fn observer_merges_and_sorts_by_rssi() {
        let o = Observer::new();
        o.begin(1);
        o.observe(1, dev("a", Some("A"), Some(-80)));
        o.observe(1, dev("b", None, Some(-40)));
        o.observe(1, dev("a", None, Some(-60)));
        let out = o.finish(1, Ok(())).unwrap().unwrap();
        assert_eq!(
            out,
            vec![dev("b", None, Some(-40)), dev("a", Some("A"), Some(-60))]
        );
    }

    #[test]
    fn observer_failure_drops_partial_result() {
        let o = Observer::new();
        o.begin(1);
        o.observe(1, dev("a", None, Some(-50)));
        let r = o.finish(1, Err(BluetoothLeError::Disabled("x"))).unwrap();
        assert!(matches!(r, Err(BluetoothLeError::Disabled(_))));
    }

    #[test]
    fn observer_ignores_stale_request_ids() {
        let o = Observer::new();
        o.begin(2);
        o.observe(1, dev("old", None, Some(-10)));
        assert!(o.finish(1, Ok(())).is_none());
        let out = o.finish(2, Ok(())).unwrap().unwrap();
        assert!(out.is_empty());
        // 確定後の finish は 2 回目を返さない。
        assert!(o.finish(2, Ok(())).is_none());
    }

    #[cfg(not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "windows",
        target_os = "android"
    )))]
    #[test]
    fn unsupported_os_shows_unsupported_not_empty() {
        use fandhe_core::screens::ble_scan::{ScanPhase, start_scan};

        /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
        struct NoopWaker;
        impl fandhe_runtime::RepaintWaker for NoopWaker {
            fn wake(&self) {}
            fn wake_after(&self, _delay: std::time::Duration) {}
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let sender = fandhe_runtime::CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        let mut state = BleScanState::default();
        start_scan(&mut state);
        ble_scan_effect(&state, &sender);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(state.phase, ScanPhase::Idle);
        assert_eq!(
            state.error,
            fandhe_core::screens::ble_scan::UNSUPPORTED_ERROR
        );
        assert!(state.rows.is_empty());
    }
}
