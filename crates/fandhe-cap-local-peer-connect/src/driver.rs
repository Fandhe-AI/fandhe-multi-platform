//! Phase 11 の拡充ループ（親 #110、Issue #511）型サンプル「近くの端末との直接通信」の effect
//! ドライバ。`fandhe_core::screens::peer_connect`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `contract.rs`（30 秒の窓・200 バイト・`sanitize_received`・
//! `should_invite`・`decode_code`）。`fandhe-cap-bluetooth-le` などと同型の pull 型。
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
//! 3 OS とも `present(request_id, own_name, outgoing, window) -> Result<(), LocalPeerConnectError>` の同じ形で、
//! 後続の OS 実装は自分のモジュールの中身だけを埋め、本ファイルの dispatch には触らない。
//! - macOS: `apple.rs`（MultipeerConnectivity の `.m`）
//! - iOS: `ios.rs`（macOS と共用の `apple.rs` へ委譲する）
//! - Android: `android.rs`（`LocalPeerHelper`、Wi-Fi Aware）
//!
//! # 上記以外
//!
//! `UnsupportedLocalPeer` を同期で呼び、`Unsupported` を画面へ出す（Windows は保留、Web は対象外）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序、失敗を成功と見せないこと、窓の終了時の資源の解放。
//! 守らない範囲は `contract.rs` のモジュール doc を参照。
//! 表示名と受け取った文字列はログへ出さない（security.md A09）。
#![cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "android", test)),
    allow(dead_code)
)]

use std::sync::Mutex;

use crate::contract::LocalPeer;
use crate::contract::{PeerExchange, is_valid_peer_name, sanitize_received};
use crate::error::LocalPeerConnectError;
use fandhe_core::screens::peer_connect::{
    PeerConnectState, PeerError, PeerRow, finish_exchange, mark_exchanging, take_request,
};

use fandhe_runtime::CompletionSender;

/// 現在のプラットフォームに応じた `LocalPeer` 実装を返す。
///
/// iOS・macOS・Android は OS のコールバックが非同期に届くため `LocalPeer` を実装せず、この trait 経由は
/// どの OS でも `Unsupported` 固定を返す（Windows は保留、Web は対象外）。
/// 実際の交換は [`start_exchange`] と [`poll_exchange`] の組で行う。
/// 画面の状態型に依存しない入口（`docs/architecture/consumer-app-structure.md` §8.2）。
pub fn platform_local_peer_connect() -> std::sync::Arc<dyn LocalPeer> {
    register_teardown();
    std::sync::Arc::new(crate::contract::UnsupportedLocalPeer)
}

/// 契約の `Result<Option<PeerExchange>, LocalPeerConnectError>` を画面用へ写す（種別は 1 対 1、
/// その他は `Display` 文言のみ。内部構造は出さない、security.md A09）。
/// `Cancelled` は契約上返らないため `Other` に写す。
pub(crate) fn to_core(
    result: Result<Option<PeerExchange>, LocalPeerConnectError>,
) -> Result<Option<PeerRow>, PeerError> {
    match result {
        Ok(found) => Ok(found.map(|p| PeerRow {
            peer_name: p.peer_name,
            received: p.received,
        })),
        Err(LocalPeerConnectError::PermissionDenied(_)) => Err(PeerError::PermissionDenied),
        Err(LocalPeerConnectError::Disabled(_)) => Err(PeerError::Disabled),
        Err(LocalPeerConnectError::Unsupported(_)) => Err(PeerError::Unsupported),
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
        result: Result<(), LocalPeerConnectError>,
    ) -> Option<Result<Option<PeerExchange>, LocalPeerConnectError>> {
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
pub(crate) fn finish(request_id: u64, result: Result<(), LocalPeerConnectError>) {
    if let Some(r) = PENDING.finish(request_id, result) {
        bridge::deliver(request_id, r);
    }
}

/// OS からのコールバックとの橋渡し（pull 型、`RequestSlot` の facade）。
/// 結果は画面にも公開入口にも使えるよう契約の型のまま置き、画面用への写像（[`to_core`]）は反映側が行う。
pub(crate) mod bridge {
    use crate::contract::PeerExchange;
    use crate::error::LocalPeerConnectError;

    use fandhe_runtime::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<Option<PeerExchange>, LocalPeerConnectError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// `request_id` 不一致・既に結果ありは捨てて `false`。
    #[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
    pub(crate) fn deliver(request_id: u64, result: Outcome) -> bool {
        SLOT.deliver(request_id, result)
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

/// 受信バッファを `request_id` で始め、OS へ交換開始を依頼して自分の表示名を返す。
/// 依頼が同期で失敗したら（コールバックは二度と来ない）その場で失敗を確定する。
#[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
fn launch(request_id: u64) -> String {
    use crate::contract::{EXCHANGE_WINDOW_MS, outgoing_text, peer_token};
    use std::time::Duration;

    let seed = random_seed();
    let own_name = peer_token(seed);
    #[cfg(target_os = "macos")]
    let os = "macOS";
    #[cfg(target_os = "ios")]
    let os = "iOS";
    #[cfg(target_os = "android")]
    let os = "Android";
    let outgoing = outgoing_text(os, seed);
    PENDING.begin(request_id);
    let window = Duration::from_millis(EXCHANGE_WINDOW_MS);
    #[cfg(target_os = "macos")]
    let started = crate::apple::present(request_id, &own_name, &outgoing, window);
    #[cfg(target_os = "ios")]
    let started = crate::ios::present(request_id, &own_name, &outgoing, window);
    #[cfg(target_os = "android")]
    let started = crate::android::present(request_id, &own_name, &outgoing, window);
    if let Err(e) = started {
        finish(request_id, Err(e));
    }
    own_name
}

/// フレームワークの外のアプリ用: 近くの端末との交換を始め、要求 id を返す。
/// 進行中、または結果が未取得の要求があれば `None`（置き換えない）。対象外 OS・同期の失敗も id を返し、失敗は [`poll_exchange`] で受け取る。
/// 画面用の effect（[`peer_connect_effect`]）と同じ slot を使うが、持ち主（`SlotOwner`）で分け、互いの要求を置き換えも横取りもしない（先行があれば後からの側が失敗または `None`）。
pub fn start_exchange() -> Option<u64> {
    register_teardown();
    let request_id = bridge::SLOT.try_begin_unless(|_| true)?;
    #[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
    let _ = launch(request_id);
    #[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "android")))]
    bridge::SLOT.deliver(
        request_id,
        Err(LocalPeerConnectError::Unsupported("local_peer")),
    );
    Some(request_id)
}

/// [`start_exchange`] の結果を取り出す。未着・別の id・取り出し済みは `None`。
/// 取り出すと要求は終わり、次の [`start_exchange`] を受け付ける。契約は [`LocalPeer::exchange`] と同じ
/// （相手が居なければ `Ok(None)`、失敗時は受け取り済みの結果も捨てる）。
pub fn poll_exchange(
    request_id: u64,
) -> Option<Result<Option<PeerExchange>, LocalPeerConnectError>> {
    register_teardown();
    bridge::SLOT.take_ready(request_id)
}

/// 画面側の要求の持ち主印（公開入口の要求と同じ [`bridge::SLOT`] を共有するため、自分の分だけを追う）。
#[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
static SCREEN_OWNER: fandhe_runtime::request_slot::SlotOwner =
    fandhe_runtime::request_slot::SlotOwner::new();

/// iOS / macOS / Android の effect ドライバ本体（`lan_discovery::drive_callback` と同型）。
/// 公開入口の要求が進行中・未取得なら置き換えず、画面へ失敗を返す。
#[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
fn drive_callback(state: &PeerConnectState, sender: &CompletionSender<PeerConnectState>) {
    use fandhe_core::screens::peer_connect::PeerPhase;

    if let Some(request) = take_request(state) {
        let Some(request_id) = SCREEN_OWNER.begin_keeping_foreign(&bridge::SLOT) else {
            let generation = request.generation;
            let _ = sender.send(Box::new(move |s: &mut PeerConnectState| {
                mark_exchanging(s, "-");
                finish_exchange(s, generation, Err(PeerError::Other("busy".into())));
            }));
            return;
        };
        let label = launch(request_id);
        let _ = sender.send(Box::new(move |s: &mut PeerConnectState| {
            mark_exchanging(s, &label)
        }));
        return;
    }

    if state.phase != PeerPhase::Exchanging {
        return;
    }
    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &SCREEN_OWNER,
        sender,
        |_request_id| {},
        move |s: &mut PeerConnectState, outcome| {
            // 反映できる状態か確かめてから `finish_exchange` し、反映できたときだけ
            // `true` を返す（`RequestSlot` 側がそれを見て `clear` する）。
            if s.phase != PeerPhase::Idle {
                finish_exchange(s, generation, to_core(outcome));
                true
            } else {
                false
            }
        },
        // Activity 再生成・公開入口で自分の要求が失われた。処理中のまま止めない。
        move |s: &mut PeerConnectState| {
            finish_exchange(s, generation, Err(PeerError::Other("cancelled".into())));
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
    register_teardown();
    #[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
    drive_callback(state, sender);
    #[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "android")))]
    drive(state, sender, platform_local_peer_connect().as_ref());
}

/// Activity 再生成時に旧要求の保留を捨てる（`android_main` が回す登録表（`fandhe_runtime::teardown`）から呼ばれる。
/// Java 側の追跡は Activity インスタンスのフィールドで、再生成で一緒に消える）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::SLOT.reset();
    PENDING.clear();
}

/// アプリ終了時に進行中の交換を止める（終了処理の登録表（`fandhe_runtime::teardown`）が呼ぶ）。
/// Rust 側のロックを握ったまま OS 側の停止を呼ばない（OS 側コールバックが同じロックを取るため、
/// `device-bridge-contracts.md` 11 節）。`PENDING.clear()` はロックを解放して戻ってから停止を呼ぶ。
pub fn shutdown() {
    PENDING.clear();
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    crate::apple::stop();
}

/// 終了処理と Android の再生成時の破棄を登録表（`fandhe_runtime::teardown`）へ登録する（Issue #1453）。
/// 公開の入口・effect・公開型のメソッドの先頭で呼ぶ。初めて使われたときに登録され、2 回目以降は何もしない。
pub(crate) fn register_teardown() {
    let t = fandhe_runtime::teardown::Teardown::new("local-peer-connect").on_exit(crate::shutdown);
    #[cfg(target_os = "android")]
    let t = t.on_android_recreate(crate::reset_pending_tracking);
    fandhe_runtime::teardown::register(t);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{UnsupportedLocalPeer, decode_code, peer_token};
    use fandhe_core::screens::peer_connect::{PeerPhase, start_exchange};

    const A: &str = "fandhe-0000000a";

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    #[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "android")))]
    struct NoopWaker;
    #[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "android")))]
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    #[test]
    fn to_core_maps_each_error_kind_distinctly() {
        assert_eq!(
            to_core(Err(LocalPeerConnectError::PermissionDenied("x"))),
            Err(PeerError::PermissionDenied)
        );
        assert_eq!(
            to_core(Err(LocalPeerConnectError::Disabled("x"))),
            Err(PeerError::Disabled)
        );
        assert_eq!(
            to_core(Err(LocalPeerConnectError::Unsupported("x"))),
            Err(PeerError::Unsupported)
        );
        assert!(matches!(
            to_core(Err(LocalPeerConnectError::Cancelled("x"))),
            Err(PeerError::Other(_))
        ));
        assert!(matches!(
            to_core(Err(LocalPeerConnectError::Internal("x"))),
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
        use crate::contract::LocalPeer;
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
        let r = p
            .finish(1, Err(LocalPeerConnectError::Disabled("x")))
            .unwrap();
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
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
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
