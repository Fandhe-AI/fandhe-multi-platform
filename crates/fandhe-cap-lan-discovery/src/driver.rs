//! Phase 11 の拡充ループ（親 #109、Issue #486）型サンプル「LAN のサービス探索」の effect
//! ドライバ。`fandhe_core::screens::lan_discover`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `crate::contract`（`_http._tcp`・10 秒・上限 50 件・
//! `normalize_services`・`decode_code`）。`fandhe-cap-bluetooth-le` の driver と同型の pull 型。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # iOS / macOS / Windows / Android: pull 型 `bridge`
//!
//! 発見と解決は OS のスレッドから非同期に届くため、`LanDiscovery` トレイトを通さずコアの状態機械へ
//! 直接書き戻す（[`bridge`] は `RequestSlot` の facade）。OS 側は [`OBSERVED`]（観測バッファ）へ
//! [`Observer::observe`] でサービスを足し、窓が閉じたら [`finish`] を 1 回呼ぶ。バッファの照合・
//! 取り出しは 1 つのロックの内側で行い（`device-bridge-contracts.md` 2 節）、成功のときだけ
//! `normalize_services` を通す。失敗時は部分結果を捨てる（同 6 節）。
//!
//! # OS ごとの `present`
//!
//! 4 OS とも `present(request_id, duration) -> Result<(), LanDiscoveryError>` の同じ形で、後続の OS 実装は
//! 自分のモジュールの中身だけを埋め、本ファイルの dispatch には触らない。
//! - macOS・iOS: `apple_lan_discovery`（`nw_browser` と dns_sd。iOS は `ios_lan_discovery` が委譲）
//! - Android: `android_lan_discovery`（`NsdManager` と Java 側 `LanDiscoveryHelper`。窓は main Handler が閉じる）
//! - Windows: `windows_lan_discovery`（Win32 `DnsServiceBrowse`。ワーカーが窓の間待つ）
//!
//! # 期限・終了
//!
//! 期限は `BROWSE_DURATION_MS` だけ（OS 側へ渡して一致させる、同 3 節）。画面離脱はプロセス終了と
//! 同義のため、[`stop_shared_discovery`] を `fandhe-gallery` の終了フックから呼んで探索を止める。再要求は
//! 古いセッションを置き換え、古い request_id の結果は照合して捨てる。Android は停止が
//! Activity のライフサイクルで完結するため分岐を持たない。
//!
//! # 上記 4 OS 以外
//!
//! `UnsupportedLanDiscovery` を同期で呼び、`Unsupported` を画面へ出す（Web は対象外）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序、失敗を成功と見せないこと、探索の停止と資源の解放。
//! 守らない範囲は `fandhe_device::lan_discovery` のモジュール doc を参照。
//! サービス名・アドレスはログへ出さない（security.md A09）。

use std::collections::HashMap;
use std::sync::Mutex;

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "android"
)))]
use crate::contract::LanDiscovery;
use crate::contract::{LanService, normalize_services};
use crate::error::LanDiscoveryError;
use fandhe_core::screens::lan_discover::{
    LanDiscoverState, LanError, LanRow, finish_browse, mark_browsing, take_request,
};

use fandhe_runtime::CompletionSender;

/// 観測バッファに溜める名前数の上限。応答の洪水でメモリが膨らむのを防ぐ
/// （最終的な件数上限は `finish` 時の `normalize_services` が `MAX_SERVICES` で切る）。
// ponytail: 超えた新しい名前は捨てる。上限に当たる環境が現れたら窓の中での入れ替え方を検討する。
#[cfg_attr(
    not(any(target_os = "ios", target_os = "macos", test)),
    allow(dead_code)
)]
const MAX_OBSERVED: usize = 256;

/// 契約の `Result<Vec<LanService>, LanDiscoveryError>` を画面用へ写す（種別は 1 対 1、
/// その他は `Display` 文言のみ。内部構造は出さない、security.md A09）。
/// `Cancelled` は契約上返らないため `Other` に写す。
pub(crate) fn to_core(
    result: Result<Vec<LanService>, LanDiscoveryError>,
) -> Result<Vec<LanRow>, LanError> {
    match result {
        Ok(services) => Ok(services
            .into_iter()
            .map(|s| LanRow {
                name: s.name,
                addr: s.addr.map(|a| a.to_string()),
                port: s.port,
            })
            .collect()),
        Err(LanDiscoveryError::PermissionDenied(_)) => Err(LanError::PermissionDenied),
        Err(LanDiscoveryError::Disabled(_)) => Err(LanError::Disabled),
        Err(LanDiscoveryError::Unsupported(_)) => Err(LanError::Unsupported),
        Err(other) => Err(LanError::Other(other.to_string())),
    }
}

type Buffer = Option<(u64, HashMap<String, LanService>)>;

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

    /// サービス 1 件を足す。現在の要求のときだけ。同名の合流規則は `normalize_services` を
    /// 再利用する（規則を二重に書かない）。
    ///
    /// 呼び出し元は Apple の `apple_lan_discovery`・Windows の `windows_lan_discovery`・
    /// Android の `android_lan_discovery`。
    #[cfg_attr(
        not(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "windows",
            target_os = "android",
            test
        )),
        allow(dead_code)
    )]
    pub(crate) fn observe(&self, request_id: u64, service: LanService) {
        let mut guard = self.lock();
        let Some((id, map)) = guard.as_mut() else {
            return;
        };
        if *id != request_id {
            return;
        }
        let merged = match map.remove(&service.name) {
            Some(prev) => normalize_services([prev, service]).remove(0),
            None if map.len() >= MAX_OBSERVED => return,
            None => service,
        };
        map.insert(merged.name.clone(), merged);
    }

    /// 現在の要求の結果を確定して取り出す。要求が古い（置き換え・停止済み）なら `None`。
    /// 成功は契約の形、失敗は部分結果を捨てて `Err`。
    pub(crate) fn finish(
        &self,
        request_id: u64,
        result: Result<(), LanDiscoveryError>,
    ) -> Option<Result<Vec<LanService>, LanDiscoveryError>> {
        let mut guard = self.lock();
        if guard.as_ref().is_none_or(|(id, _)| *id != request_id) {
            return None;
        }
        let (_, map) = guard.take()?;
        drop(guard);
        Some(result.map(|()| normalize_services(map.into_values())))
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
pub(crate) fn finish(request_id: u64, result: Result<(), LanDiscoveryError>) {
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
    use crate::contract::LanService;
    use crate::error::LanDiscoveryError;

    use fandhe_runtime::request_slot::RequestSlot;

    /// OS の結果をそのまま保持する（画面用の写像は `drive_callback` が行い、画面非依存の公開入口と共有する）。
    pub(crate) type Outcome = Result<Vec<LanService>, LanDiscoveryError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// `request_id` 不一致・既に結果ありは捨てて `false`。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<Vec<LanService>, LanDiscoveryError>,
    ) -> bool {
        SLOT.deliver(request_id, result)
    }
}

/// iOS / macOS / Windows / Android の effect ドライバ本体（`bluetooth_le::drive_callback` と同型）。
#[cfg(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "android"
))]
fn drive_callback(state: &LanDiscoverState, sender: &CompletionSender<LanDiscoverState>) {
    use fandhe_core::screens::lan_discover::LanPhase;
    use std::time::Duration;

    if let Some(request) = take_request(state) {
        let _ = sender.send(Box::new(mark_browsing));

        // 先行（公開入口など）を置き換えず、画面には失敗として返す。
        let Some(request_id) = SCREEN_OWNER.begin_excluding(&bridge::SLOT, &[&PUBLIC_OWNER]) else {
            let generation = request.generation;
            let _ = sender.send(Box::new(move |s: &mut LanDiscoverState| {
                finish_browse(
                    s,
                    generation,
                    Err(LanError::Other(
                        "他の探索が処理中です。完了後にもう一度お試しください".to_string(),
                    )),
                );
            }));
            return;
        };
        OBSERVED.begin(request_id);
        let duration = Duration::from_millis(crate::contract::BROWSE_DURATION_MS);
        #[cfg(target_os = "macos")]
        let started = crate::apple::present(request_id, duration);
        #[cfg(target_os = "ios")]
        let started = crate::ios::present(request_id, duration);
        #[cfg(target_os = "android")]
        let started = crate::android::present(request_id, duration);
        #[cfg(target_os = "windows")]
        let started = crate::windows::present(request_id, duration);
        if let Err(e) = started {
            // 依頼自体が同期的に失敗した（コールバックは二度と来ない）。
            finish(request_id, Err(e));
        }
        return;
    }

    if state.phase != LanPhase::Browsing {
        return;
    }
    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &SCREEN_OWNER,
        sender,
        |_request_id| {},
        move |s: &mut LanDiscoverState, outcome| {
            // 反映できる状態か確かめてから `finish_browse` し、反映できたときだけ
            // `true` を返す（`RequestSlot` 側がそれを見て `clear` する）。
            if s.phase != LanPhase::Idle {
                finish_browse(s, generation, to_core(outcome));
                true
            } else {
                false
            }
        },
        move |s: &mut LanDiscoverState| {
            finish_browse(
                s,
                generation,
                Err(LanError::Other(
                    "探索が中断されました。もう一度お試しください".to_string(),
                )),
            );
        },
    );
}

/// 上記 4 OS 以外向け: `LanDiscovery` トレイト経由の同期呼び出し。
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "android"
)))]
fn drive(
    state: &LanDiscoverState,
    sender: &CompletionSender<LanDiscoverState>,
    lan: &dyn LanDiscovery,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_browsing));
    let outcome = to_core(lan.browse());
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut LanDiscoverState| {
        finish_browse(s, generation, outcome);
    }));
}

/// `registry.rs` の `effect` 区画から `lan_discover` 画面用に配線される effect ドライバ。
pub fn lan_discover_effect(state: &LanDiscoverState, sender: &CompletionSender<LanDiscoverState>) {
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
    drive(state, sender, &crate::contract::UnsupportedLanDiscovery);
}

/// Activity 再生成時に旧要求の保留を捨てる（`fandhe-gallery` の `android_main` から呼ばれる。
/// Java 側の追跡は Activity インスタンスのフィールドで、再生成で一緒に消える）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::SLOT.reset();
    OBSERVED.clear();
}

/// 画面の状態型に依存しない入口。LAN 探索は結果が OS のスレッドから非同期に届く（`OBSERVED` の観測と
/// `finish` の 1 回の完了通知）ため、結果が届くまで塞ぐ同期の [`LanDiscovery`] 実装は提供しない。
/// 画面の外のアプリが探索するなら [`begin_lan_discovery`] / [`poll_lan_discovery`] を使う。ここは同期 trait 経由の呼び出しに
/// 対して「未対応」を正直に返す（成功の 0 件にしない）。
// ponytail: 同期 trait の実装は要求が出たら足す（`OBSERVED.finish` を待つワーカーが要る）。
pub fn platform_lan_discovery() -> Box<dyn crate::contract::LanDiscovery> {
    Box::new(crate::contract::UnsupportedLanDiscovery)
}

#[cfg(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "android"
))]
/// 公開入口 [`begin_lan_discovery`] が追う自分の分の要求（画面側が置き換え・横取りしない）。
static PUBLIC_OWNER: fandhe_runtime::request_slot::SlotOwner =
    fandhe_runtime::request_slot::SlotOwner::new();

#[cfg(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "android"
))]
/// 画面 effect が追う自分の分の要求（公開入口と slot を共有するため区別する）。
static SCREEN_OWNER: fandhe_runtime::request_slot::SlotOwner =
    fandhe_runtime::request_slot::SlotOwner::new();

/// 探索を OS へ依頼し、結果を [`poll_lan_discovery`] で受け取るための要求 id を返す
/// （iOS・macOS・Windows・Android）。
///
/// 画面の状態型に依存しない非同期の入口で、フレームワークの外のアプリも使える。1 プロセスに 1 要求で、
/// 進行中、または結果が未取得の要求があるうちは `Internal`（busy）で断る。依頼自体の失敗は `Err`（結果は届かない）。
/// それ以外の OS は `Unsupported`。
#[cfg(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "android"
))]
pub fn begin_lan_discovery() -> Result<u64, LanDiscoveryError> {
    let duration = std::time::Duration::from_millis(crate::contract::BROWSE_DURATION_MS);
    let request_id = PUBLIC_OWNER
        .begin_excluding(&bridge::SLOT, &[&SCREEN_OWNER, &PUBLIC_OWNER])
        .ok_or(LanDiscoveryError::Internal("lan_discovery busy"))?;
    OBSERVED.begin(request_id);
    #[cfg(target_os = "macos")]
    let started = crate::apple::present(request_id, duration);
    #[cfg(target_os = "ios")]
    let started = crate::ios::present(request_id, duration);
    #[cfg(target_os = "android")]
    let started = crate::android::present(request_id, duration);
    #[cfg(target_os = "windows")]
    let started = crate::windows::present(request_id, duration);
    if let Err(e) = started {
        OBSERVED.clear();
        bridge::SLOT.clear(request_id);
        PUBLIC_OWNER.release(request_id);
        return Err(e);
    }
    Ok(request_id)
}

/// 非対応 OS では成功へ倒さず `Unsupported`。
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "android"
)))]
pub fn begin_lan_discovery() -> Result<u64, LanDiscoveryError> {
    Err(LanDiscoveryError::Unsupported("lan_discovery"))
}

/// [`begin_lan_discovery`] の結果を取り出す。窓が閉じて届いていれば 1 度だけ `Some`、未着・旧 id・取り出し済みは `None`。
pub fn poll_lan_discovery(request_id: u64) -> Option<Result<Vec<LanService>, LanDiscoveryError>> {
    #[cfg(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "windows",
        target_os = "android"
    ))]
    {
        let (id, ready) = bridge::SLOT.snapshot()?;
        if id != request_id {
            return None;
        }
        let result = ready?;
        bridge::SLOT.clear(id);
        PUBLIC_OWNER.release(id);
        Some(result)
    }
    #[cfg(not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        let _ = request_id;
        None
    }
}

/// アプリ終了時に進行中の探索を止める（`fandhe-gallery` の終了フック（`shutdown()`）から呼ばれる）。
/// Rust 側のロックを握ったまま OS 側の停止を呼ばない（OS 側コールバックが同じロックを取るため、
/// `device-bridge-contracts.md` 11 節）。
pub fn stop_shared_discovery() {
    OBSERVED.clear();
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    crate::apple::stop();
    #[cfg(target_os = "windows")]
    crate::windows::stop_shared();
}

#[cfg(test)]
mod tests {
    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    #[allow(dead_code)] // 非対応 OS の同期経路のテストだけが使う
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    use super::*;
    use crate::contract::{
        LanDiscovery, MAX_SERVICES, UnsupportedLanDiscovery, decode_code, parse_addr,
    };
    use fandhe_core::screens::lan_discover::{LanPhase, start_browse};

    fn svc(name: &str, addr: Option<&str>, port: Option<u16>) -> LanService {
        LanService {
            name: name.into(),
            addr: addr.and_then(parse_addr),
            port,
        }
    }

    #[test]
    fn to_core_maps_each_error_kind_distinctly() {
        assert_eq!(
            to_core(Err(LanDiscoveryError::PermissionDenied("x"))),
            Err(LanError::PermissionDenied)
        );
        assert_eq!(
            to_core(Err(LanDiscoveryError::Disabled("x"))),
            Err(LanError::Disabled)
        );
        assert_eq!(
            to_core(Err(LanDiscoveryError::Unsupported("x"))),
            Err(LanError::Unsupported)
        );
        assert!(matches!(
            to_core(Err(LanDiscoveryError::Cancelled("x"))),
            Err(LanError::Other(_))
        ));
        assert!(matches!(
            to_core(Err(LanDiscoveryError::Internal("x"))),
            Err(LanError::Other(_))
        ));
        assert_eq!(to_core(Ok(vec![])), Ok(vec![]));
    }

    #[test]
    fn policy_denied_code_becomes_permission_denied_screen_error() {
        let r = to_core(decode_code(1).map(|()| vec![]));
        assert_eq!(r, Err(LanError::PermissionDenied));
    }

    #[test]
    fn unsupported_is_error_not_empty_success() {
        assert_eq!(
            to_core(UnsupportedLanDiscovery.browse()),
            Err(LanError::Unsupported)
        );
    }

    #[test]
    fn unresolved_rows_stay_in_successful_result() {
        let o = Observer::new();
        o.begin(1);
        o.observe(1, svc("b", None, None));
        o.observe(1, svc("a", Some("10.0.0.2"), Some(80)));
        let out = to_core(o.finish(1, Ok(())).unwrap()).unwrap();
        assert_eq!(out[0].addr.as_deref(), Some("10.0.0.2"));
        assert_eq!(out[1].name, "b");
        assert_eq!((out[1].addr.clone(), out[1].port), (None, None));
    }

    #[test]
    fn observer_merges_and_does_not_erase_with_none() {
        let o = Observer::new();
        o.begin(1);
        o.observe(1, svc("a", Some("10.0.0.2"), Some(80)));
        o.observe(1, svc("a", None, None));
        let out = o.finish(1, Ok(())).unwrap().unwrap();
        assert_eq!(out, vec![svc("a", Some("10.0.0.2"), Some(80))]);
    }

    #[test]
    fn observer_failure_drops_partial_result() {
        let o = Observer::new();
        o.begin(1);
        o.observe(1, svc("a", None, None));
        let r = o.finish(1, Err(LanDiscoveryError::Disabled("x"))).unwrap();
        assert!(matches!(r, Err(LanDiscoveryError::Disabled(_))));
        assert_eq!(
            to_core(r),
            Err(LanError::Disabled),
            "失敗の完了で部分結果は残らない"
        );
    }

    #[test]
    fn observer_ignores_stale_request_ids_and_double_finish() {
        let o = Observer::new();
        o.begin(2);
        o.observe(1, svc("old", None, None));
        assert!(o.finish(1, Ok(())).is_none());
        let out = o.finish(2, Ok(())).unwrap().unwrap();
        assert!(out.is_empty());
        assert!(o.finish(2, Ok(())).is_none());
    }

    #[test]
    fn observer_caps_buffer_and_result() {
        let o = Observer::new();
        o.begin(1);
        for i in 0..MAX_OBSERVED + 10 {
            o.observe(1, svc(&format!("{i:04}"), None, None));
        }
        let out = o.finish(1, Ok(())).unwrap().unwrap();
        assert_eq!(out.len(), MAX_SERVICES);
    }

    #[test]
    fn stale_generation_completion_does_not_change_state() {
        let mut s = LanDiscoverState::default();
        start_browse(&mut s);
        mark_browsing(&mut s);
        let stale = s.generation + 1;
        finish_browse(&mut s, stale, Ok(vec![]));
        assert_eq!(s.phase, LanPhase::Browsing);
    }

    #[cfg(not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "windows",
        target_os = "android"
    )))]
    #[test]
    fn unsupported_os_shows_unsupported_not_empty() {
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        let mut state = LanDiscoverState::default();
        start_browse(&mut state);
        lan_discover_effect(&state, &sender);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(state.phase, LanPhase::Idle);
        assert_eq!(
            state.error,
            fandhe_core::screens::lan_discover::UNSUPPORTED_ERROR
        );
        assert!(state.rows.is_empty());
    }
}
