//! Phase 11 の拡充ループ（親 #110、Issue #511）型サンプル「近くの端末との直接通信」の effect
//! ドライバ。`fandhe_core::screens::peer_connect`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `fandhe_device::local_peer`（30 秒の窓・200 バイト・`sanitize_received`・
//! `should_invite`・`decode_code`）。`device/lan_discovery.rs` と同型の pull 型。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # iOS / macOS / Android: pull 型 `bridge`
//!
//! 相手の発見と受信は OS のスレッドから非同期に届くため、`LocalPeer` トレイトを通さずコアの状態機械へ
//! 直接書き戻す（[`bridge`] は `RequestSlot` の facade）。OS 側は [`PENDING`] へ [`Pending::offer`] で
//! 受け取った文字列を足し、窓が閉じたら [`finish`] を 1 回呼ぶ。照合と取り出しは 1 つのロックの
//! 内側で行い（`device-bridge-contracts.md` 2 節）、失敗時は受け取り済みの結果を捨てる（同 6 節）。
//!
//! # OS ごとの `present`
//!
//! 3 OS とも `present(request_id, own_name, outgoing, window) -> Result<(), DeviceError>` の同じ形で、
//! 後続の OS 実装は自分のモジュールの中身だけを埋め、本ファイルの dispatch には触らない。
//! - macOS: `apple_local_peer`（MultipeerConnectivity の `.m`）
//! - iOS: `ios_local_peer`（b-ios が `apple_local_peer` へ委譲するまでは `Unsupported`）
//! - Android: `android_local_peer`（b-android が埋めるまでは `Unsupported`）
//!
//! # 上記以外
//!
//! `UnsupportedLocalPeer` を同期で呼び、`Unsupported` を画面へ出す（Windows は保留、Web は対象外）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序、失敗を成功と見せないこと、窓の終了時の資源の解放。
//! 守らない範囲は `fandhe_device::local_peer` のモジュール doc を参照。
//! 表示名と受け取った文字列はログへ出さない（security.md A09）。
#![cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "android", test)),
    allow(dead_code)
)]

use std::sync::Mutex;

use fandhe_core::screens::peer_connect::{
    finish_exchange, mark_exchanging, take_request, PeerConnectState, PeerError, PeerRow,
};
#[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "android")))]
use fandhe_device::local_peer::LocalPeer;
use fandhe_device::local_peer::{is_valid_peer_name, sanitize_received, PeerExchange};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 契約の `Result<Option<PeerExchange>, DeviceError>` を画面用へ写す（種別は 1 対 1、
/// その他は `Display` 文言のみ。内部構造は出さない、security.md A09）。
/// `Cancelled` は契約上返らないため `Other` に写す。
pub(crate) fn to_core(
    result: Result<Option<PeerExchange>, DeviceError>,
) -> Result<Option<PeerRow>, PeerError> {
    match result {
        Ok(found) => Ok(found.map(|p| PeerRow {
            peer_name: p.peer_name,
            received: p.received,
        })),
        Err(DeviceError::PermissionDenied(_)) => Err(PeerError::PermissionDenied),
        Err(DeviceError::Disabled(_)) => Err(PeerError::Disabled),
        Err(DeviceError::Unsupported(_)) => Err(PeerError::Unsupported),
        Err(other) => Err(PeerError::Other(other.to_string())),
    }
}

type Slot = Option<(u64, Option<PeerExchange>)>;

/// 受信バッファ（1 件版の観測バッファ）。request_id ごとに作り直し、照合と取り出しを同じロックで行う。
pub(crate) struct Pending {
    inner: Mutex<Slot>,
}

impl Pending {
    pub(crate) const fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 新しい要求の受信を始める（前の要求のバッファは捨てる）。
    pub(crate) fn begin(&self, request_id: u64) {
        *self.lock() = Some((request_id, None));
    }

    /// 相手の名前と受信データを足す。現在の要求で、まだ受け取っておらず、名前とデータの検証を
    /// 通ったときだけ保存して `true`。`false` のとき OS 側は待ち続けてよい（不正な入力は捨てる）。
    // Android は b-android の JNI が呼ぶまで未使用（そのとき外す）。
    #[cfg_attr(target_os = "android", allow(dead_code))]
    pub(crate) fn offer(&self, request_id: u64, peer_name: &str, bytes: &[u8]) -> bool {
        if !is_valid_peer_name(peer_name) {
            return false;
        }
        let Some(received) = sanitize_received(bytes) else {
            return false;
        };
        let mut guard = self.lock();
        match guard.as_mut() {
            Some((id, got)) if *id == request_id && got.is_none() => {
                *got = Some(PeerExchange {
                    peer_name: peer_name.to_string(),
                    received,
                });
                true
            }
            _ => false,
        }
    }

    /// 現在の要求の結果を確定して取り出す。要求が古い（置き換え・停止済み）・二重の完了なら `None`。
    /// 失敗は受け取り済みの結果を捨てて `Err`。
    pub(crate) fn finish(
        &self,
        request_id: u64,
        result: Result<(), DeviceError>,
    ) -> Option<Result<Option<PeerExchange>, DeviceError>> {
        let mut guard = self.lock();
        if guard.as_ref().is_none_or(|(id, _)| *id != request_id) {
            return None;
        }
        let (_, got) = guard.take()?;
        drop(guard);
        Some(result.map(|()| got))
    }

    /// バッファを空にする（終了時）。
    pub(crate) fn clear(&self) {
        *self.lock() = None;
    }
}

/// iOS / macOS / Android 共通の受信バッファ（OS 側コールバックが書く）。
pub(crate) static PENDING: Pending = Pending::new();

#[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
/// OS 側の完了通知。バッファを確定して [`bridge`] へ置く（古い要求は捨てる）。
pub(crate) fn finish(request_id: u64, result: Result<(), DeviceError>) {
    if let Some(r) = PENDING.finish(request_id, result) {
        bridge::deliver(request_id, r);
    }
}

#[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
/// OS からのコールバックとの橋渡し（pull 型、`RequestSlot` の facade）。
pub(crate) mod bridge {
    use fandhe_core::screens::peer_connect::{PeerError, PeerRow};
    use fandhe_device::local_peer::PeerExchange;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<Option<PeerRow>, PeerError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// `request_id` 不一致・既に結果ありは捨てて `false`。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<Option<PeerExchange>, DeviceError>,
    ) -> bool {
        SLOT.deliver(request_id, super::to_core(result))
    }
}

/// 表示名・送信文字列の種に使う乱数（`RandomState` は std で、新しい依存を足さない。
/// 表示用のトークンで秘密値ではない）。
#[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
fn random_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

/// iOS / macOS / Android の effect ドライバ本体（`lan_discovery::drive_callback` と同型）。
#[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
fn drive_callback(state: &PeerConnectState, sender: &CompletionSender<PeerConnectState>) {
    use fandhe_core::screens::peer_connect::PeerPhase;
    use fandhe_device::local_peer::{outgoing_text, peer_token, EXCHANGE_WINDOW_MS};
    use std::time::Duration;

    if take_request(state).is_some() {
        let seed = random_seed();
        let own_name = peer_token(seed);
        #[cfg(target_os = "macos")]
        let os = "macOS";
        #[cfg(target_os = "ios")]
        let os = "iOS";
        #[cfg(target_os = "android")]
        let os = "Android";
        let outgoing = outgoing_text(os, seed);
        let label = own_name.clone();
        let _ = sender.send(Box::new(move |s: &mut PeerConnectState| {
            mark_exchanging(s, &label)
        }));

        let request_id = bridge::begin();
        PENDING.begin(request_id);
        let window = Duration::from_millis(EXCHANGE_WINDOW_MS);
        #[cfg(target_os = "macos")]
        let started = super::apple_local_peer::present(request_id, &own_name, &outgoing, window);
        #[cfg(target_os = "ios")]
        let started = super::ios_local_peer::present(request_id, &own_name, &outgoing, window);
        #[cfg(target_os = "android")]
        let started = super::android_local_peer::present(request_id, &own_name, &outgoing, window);
        if let Err(e) = started {
            // 依頼自体が同期的に失敗した（コールバックは二度と来ない）。
            finish(request_id, Err(e));
        }
        return;
    }

    if state.phase != PeerPhase::Exchanging {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut PeerConnectState, outcome| {
            // 反映できる状態か確かめてから `finish_exchange` し、反映できたときだけ
            // `true` を返す（`RequestSlot` 側がそれを見て `clear` する）。
            if s.phase != PeerPhase::Idle {
                finish_exchange(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// 上記 3 OS 以外向け: `LocalPeer` トレイト経由の同期呼び出し。
#[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "android")))]
fn drive(
    state: &PeerConnectState,
    sender: &CompletionSender<PeerConnectState>,
    peer: &dyn LocalPeer,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(|s: &mut PeerConnectState| mark_exchanging(s, "-")));
    let outcome = to_core(peer.exchange(""));
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut PeerConnectState| {
        finish_exchange(s, generation, outcome);
    }));
}

/// `registry.rs` の `effect` 区画から `peer_connect` 画面用に配線される effect ドライバ。
pub fn peer_connect_effect(state: &PeerConnectState, sender: &CompletionSender<PeerConnectState>) {
    #[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
    drive_callback(state, sender);
    #[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "android")))]
    drive(
        state,
        sender,
        &fandhe_device::local_peer::UnsupportedLocalPeer,
    );
}

/// Activity 再生成時に旧要求の保留を捨てる（`android.rs::android_main` から呼ばれる。
/// Java 側の追跡は Activity インスタンスのフィールドで、再生成で一緒に消える）。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    bridge::SLOT.reset();
    PENDING.clear();
}

/// アプリ終了時に進行中の交換を止める（`device::shutdown` から呼ばれる）。
/// Rust 側のロックを握ったまま OS 側の停止を呼ばない（OS 側コールバックが同じロックを取るため、
/// `device-bridge-contracts.md` 11 節）。`PENDING.clear()` はロックを解放して戻ってから停止を呼ぶ。
pub fn stop_shared_exchange() {
    PENDING.clear();
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    super::apple_local_peer::stop();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::peer_connect::{start_exchange, PeerPhase};
    use fandhe_device::local_peer::{decode_code, peer_token, UnsupportedLocalPeer};

    const A: &str = "fandhe-0000000a";

    #[test]
    fn to_core_maps_each_error_kind_distinctly() {
        assert_eq!(
            to_core(Err(DeviceError::PermissionDenied("x"))),
            Err(PeerError::PermissionDenied)
        );
        assert_eq!(
            to_core(Err(DeviceError::Disabled("x"))),
            Err(PeerError::Disabled)
        );
        assert_eq!(
            to_core(Err(DeviceError::Unsupported("x"))),
            Err(PeerError::Unsupported)
        );
        assert!(matches!(
            to_core(Err(DeviceError::Cancelled("x"))),
            Err(PeerError::Other(_))
        ));
        assert!(matches!(
            to_core(Err(DeviceError::Internal("x"))),
            Err(PeerError::Other(_))
        ));
        assert_eq!(to_core(Ok(None)), Ok(None));
    }

    #[test]
    fn denied_code_becomes_permission_denied_screen_error() {
        assert_eq!(
            to_core(decode_code(1).map(|()| None)),
            Err(PeerError::PermissionDenied)
        );
    }

    #[test]
    fn unsupported_is_error_not_empty_success() {
        use fandhe_device::local_peer::LocalPeer;
        assert_eq!(
            to_core(UnsupportedLocalPeer.exchange("x")),
            Err(PeerError::Unsupported)
        );
    }

    #[test]
    fn nothing_received_is_ok_none() {
        let p = Pending::new();
        p.begin(1);
        assert_eq!(p.finish(1, Ok(())).unwrap().unwrap(), None);
    }

    #[test]
    fn offer_then_finish_returns_exchange() {
        let p = Pending::new();
        p.begin(1);
        assert!(p.offer(1, A, b"hello"));
        let got = p.finish(1, Ok(())).unwrap().unwrap().unwrap();
        assert_eq!(
            (got.peer_name.as_str(), got.received.as_str()),
            (A, "hello")
        );
    }

    #[test]
    fn stale_ids_and_double_finish_are_dropped() {
        let p = Pending::new();
        p.begin(2);
        assert!(!p.offer(1, A, b"old"));
        assert!(p.finish(1, Ok(())).is_none());
        assert!(p.finish(2, Ok(())).is_some());
        assert!(p.finish(2, Ok(())).is_none());
        assert!(!p.offer(2, A, b"late"));
    }

    #[test]
    fn failure_drops_received_result() {
        let p = Pending::new();
        p.begin(1);
        assert!(p.offer(1, A, b"hello"));
        let r = p.finish(1, Err(DeviceError::Disabled("x"))).unwrap();
        assert_eq!(to_core(r), Err(PeerError::Disabled));
    }

    #[test]
    fn invalid_offer_is_rejected_and_next_valid_one_accepted() {
        let p = Pending::new();
        p.begin(1);
        assert!(!p.offer(1, "evil\nname", b"hello"));
        assert!(!p.offer(1, A, &[0xff]));
        assert!(!p.offer(1, A, &[b'a'; 201]));
        assert!(p.offer(1, &peer_token(7), b"ok"));
    }

    #[test]
    fn second_offer_is_ignored() {
        let p = Pending::new();
        p.begin(1);
        assert!(p.offer(1, A, b"first"));
        assert!(!p.offer(1, A, b"second"));
        let got = p.finish(1, Ok(())).unwrap().unwrap().unwrap();
        assert_eq!(got.received, "first");
    }

    #[test]
    fn stale_generation_completion_does_not_change_state() {
        let mut s = PeerConnectState::default();
        start_exchange(&mut s);
        mark_exchanging(&mut s, A);
        let stale = s.generation + 1;
        finish_exchange(&mut s, stale, Ok(None));
        assert_eq!(s.phase, PeerPhase::Exchanging);
    }

    #[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "android")))]
    #[test]
    fn unsupported_os_shows_unsupported_not_empty() {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, ctx);
        let mut state = PeerConnectState::default();
        start_exchange(&mut state);
        peer_connect_effect(&state, &sender);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(state.phase, PeerPhase::Idle);
        assert_eq!(
            state.error,
            fandhe_core::screens::peer_connect::UNSUPPORTED_ERROR
        );
        assert!(state.row.is_none());
    }
}
