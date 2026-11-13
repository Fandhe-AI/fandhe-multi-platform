//! NFC カードエミュレーション（HCE、nfc-card-emulation）の機能 crate（REQ-21 / TASK-21.3、Issue #1172）。
//!
//! 契約（[`CardEmulation`]・[`HceStatus`]・[`AID`]・[`CardEmulationError`]・[`UnsupportedCardEmulation`]）、
//! Android 実装（JNI 経由で `NfcCardEmulationHelper`）、画面の状態機械
//! （`fandhe_core::screens::nfc_card_emulation_show`）へ橋渡しする effect ドライバ
//! （[`nfc_card_emulation_show_effect`]）、機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）と
//! Android の Java・res（`android/`）を 1 つに集める。iOS は Apple の entitlement 承認待ちで対象外。
//!
//! 依存方向は本 crate → `fandhe-core` / `fandhe-runtime` / `fandhe-platform-android` の一方向で、
//! egui・`fandhe-egui-proxy`・`fandhe-device` には依存しない。呼び出し元は画面カタログ
//! （`fandhe-gallery` の `screens/registry.rs` の `effect "nfc_card_emulation_show"` 行）で、
//! `ProxyApp::update` のフレーム末尾から毎フレーム [`nfc_card_emulation_show_effect`] が呼ばれる。
//! 画面の状態機械は `fandhe-core` に残している（置き場の最終判断は #1165 系）。
//!
//! 扱う範囲は「固定の独自 AID への SELECT に固定の応答を返す HCE の状態を読み出す」ところまで。
//! 応答そのものは OS 側のサービス（Android は `HostApduService`）が返し、[`CardEmulation`] は状態の
//! 読み出しだけを持つ。実カードの模擬・決済・応答内容の変更は扱わない。
//!
//! # 契約（各 OS 実装・呼び出し元が守る不変条件）
//!
//! - **結果は 3 種**: `Ok(HceStatus)`、`Err(Internal)`（JNI 失敗・Activity の一時的な不在。
//!   `Unsupported` にするとドライバが読み出しをやめ、復帰しても戻らないためこちらにする）、
//!   `Err(Unsupported)`（Android 以外、`FEATURE_NFC_HOST_CARD_EMULATION` が無い、`NfcAdapter` が null）。
//!   初期化できない実装もダミーの成功へ倒さず [`UnsupportedCardEmulation`] へ倒す
//! - **NFC がオフでも失敗にしない**: `nfc_enabled=false` の `Ok` で返す。NFC の有効と既定の振り先は
//!   読むたびに OS（`NfcAdapter.isEnabled`・`CardEmulation.isDefaultServiceForAid`）へ問い、アプリ内に持たない
//! - **応答回数**: `responses` は「OS 側のサービスが応答を返した回数」で、リーダーが受け取ったことは
//!   保証しない。`since_last_ms` は単調時計（Android は `SystemClock.elapsedRealtime`）で測り、
//!   応答 0 回なら `None`（番兵値を使わない）
//! - **キャンセル・正常終了・失敗の三値**（`device-bridge-contracts.md` 10）: 利用者のキャンセル操作は
//!   無い。実行時の権限ダイアログも無い（`NFC` は normal 権限）ので拒否も無い。成功・NFC オフ（正常）・
//!   失敗を混ぜない
//! - **AID の正はこの crate の [`AID`] だけ**。Java へは JNI の引数で渡す。`android/res/xml/apduservice.xml`
//!   への複製は Android 実装（#1129）の `include_str!` テストで固定する
//! - **応答の中身・OS の例外文言をログ・`Debug`・エラー文言に出さない**
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内で読み出しが高々 1 件、古い世代の完了を捨てること、失敗を 0 回や成功として
//! 見せないこと、嘘の成功の禁止。
//! 守らない: 他アプリが同じ AID を登録したときの振り先（`default_for_aid=false` で出す）、画面オフ・
//! ロック中、Secure NFC、リーダーの再試行による重複計数、プロセス終了で回数が 0 に戻ること、
//! ポーリング間隔ぶんの表示の遅れ、iOS（Apple の entitlement 承認待ち）。
//!
//! # pull 型のポーリングと別スレッド読み出し
//!
//! 画面へは effect ドライバが [`POLL_INTERVAL`] ごとに状態を読んで届ける。読み出しはブロックしうるため
//! ワーカースレッドで行い、`Poller` が高々 1 件に絞る（判定と状態更新は同じロックの内側）。期限は
//! 単調時計（`Instant`）で測る。期限前の分岐でも次回のポーリングは必ず予約する
//! （`device-bridge-contracts.md`「4. ポーリング・再描画」）。完了は先に次回の期限と空きを確定して
//! から画面へ送る。失敗の後も読み出しを続け、復帰したら状態を出す。非対応 OS は結果が変わらない
//! ため 1 回だけ送り、以後は読み出しも再描画の予約もしない。

// unsafe は持たない。JNI の attach と Activity 取得は `fandhe-platform-android::with_activity` に閉じる。
#![forbid(unsafe_code)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::nfc_card_emulation_show::{
    NfcCardEmulationShowState, ReadOutcome, apply_unsupported, finish_read, set_aid,
};
use fandhe_runtime::CompletionSender;

/// 本アプリの HCE が応答する独自 AID（16 進）。`F0` + ASCII "FANDHE" + `01`（ISO/IEC 7816-4 の
/// 独自範囲、5〜16 バイト）。
pub const AID: &str = "F046414E44484501";

/// 状態を読み直す間隔。表示の遅れはこの間隔ぶん許容する（crate doc の守らない範囲）。
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 読み出しの失敗。OS の文言は持たない（ログ・`Debug` に出さない契約）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CardEmulationError {
    /// この OS・端末では HCE を使えない。ドライバは以後読み出さない。
    #[error("nfc_card_emulation is unsupported")]
    Unsupported,
    /// 一時的な失敗（JNI 失敗・Activity の不在・値域外）。次回のポーリングで回復しうる。
    #[error("nfc_card_emulation failed")]
    Internal,
}

/// HCE の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HceStatus {
    /// NFC が有効か。オフは失敗ではない。
    pub nfc_enabled: bool,
    /// この AID の既定の振り先が本アプリか。
    pub default_for_aid: bool,
    /// OS 側のサービスが応答を返した回数（このプロセス内）。
    pub responses: u64,
    /// 最後の応答からの経過ミリ秒。応答 0 回なら `None`。
    pub since_last_ms: Option<u64>,
}

/// HCE の状態の読み出し。ブロックしうるため UI スレッドではなくワーカーで呼ぶ。
pub trait CardEmulation: Send + Sync {
    /// 現在の状態を読む。
    fn status(&self) -> Result<HceStatus, CardEmulationError>;
}

/// 非対応 OS 向け。常に `Err(Unsupported)`。
pub struct UnsupportedCardEmulation;

impl CardEmulation for UnsupportedCardEmulation {
    fn status(&self) -> Result<HceStatus, CardEmulationError> {
        Err(CardEmulationError::Unsupported)
    }
}

/// 読み出しの二重起動防止・世代・次回期限をまとめて持つ。
struct Poller {
    inner: Mutex<PollerInner>,
}

struct PollerInner {
    /// 次に読んでよい時刻。`None` は未開始（すぐ読む）。
    next_due: Option<Instant>,
    /// 発行済みの最新世代。
    generation: u64,
    /// 読み出しスレッドが走っているか。
    in_flight: bool,
}

impl Poller {
    const fn new() -> Self {
        Self {
            inner: Mutex::new(PollerInner {
                next_due: None,
                generation: 0,
                in_flight: false,
            }),
        }
    }
}

/// 現在の OS の実装（Android は `android::implementation`、他は `Unsupported` 固定）。
/// 状態型に依存しない入口（consumer-app-structure.md §8.2）。フレームワークの外のアプリも、自分の状態型の effect から使える。
pub fn platform_nfc_card_emulation() -> &'static Arc<dyn CardEmulation> {
    static EMU: OnceLock<Arc<dyn CardEmulation>> = OnceLock::new();
    EMU.get_or_init(|| {
        #[cfg(target_os = "android")]
        {
            android::implementation()
        }
        #[cfg(not(target_os = "android"))]
        {
            Arc::new(UnsupportedCardEmulation)
        }
    })
}

/// 毎フレーム呼ばれ、期限が来ていて読み出し中でなければ読み出しを 1 件起動する。
fn drive(
    state: &NfcCardEmulationShowState,
    sender: &CompletionSender<NfcCardEmulationShowState>,
    emu: &Arc<dyn CardEmulation>,
    poller: &'static Poller,
    now: Instant,
) {
    // 非対応は確定済み。読み出しも再描画の予約もしない。
    if state.is_unsupported() {
        return;
    }
    // 受信側がドロップ済みの送信失敗は無視してよい（coding-rust.md: panic を API 契約にしない）。
    if state.aid().is_none() {
        let _ = sender.send(Box::new(|s: &mut NfcCardEmulationShowState| {
            set_aid(s, AID);
        }));
    }

    let generation = {
        let mut inner = match poller.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // 読み出し中は何もしない。完了の send が再描画を起こす（二重起動の防止）。
        if inner.in_flight {
            return;
        }
        if let Some(due) = inner.next_due
            && now < due
        {
            // 送信を省く分岐でも次回を予約する。
            sender.request_repaint_after(due - now);
            return;
        }
        inner.generation += 1;
        inner.in_flight = true;
        inner.generation
    };

    let emu = Arc::clone(emu);
    let tx = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("fandhe-nfc-card-emulation".into())
        .spawn({
            let tx = tx.clone();
            move || {
                let result = emu.status();
                finish(poller, &tx, generation, result);
            }
        });
    if spawned.is_err() {
        // 起動できなければ失敗として確定する（成功を装わない）。
        finish(poller, &tx, generation, Err(CardEmulationError::Internal));
    }
}

/// 読み出しの完了。先に次回の期限と空きを確定してから画面へ送る。
fn finish(
    poller: &Poller,
    sender: &CompletionSender<NfcCardEmulationShowState>,
    generation: u64,
    result: Result<HceStatus, CardEmulationError>,
) {
    {
        let mut inner = match poller.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        inner.next_due = Some(Instant::now() + POLL_INTERVAL);
        inner.in_flight = false;
    }
    match result {
        Ok(st) => {
            let outcome = ReadOutcome::Status {
                nfc_enabled: st.nfc_enabled,
                default_for_aid: st.default_for_aid,
                responses: st.responses,
                since_last_ms: st.since_last_ms,
            };
            let _ = sender.send(Box::new(move |s: &mut NfcCardEmulationShowState| {
                finish_read(s, generation, outcome);
            }));
            sender.request_repaint_after(POLL_INTERVAL);
        }
        // 非対応は結果が変わらないため 1 回だけ送る。再描画は予約しない。
        Err(CardEmulationError::Unsupported) => {
            let _ = sender.send(Box::new(apply_unsupported));
        }
        // 理由（OS の文言）は画面へ渡さない。次回も読み、復帰したら状態へ戻る。
        Err(CardEmulationError::Internal) => {
            let _ = sender.send(Box::new(move |s: &mut NfcCardEmulationShowState| {
                finish_read(s, generation, ReadOutcome::Failed);
            }));
            sender.request_repaint_after(POLL_INTERVAL);
        }
    }
}

/// `registry.rs` の `effect` 区画から `nfc_card_emulation_show` 画面用に配線される effect ドライバ。
pub fn nfc_card_emulation_show_effect(
    state: &NfcCardEmulationShowState,
    sender: &CompletionSender<NfcCardEmulationShowState>,
) {
    static POLLER: Poller = Poller::new();
    drive(
        state,
        sender,
        platform_nfc_card_emulation(),
        &POLLER,
        Instant::now(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::nfc_card_emulation_show::{responses_text, status_text};
    use fandhe_runtime::{Completion, RepaintWaker};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};

    type St = NfcCardEmulationShowState;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    fn sender_with_channel() -> (CompletionSender<St>, Receiver<Completion<St>>) {
        let (tx, rx) = channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
    }

    fn drain_wait(rx: &Receiver<Completion<St>>, state: &mut St, want: usize) -> usize {
        let mut n = 0;
        while n < want {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(c) => {
                    c(state);
                    n += 1;
                }
                Err(_) => break,
            }
        }
        n
    }

    fn drain_now(rx: &Receiver<Completion<St>>, state: &mut St) -> usize {
        let mut n = 0;
        while let Ok(c) = rx.try_recv() {
            c(state);
            n += 1;
        }
        n
    }

    struct Scripted {
        calls: Arc<AtomicUsize>,
        results: Mutex<Vec<Result<HceStatus, CardEmulationError>>>,
    }
    impl CardEmulation for Scripted {
        fn status(&self) -> Result<HceStatus, CardEmulationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut r = self.results.lock().unwrap();
            if r.len() > 1 { r.remove(0) } else { r[0] }
        }
    }

    fn scripted(
        results: Vec<Result<HceStatus, CardEmulationError>>,
    ) -> (Arc<dyn CardEmulation>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Scripted {
                calls: calls.clone(),
                results: Mutex::new(results),
            }),
            calls,
        )
    }

    fn leak_poller() -> &'static Poller {
        Box::leak(Box::new(Poller::new()))
    }

    fn st(n: u64) -> Result<HceStatus, CardEmulationError> {
        Ok(HceStatus {
            nfc_enabled: true,
            default_for_aid: true,
            responses: n,
            since_last_ms: None,
        })
    }

    fn fail() -> Result<HceStatus, CardEmulationError> {
        Err(CardEmulationError::Internal)
    }

    #[test]
    fn aid_is_proprietary_even_hex_within_limit() {
        assert!(AID.starts_with("F0"));
        assert_eq!(AID.len() % 2, 0);
        assert!(
            AID.chars()
                .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))
        );
        assert!((5..=16).contains(&(AID.len() / 2)));
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(
            UnsupportedCardEmulation.status(),
            Err(CardEmulationError::Unsupported)
        );
    }

    #[test]
    fn unsupported_is_sent_once_then_silent() {
        let (tx, rx) = sender_with_channel();
        let emu: Arc<dyn CardEmulation> = Arc::new(UnsupportedCardEmulation);
        let poller = leak_poller();
        let mut state = St::default();

        drive(&state, &tx, &emu, poller, Instant::now());
        // AID 注入 + 非対応
        assert_eq!(drain_wait(&rx, &mut state, 2), 2);
        assert!(state.is_unsupported());

        drive(
            &state,
            &tx,
            &emu,
            poller,
            Instant::now() + Duration::from_secs(60),
        );
        assert_eq!(drain_now(&rx, &mut state), 0);
    }

    #[test]
    fn reads_status_and_injects_aid() {
        let (tx, rx) = sender_with_channel();
        let (emu, _) = scripted(vec![st(3)]);
        let mut state = St::default();
        drive(&state, &tx, &emu, leak_poller(), Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 2), 2);
        assert_eq!(responses_text(&state), "応答した回数: 3（このプロセス内）");
        assert_eq!(state.aid(), Some(AID));
    }

    #[test]
    fn failure_is_not_zero_and_recovers() {
        let (tx, rx) = sender_with_channel();
        let (emu, _) = scripted(vec![fail(), st(4)]);
        let poller = leak_poller();
        let mut state = St::default();

        drive(&state, &tx, &emu, poller, Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 2), 2);
        assert_eq!(status_text(&state), "状態を読み出せません");
        assert_eq!(responses_text(&state), "");

        drive(
            &state,
            &tx,
            &emu,
            poller,
            Instant::now() + POLL_INTERVAL * 2,
        );
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert_eq!(responses_text(&state), "応答した回数: 4（このプロセス内）");
    }

    #[test]
    fn before_due_does_not_read_or_send() {
        let (tx, rx) = sender_with_channel();
        let (emu, calls) = scripted(vec![st(1)]);
        let poller = leak_poller();
        let mut state = St::default();

        drive(&state, &tx, &emu, poller, Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 2), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        drive(&state, &tx, &emu, poller, Instant::now());
        assert_eq!(drain_now(&rx, &mut state), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    struct Blocking {
        calls: Arc<AtomicUsize>,
        gate: Mutex<Receiver<()>>,
    }
    impl CardEmulation for Blocking {
        fn status(&self) -> Result<HceStatus, CardEmulationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let _ = self
                .gate
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
            st(9)
        }
    }

    #[test]
    fn in_flight_read_is_not_started_twice() {
        let (tx, rx) = sender_with_channel();
        let (open, gate): (Sender<()>, Receiver<()>) = channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let emu: Arc<dyn CardEmulation> = Arc::new(Blocking {
            calls: calls.clone(),
            gate: Mutex::new(gate),
        });
        let poller = leak_poller();
        let mut state = St::default();

        let far = Instant::now() + Duration::from_secs(100);
        drive(&state, &tx, &emu, poller, far);
        drive(&state, &tx, &emu, poller, far);
        drive(&state, &tx, &emu, poller, far);
        open.send(()).unwrap();
        // AID 注入（state が未注入のまま 3 回呼ばれるため複数通）+ 読み出し結果
        drain_wait(&rx, &mut state, 4);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(responses_text(&state), "応答した回数: 9（このプロセス内）");
    }

    #[test]
    fn stale_generation_does_not_override_newer_failure() {
        let mut state = St::default();
        finish_read(&mut state, 2, ReadOutcome::Failed);
        finish_read(
            &mut state,
            1,
            ReadOutcome::Status {
                nfc_enabled: true,
                default_for_aid: true,
                responses: 5,
                since_last_ms: None,
            },
        );
        assert_eq!(status_text(&state), "状態を読み出せません");
    }
}
