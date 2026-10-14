//! Phase 11（親 #864）型サンプル「読み上げ」（Issue #865）の effect ドライバ。
//! `fandhe_core::screens::tts_speak`（端末依存ゼロ）の発話・停止要求を観測し、
//! `fandhe_device::tts::Tts` で OS の音声合成を呼び、`CompletionSender` 経由で
//! 結果を返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。iOS/Android は本ファイルの
//! [`shared_tts`] が `device::ios::IosTts`/`device::android::AndroidTts`
//! （Issue #866）へ分岐する。Web は #867 が別経路（`fandhe-web-dom`）で配線する。
//!
//! # 別スレッドで OS を呼ぶ理由
//!
//! `fandhe_device::tts::CommandTts::speak` は子プロセス起動直後の即時失敗を検知する
//! ため `STARTUP_FAILURE_GRACE`（既定 200ms）だけ同期的に待つ。`drive`/effect は
//! `ProxyApp::update`（UI スレッド）から毎フレーム呼ばれるため、そのまま同期で呼ぶと
//! 「読み上げ」を押すたびに描画・入力処理が最低 200ms 止まる（codex-review P1 指摘・
//! Cursor Bugbot 指摘、`crates/fandhe-device/src/tts.rs` の `STARTUP_FAILURE_GRACE`
//! doc 参照）。発話・停止それぞれの `dispatch_*` は要求を発送済みにする
//! （`mark_*_dispatched`）完了だけを同期送信し、実際の OS 呼び出しは
//! [`command_queue`] のワーカースレッドで行う（`http_fetch.rs::spawn_fetch` と同型の
//! 二段構え、`fandhe_core::screens::tts_speak` モジュール doc「発話中でも停止を
//! 受け付ける理由」参照）。
//!
//! UI スレッドから切り出したことで、`speak` の猶予（200ms）を過ぎてから失敗する
//! 子プロセスの検知（[`fandhe_device::tts::Tts::wait_outcome`]、ブロッキング）も
//! UI スレッドを塞がずに行える（`wait_outcome` 自体はワーカースレッドからさらに
//! 別スレッドへ切り出す、後述「OS 呼び出しを 1 本のキューで順序保証する理由」参照）。
//!
//! # 発話と停止を独立に発送する理由（codex-review P1・Cursor Bugbot 指摘の是正）
//!
//! 発話（[`run_speak`]、`wait_outcome` でブロッキング待機）と停止（[`run_stop`]）を
//! それぞれ別の要求として扱う。1 本化していた旧版は、発話の完了
//! （`wait_outcome` の戻り）を待ってからでないと停止要求を発送できず、発話中は
//! 停止ボタンが機能しなかった。[`fandhe_device::tts::CommandTts::wait_outcome`]
//! は内部ロックを短時間ずつ手放すため（`WAIT_OUTCOME_POLL_INTERVAL` 参照）、
//! 発話待機と並行して `Tts::stop` を呼んでも安全に子プロセスを kill できる。
//! [`tts_speak_effect`] は毎フレーム両方の要求を独立に観測し、それぞれ即座に
//! [`command_queue`] へ渡す。
//!
//! # OS 呼び出しを 1 本のキューで順序保証する理由（codex-review P1 指摘の是正）
//!
//! 旧版は発話・停止それぞれの発送のたびに `std::thread::spawn` で独立したスレッドを
//! 起こし、そのスレッドの中で `Tts::speak`/`Tts::stop` を呼んでいた。スレッドの
//! spawn 順序と実際にスケジューラが実行する順序は一致する保証が無いため、
//! 「発話要求を発送した直後のフレームで停止要求を発送する」と、停止スレッドが
//! 先に実行されて（この時点では `Tts` 内部にまだ子プロセスが無いため）
//! 何もせず成功し、その後から発話スレッドが実際に `Tts::speak` を呼んで
//! 発話を開始してしまう競合があった（画面は「停止しました」のまま鳴り続ける）。
//!
//! [`command_queue`] は発話・停止のいずれも `TtsCommand` として 1 本の FIFO
//! チャンネルへ積み、唯一のワーカースレッド（[`worker_loop`]）が順番どおりに
//! `Tts::speak`/`Tts::stop` を呼ぶ。これにより OS 呼び出しの実行順序が
//! 発送順序と一致する。ワーカーは `Tts::speak` の起動判定（`STARTUP_FAILURE_GRACE`
//! 分だけ同期的にブロックする）までは自スレッドで行うが、その後の
//! `wait_outcome`（長時間ブロッキング）は別スレッドへ切り出すため、次に
//! キューへ積まれた停止要求の処理を待たせない（発話中でも停止を即座に
//! 受け付けられる、という上記の要件を壊さない）。
//!
//! アプリ終了時の [`stop_shared_tts`] も同じキューへ積む（`StopAndAck`）ことで、
//! 「終了直前に発送されまだ処理されていない発話要求」より必ず後に停止が
//! 実行されることを保証する（codex-review 指摘: 「終了時の stop_shared_tts にも
//! 同じ順序保証が必要」）。
//!
//! # Activity 再生成と完了の配送（#881 の判断）
//!
//! [`run_speak`] が `wait_outcome` を待つために切り出す別スレッドは、要求元の
//! `ProxyApp` が持つ `CompletionSender` を clone して持ち出す（push 型）。Android で
//! Activity が再生成される（`configChanges` 外の uiMode / locale 変更。回転は
//! 抑止済み）と `ProxyApp` ごと作り直されるため、持ち出し済み sender は旧
//! チャンネルを指したまま実送信は `let _ =` で無視される。
//!
//! `crate::device::file_pick`（#871）と異なり pull 型へは寄せない判断をした。
//! 理由: Java `MainActivity.onDestroy` は `currentSpeakingRequestId` を
//! `getAndSet` で読み、発話中であれば `onStop` を待たず即座に
//! `nativeOnTtsResult` を呼んで確定させる（`AtomicLong` の doc・`onDestroy` 実装
//! 参照）。これにより旧 sender へ送られる完了は必ず「Java が既に確定させた
//! 結果」であり、新 `ProxyApp` の画面は再生成後 `speak_text: None` から始まる。
//! [`fandhe_core::screens::tts_speak::finish_speak`] は `speak_text.is_none()`
//! のとき完了を無視するため no-op で、実害は「読み上げました」の文言が新画面に
//! 出ない既知の天井のみ（停止ボタンは新 Activity の `TextToSpeech` に対して
//! 正しく機能する）。pull 型へ寄せて復元するには `finish_speak` が要求する
//! `generation` 照合を `fandhe-core` 側に世代付き復元 API として追加する必要があり
//! （コア変更を要する）、得られる利得（文言 1 つ）に見合わないと判断した。

use std::sync::{mpsc, OnceLock};

use fandhe_core::screens::tts_speak::{
    finish_speak, finish_stop, mark_speak_dispatched, mark_stop_dispatched, take_speak_request,
    take_stop_request, TtsSpeakState,
};
use fandhe_device::tts::Tts;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// プロセス内で共有する TTS 実装（接続失敗はエラーとして保持する）。
///
/// `CommandTts` は前回の子プロセスを `Mutex<Option<Child>>` で保持し続けるため、
/// ハンドルを作り直さず 1 個だけ使い回す（`fandhe_device::tts::CommandTts` の doc 参照）。
static TTS: OnceLock<Result<Box<dyn Tts>, String>> = OnceLock::new();

/// OS の読み上げへ接続できないときに画面へ出す案内。
const UNAVAILABLE: &str = "この環境では読み上げを使えません";

/// 実行対象プラットフォームに応じた `Tts` 実装を返す。
///
/// **接続失敗を成功として偽装しない**（`device/clipboard.rs` と同じ判断、
/// #859 の P1 指摘・親 #864 の再発防止点）。iOS/Android は #866 で
/// `IosTts`/`AndroidTts` を配線したため常に接続できる（初期化自体が失敗しない
/// 構成、実際の発話成否は `Tts::speak`/`wait_outcome` の戻り値で個別に判定する）。
/// それ以外（macOS/Windows は `CommandTts`、Linux 等その他は未対応）は
/// 従来どおり `CommandTts::new()` の成否に従う。
fn shared_tts() -> Result<&'static dyn Tts, &'static str> {
    let cached = TTS.get_or_init(|| {
        #[cfg(target_os = "ios")]
        {
            Ok(Box::new(crate::device::ios::IosTts) as Box<dyn Tts>)
        }
        #[cfg(target_os = "android")]
        {
            Ok(Box::new(crate::device::android::AndroidTts) as Box<dyn Tts>)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            use fandhe_device::tts::CommandTts;
            CommandTts::new()
                .map(|t| Box::new(t) as Box<dyn Tts>)
                .map_err(|e: DeviceError| e.to_string())
        }
    });
    match cached {
        Ok(tts) => Ok(tts.as_ref()),
        // 接続時のエラー文言は内部要因を含み得るため画面へは転送せず、
        // 固定の案内へ畳む（security.md A09）。
        Err(_) => Err(UNAVAILABLE),
    }
}

/// `DeviceError` を画面へ渡す文言へ変換する（`device/clipboard.rs::to_message` と同型）。
fn to_message(e: DeviceError) -> String {
    e.to_string()
}

/// [`command_queue`] へ積む要求。発話・停止いずれも唯一のワーカースレッド
/// （[`spawn_worker`]）が受信順（FIFO）に処理するため、`Tts::speak`/`Tts::stop`
/// の実行順序が発送順序と一致する（モジュール doc「OS 呼び出しを 1 本のキューで
/// 順序保証する理由」参照）。
enum TtsCommand {
    /// 発話要求。`generation` は [`fandhe_core::screens::tts_speak::take_speak_request`]
    /// が返した世代番号で、[`finish_speak`] へそのまま引き渡して完了通知の照合に使う
    /// （codex-review P1・Cursor Bugbot 指摘の是正:
    /// `fandhe_core::screens::tts_speak` モジュール doc「完了通知を要求 ID（世代）で
    /// 識別する理由」参照）。
    Speak {
        generation: u64,
        text: String,
        sender: CompletionSender<TtsSpeakState>,
    },
    /// 停止要求（画面側からの発送）。
    Stop {
        sender: CompletionSender<TtsSpeakState>,
    },
    /// アプリ終了時の停止要求（[`stop_shared_tts`]）。画面へ結果を表示する
    /// 相手がいないため `CompletionSender` の代わりに応答用チャンネルを持つ。
    /// キューへ積むことで、終了直前に発送されまだ処理されていない発話要求より
    /// 必ず後に実行される（モジュール doc 参照）。
    StopAndAck { ack: mpsc::Sender<()> },
}

/// [`TtsCommand`] を受け取る唯一のワーカースレッドへの送信端。
/// 初回アクセス時にワーカースレッドを 1 個だけ起動する（[`worker_loop`]）。
static COMMAND_QUEUE: OnceLock<mpsc::Sender<TtsCommand>> = OnceLock::new();

/// ワーカースレッドへの送信端を返す（未起動なら起動する）。
fn command_queue() -> &'static mpsc::Sender<TtsCommand> {
    COMMAND_QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<TtsCommand>();
        std::thread::spawn(move || worker_loop(rx, shared_tts));
        tx
    })
}

/// キューから受け取った要求を受信順（FIFO）に処理するワーカー本体。
/// プロセスが生きている間ずっと動き続ける（`ProxyApp` は 1 プロセスに 1 個の
/// アプリのみを想定するため、キューは使い捨てず再利用する）。
///
/// `get_tts` を引数で受け取る（本番は [`shared_tts`] を渡す）ことで、テストから
/// 呼び出し順序を記録するモック `Tts` を注入し、[`TtsCommand`] を FIFO で処理する
/// こと自体（＝発送順序どおりに `Tts::speak`/`Tts::stop` が呼ばれること）を
/// 実行順序に依存せず検証できる。
fn worker_loop(
    rx: mpsc::Receiver<TtsCommand>,
    get_tts: impl Fn() -> Result<&'static dyn Tts, &'static str>,
) {
    for command in rx {
        match command {
            TtsCommand::Speak {
                generation,
                text,
                sender,
            } => match get_tts() {
                Ok(tts) => run_speak(tts, generation, text, &sender),
                Err(message) => fail_speak(&sender, generation, message),
            },
            TtsCommand::Stop { sender } => match get_tts() {
                Ok(tts) => run_stop(tts, &sender),
                Err(message) => fail_stop(&sender, message),
            },
            TtsCommand::StopAndAck { ack } => {
                if let Ok(tts) = get_tts() {
                    let _ = tts.stop();
                }
                let _ = ack.send(());
            }
        }
    }
}

/// アプリ終了時に共有 TTS の発話を止める（`crate::device::shutdown` → `ProxyApp::on_exit`
/// から呼ばれる契約、`app.rs` の `on_exit` 実装 doc 参照）。
///
/// [`TTS`] は `static OnceLock` で保持しているため、プロセス終了時に `Drop`
/// （`fandhe_device::tts::CommandTts` の子プロセス回収処理）が走らない
/// （Rust の `static` は正常終了時に破棄されない仕様、codex-review P1 指摘）。
/// ウィンドウを閉じても子プロセス（`say`/PowerShell）が発話を続けたまま
/// プロセスだけ終了するのを防ぐため、`on_exit` から明示的に `stop()` する。
///
/// [`command_queue`] 経由で発送する（直接 `TTS.get()` を呼んで即座に止めない）。
/// 終了直前に発送されまだワーカーが処理していない発話要求がキューに残っている
/// ケースで、この停止が先に実行されて子プロセス未登録のまま成功してしまうと
/// （＝停止直後に発話要求が実際の子プロセスを起動する）意味がない（codex-review
/// 指摘「終了時の stop_shared_tts にも同じ順序保証が必要」）。FIFO キューへ積み、
/// ワーカーが実際に処理し終える（＝先行する発話要求の `Tts::speak` 呼び出しが
/// 既に終わっている）まで待ってから戻る。
///
/// まだキューが 1 度も使われていない（[`COMMAND_QUEUE`] が `None`、＝発話も停止も
/// 一度も発送されていない）場合は接続すら試みていないため何もしない。
/// 応答待ちには上限（2 秒）を設け、ワーカーが `wait_outcome` の長時間ポーリングで
/// 詰まっている場合でもプロセス終了処理自体をブロックし続けない。
pub fn stop_shared_tts() {
    let Some(queue) = COMMAND_QUEUE.get() else {
        return;
    };
    let (ack_tx, ack_rx) = mpsc::channel();
    if queue.send(TtsCommand::StopAndAck { ack: ack_tx }).is_err() {
        // ワーカースレッドが既に終了している（通常は起きない）。これ以上は待てない。
        return;
    }
    let _ = ack_rx.recv_timeout(std::time::Duration::from_secs(2));
}

/// 未発送の発話要求があれば取り出し、[`mark_speak_dispatched`] を同期送信して
/// 二重発送を防ぐ。実際の OS 呼び出しは呼び出し元が [`command_queue`] へ渡す。
fn dispatch_speak(
    state: &TtsSpeakState,
    sender: &CompletionSender<TtsSpeakState>,
) -> Option<(u64, String)> {
    let request = take_speak_request(state)?;
    // 受信側（ProxyApp）がドロップ済み（アプリ終了中）の送信失敗は無視してよい
    // （coding-rust.md: panic を API 契約にしない、`device/tilt.rs` と同じ判断）。
    let _ = sender.send(Box::new(mark_speak_dispatched));
    Some(request)
}

/// 未発送の停止要求があれば取り出し、[`mark_stop_dispatched`] を同期送信して
/// 二重発送を防ぐ（`dispatch_speak` の停止版。発話の発送状況とは独立に判定する、
/// モジュール doc「発話と停止を独立に発送する理由」参照）。
fn dispatch_stop(state: &TtsSpeakState, sender: &CompletionSender<TtsSpeakState>) -> bool {
    if !take_stop_request(state) {
        return false;
    }
    let _ = sender.send(Box::new(mark_stop_dispatched));
    true
}

/// 発送済みの発話要求を OS へ渡し、結果を完了として送る（テストから任意の実装を
/// 差し込めるよう `worker_loop` 本体から切り出す。呼び出し元（ワーカースレッド）が
/// 同期的に呼ぶ契約だが、`Tts::speak` の起動判定（`STARTUP_FAILURE_GRACE`）分しか
/// 占有しない — その後の `wait_outcome`（長時間ブロッキング）は別スレッドへ
/// 切り出し、キューの次の要求（停止など）を待たせない）。
///
/// Android の Activity 再生成時、`wait_outcome` から戻った完了は旧 `ProxyApp` の
/// sender へ送られ無視される（`onDestroy` が既に確定させているため実結果は
/// 不要、モジュール doc「Activity 再生成と完了の配送（#881 の判断）」参照）。
fn run_speak(
    tts: &'static dyn Tts,
    generation: u64,
    text: String,
    sender: &CompletionSender<TtsSpeakState>,
) {
    match tts.speak(&text) {
        Ok(device_generation) => {
            // 起動には成功。真の結果（`wait_outcome`）は長時間ブロッキングしうるため
            // 別スレッドへ切り出し、ワーカー本体（キュー処理）を塞がない
            // （後続の停止要求を即座に処理できるようにする、モジュール doc 参照）。
            // `device_generation`（`Tts::speak` が払い出した世代 id）を渡すことで、
            // このスレッドの sleep 中に別の発話が始まっても他の世代の結果を
            // 誤って回収しない（`fandhe_device::tts::Tts::wait_outcome` doc・
            // `CommandTts::wait_outcome` doc 参照。`generation` は画面側の完了通知
            // 照合用 id で、`device_generation` とは別物）。
            let sender = sender.clone();
            std::thread::spawn(move || {
                let result = tts.wait_outcome(device_generation).map_err(to_message);
                let _ = sender.send(Box::new(move |s: &mut TtsSpeakState| {
                    finish_speak(s, generation, result);
                }));
            });
        }
        Err(e) => fail_speak(sender, generation, to_message(e)),
    }
}

/// 発送済みの停止要求を OS へ渡し、結果を完了として送る（`run_speak` の停止版）。
/// 発話待機スレッドが `wait_outcome` でブロッキング中でも、`Tts::stop` は
/// 内部ロックを奪い合って並行に呼べる契約（`CommandTts::wait_outcome` doc 参照）。
fn run_stop(tts: &dyn Tts, sender: &CompletionSender<TtsSpeakState>) {
    let result = tts.stop().map_err(to_message);
    let _ = sender.send(Box::new(move |s: &mut TtsSpeakState| {
        finish_stop(s, result);
    }));
}

/// OS の読み上げへ接続できないときに、発送済みの発話要求を失敗として畳む
/// （要求が残ったままだと画面のボタンが戻らず操作不能になる）。
fn fail_speak(
    sender: &CompletionSender<TtsSpeakState>,
    generation: u64,
    message: impl Into<String>,
) {
    let message = message.into();
    let _ = sender.send(Box::new(move |s: &mut TtsSpeakState| {
        finish_speak(s, generation, Err(message));
    }));
}

/// OS の読み上げへ接続できないときに、発送済みの停止要求を失敗として畳む
/// （`fail_speak` の停止版）。
fn fail_stop(sender: &CompletionSender<TtsSpeakState>, message: &'static str) {
    let message = message.to_string();
    let _ = sender.send(Box::new(move |s: &mut TtsSpeakState| {
        finish_stop(s, Err(message));
    }));
}

/// `registry.rs` の `effect` 区画から `tts_speak` 画面用に配線される effect ドライバ。
/// `ProxyApp::update`（UI スレッド）から毎フレーム呼ばれるが、実際の OS 呼び出しは
/// [`dispatch_speak`]/[`dispatch_stop`] で要求を発送済みにした直後にそれぞれ
/// [`command_queue`] へ渡し、UI スレッドは待たない。発話・停止は独立に発送するため、
/// 発話が進行中でも同じフレームで停止要求を発送できる
/// （モジュール doc「発話と停止を独立に発送する理由」参照）。キューは 1 本の
/// ワーカースレッドが FIFO で処理するため、`Tts::speak`/`Tts::stop` の実行順序は
/// ここで発送した順序と一致する（モジュール doc「OS 呼び出しを 1 本のキューで
/// 順序保証する理由」参照）。
pub fn tts_speak_effect(state: &TtsSpeakState, sender: &CompletionSender<TtsSpeakState>) {
    if dispatch_stop(state, sender) {
        let _ = command_queue().send(TtsCommand::Stop {
            sender: sender.clone(),
        });
    }
    if let Some((generation, text)) = dispatch_speak(state, sender) {
        let _ = command_queue().send(TtsCommand::Speak {
            generation,
            text,
            sender: sender.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::tts_speak::{set_input, speak, stop};

    fn sender_with_channel() -> (
        CompletionSender<TtsSpeakState>,
        std::sync::mpsc::Receiver<crate::app::Completion<TtsSpeakState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    /// 発話テキストを記録するだけの `Tts`（正常系の橋渡しを検証する）。
    #[derive(Default)]
    struct RecordingTts {
        spoken: std::sync::Mutex<Vec<String>>,
    }

    impl Tts for RecordingTts {
        fn speak(&self, text: &str) -> Result<u64, DeviceError> {
            self.spoken
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(text.to_string());
            Ok(1)
        }
        fn stop(&self) -> Result<(), DeviceError> {
            Ok(())
        }
    }

    /// 常に失敗する `Tts`（エラー文言の転送経路を検証する）。
    struct FailingTts;

    impl Tts for FailingTts {
        fn speak(&self, _text: &str) -> Result<u64, DeviceError> {
            Err(DeviceError::Unsupported("tts"))
        }
        fn stop(&self) -> Result<(), DeviceError> {
            Err(DeviceError::Unsupported("tts"))
        }
    }

    #[test]
    fn dispatch_speak_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let state = TtsSpeakState::default();

        assert_eq!(dispatch_speak(&state, &sender), None);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn dispatch_speak_marks_state_dispatched_and_returns_text() {
        // `dispatch_speak` は `mark_speak_dispatched` を同期送信するだけで、
        // 実際の完了は呼び出し元（別スレッド想定）が `run_speak` で送る
        // （二重発送しないことの固定、
        // `fandhe_core::screens::tts_speak::take_speak_request_returns_none_once_dispatched`
        // と対）。
        let (sender, rx) = sender_with_channel();
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "発話テスト".to_string());
        speak(&mut state);

        let (generation, text) = dispatch_speak(&state, &sender).expect("要求がある");
        assert_eq!(text, "発話テスト");
        assert_eq!(generation, state.speak_generation);

        rx.recv().expect("mark_speak_dispatched が送られる")(&mut state);
        assert!(state.speak_dispatched);
        assert_eq!(
            dispatch_speak(&state, &sender),
            None,
            "発送済みの二重取得はしない"
        );
    }

    #[test]
    fn dispatch_stop_can_fire_while_speak_is_dispatched() {
        // codex-review P1・Cursor Bugbot 指摘の是正: 発話が発送済み（進行中）
        // でも停止要求は独立に発送できる。
        let (sender, rx) = sender_with_channel();
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "発話テスト".to_string());
        speak(&mut state);
        let _ = dispatch_speak(&state, &sender).expect("要求がある");
        rx.recv().expect("mark_speak_dispatched")(&mut state);
        assert!(state.speak_dispatched);

        stop(&mut state);
        assert!(
            dispatch_stop(&state, &sender),
            "発話中でも停止要求を発送できる"
        );
    }

    #[test]
    fn run_speak_reports_success() {
        let tts: &'static RecordingTts = Box::leak(Box::new(RecordingTts::default()));
        let (sender, rx) = sender_with_channel();
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "発話テスト".to_string());
        speak(&mut state);

        let (generation, text) = dispatch_speak(&state, &sender).expect("要求がある");
        rx.recv().expect("mark_speak_dispatched")(&mut state);
        run_speak(tts, generation, text, &sender);
        rx.recv().expect("完了が送られる")(&mut state);

        assert_eq!(state.status, fandhe_core::screens::tts_speak::SPOKE_STATUS);
        assert_eq!(state.error, "");
        assert!(!state.speak_dispatched);
        assert_eq!(tts.spoken.lock().unwrap().as_slice(), ["発話テスト"]);
    }

    #[test]
    fn run_speak_reports_failure_without_faking_success() {
        let tts: &'static FailingTts = Box::leak(Box::new(FailingTts));
        let (sender, rx) = sender_with_channel();
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "失敗する".to_string());
        speak(&mut state);

        let (generation, text) = dispatch_speak(&state, &sender).expect("要求がある");
        rx.recv().expect("mark_speak_dispatched")(&mut state);
        run_speak(tts, generation, text, &sender);
        rx.recv().expect("失敗でも完了は送られる")(&mut state);

        assert!(state.status.is_empty(), "got: {}", state.status);
        assert!(state.error.contains("失敗しました"), "{}", state.error);
        assert!(state.speak_text.is_none());
    }

    #[test]
    fn run_stop_reports_stopped() {
        let tts = RecordingTts::default();
        let (sender, rx) = sender_with_channel();
        let mut state = TtsSpeakState::default();
        stop(&mut state);

        assert!(dispatch_stop(&state, &sender));
        rx.recv().expect("mark_stop_dispatched")(&mut state);
        run_stop(&tts, &sender);
        rx.recv().expect("完了が送られる")(&mut state);

        assert_eq!(
            state.status,
            fandhe_core::screens::tts_speak::STOPPED_STATUS
        );
        assert!(!state.stop_requested);
    }

    #[test]
    fn fail_speak_completes_with_error() {
        let (sender, rx) = sender_with_channel();
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "読み上げられない".to_string());
        speak(&mut state);

        let (generation, _text) = dispatch_speak(&state, &sender).expect("要求がある");
        rx.recv().expect("mark_speak_dispatched")(&mut state);
        fail_speak(&sender, generation, UNAVAILABLE);
        rx.recv().expect("完了が送られる")(&mut state);

        assert!(state.speak_text.is_none());
        assert!(state.error.contains(UNAVAILABLE), "{}", state.error);
        // 「読み上げました」という嘘の成功を出していないこと。
        assert!(state.status.is_empty(), "{}", state.status);
    }

    #[test]
    fn fail_stop_completes_with_error() {
        let (sender, rx) = sender_with_channel();
        let mut state = TtsSpeakState::default();
        stop(&mut state);

        assert!(dispatch_stop(&state, &sender));
        rx.recv().expect("mark_stop_dispatched")(&mut state);
        fail_stop(&sender, UNAVAILABLE);
        rx.recv().expect("完了が送られる")(&mut state);

        assert!(!state.stop_requested);
        assert!(state.error.contains(UNAVAILABLE), "{}", state.error);
    }

    #[test]
    fn worker_loop_serializes_speak_and_stop_in_enqueue_order() {
        // codex-review P1 指摘の再発防止: 発話・停止をそれぞれ独立した
        // `std::thread::spawn` で扱っていた旧版は、spawn した順序と実際に
        // スケジューラが `Tts::speak`/`Tts::stop` を実行する順序が一致する
        // 保証が無く、発話の直後に停止を発送しても停止が先に実行されてしまう
        // 競合があった。`worker_loop` は 1 本の FIFO チャンネルから受信した
        // 順にこの 2 つを同じスレッドで逐次呼ぶため、発送順序と実行順序が
        // 必ず一致することを固定する。
        struct OrderRecordingTts {
            order: std::sync::Mutex<Vec<&'static str>>,
        }
        impl Tts for OrderRecordingTts {
            fn speak(&self, _text: &str) -> Result<u64, DeviceError> {
                // 「まだ子プロセスを起動していない」隙間を模して記録前に少し待つ
                // （旧版はこの隙間で停止スレッドが先に走ってしまった）。
                std::thread::sleep(std::time::Duration::from_millis(30));
                self.order
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push("speak");
                Ok(1)
            }
            fn stop(&self) -> Result<(), DeviceError> {
                self.order
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push("stop");
                Ok(())
            }
        }

        let tts: &'static OrderRecordingTts = Box::leak(Box::new(OrderRecordingTts {
            order: std::sync::Mutex::new(Vec::new()),
        }));
        let (tx, rx) = mpsc::channel::<TtsCommand>();
        std::thread::spawn(move || worker_loop(rx, || Ok(tts as &dyn Tts)));

        let (sender, out_rx) = sender_with_channel();
        tx.send(TtsCommand::Speak {
            generation: 1,
            text: "x".to_string(),
            sender: sender.clone(),
        })
        .expect("送信できる");
        tx.send(TtsCommand::Stop {
            sender: sender.clone(),
        })
        .expect("送信できる");

        let mut state = TtsSpeakState::default();
        for _ in 0..2 {
            out_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("完了が届く")(&mut state);
        }

        assert_eq!(
            tts.order.lock().unwrap().as_slice(),
            ["speak", "stop"],
            "発送順序どおりに speak → stop の順で実行される必要がある"
        );
    }

    #[test]
    fn shared_tts_reports_failure_instead_of_faking_success() {
        // 接続できた場合は発話できること、できない場合（Linux CI・想定外環境）は
        // ダミーで成功を装わずエラーを返すことを確認する（#859 の P1 指摘と同型の担保）。
        match shared_tts() {
            Ok(tts) => assert!(tts.speak("fandhe tts self check").is_ok()),
            Err(message) => assert_eq!(message, UNAVAILABLE),
        }
    }

    #[test]
    fn stop_shared_tts_does_not_panic_before_connection() {
        // まだ `shared_tts()` を呼んでいない（`TTS.get()` が `None`）プロセスで
        // `on_exit` から呼ばれても panic しないことの固定（codex-review P1 指摘の是正）。
        // `TTS` はプロセス内で共有される static のため、他テストが先に初期化済みの
        // 場合は分岐が変わるだけで、いずれの場合も panic しないことを見る。
        stop_shared_tts();
    }

    #[test]
    fn tts_speak_effect_completes_without_blocking_caller() {
        // UI スレッド解放の固定: `tts_speak_effect` 自体は `STARTUP_FAILURE_GRACE`
        // （200ms）分ブロックせずに戻ることを確認する（codex-review P1・Cursor Bugbot
        // 指摘の是正）。実際の発話完了は別スレッドから非同期に届く。
        let (sender, rx) = sender_with_channel();
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "非同期テスト".to_string());
        speak(&mut state);

        let started = std::time::Instant::now();
        tts_speak_effect(&state, &sender);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(50),
            "tts_speak_effect が UI スレッドをブロックした: {:?}",
            started.elapsed()
        );

        // mark_speak_dispatched → 発話結果、の 2 件が届くまで待つ（別スレッドの完了待ち）。
        rx.recv().expect("mark_speak_dispatched が送られる")(&mut state);
        assert!(state.speak_dispatched);
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("発話結果が届く")(&mut state);
        assert!(!state.speak_dispatched);
        assert!(state.speak_text.is_none());
    }

    #[test]
    fn tts_speak_effect_dispatches_stop_in_same_frame_as_in_flight_speak() {
        // codex-review P1・Cursor Bugbot 指摘の是正の統合確認: 発話を発送した
        // 直後のフレームで停止要求を立てても、同じ `tts_speak_effect` 呼び出しで
        // 停止側の `mark_stop_dispatched` が届く（発話完了を待たされない）。
        // 本テストはプロセス共有の `COMMAND_QUEUE`（実ワーカースレッド）を経由する
        // 統合確認であり、ワーカーの発話完了（`finish_speak`）が同じチャンネルへ
        // 混入することがある（#966）。
        let (sender, rx) = sender_with_channel();
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "長い発話".to_string());
        speak(&mut state);
        tts_speak_effect(&state, &sender);
        rx.recv().expect("mark_speak_dispatched が送られる")(&mut state);
        assert!(state.speak_dispatched);

        stop(&mut state);
        assert!(state.speak_text.is_none(), "停止で発話要求は打ち切られる");

        // `tts_speak_effect` の呼び出し自体（`dispatch_stop` の同期送信のみで
        // 完結する）が発話完了を待ってブロックしていないかは、下の try_recv
        // ループでは検出できない（呼び出しがブロックして戻っても、戻った時点で
        // 完了がチャンネルに揃っていればループはそのまま通過してしまう。
        // codex-review P2 指摘 #976 の是正）。呼び出し自体の経過時間で
        // 独立に確認する。呼び出し後の完了混入待ち（下の try_recv ループ）とは
        // 別物のため、このタイマーは呼び出しの前後だけを計測する。
        let started = std::time::Instant::now();
        tts_speak_effect(&state, &sender);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(50),
            "停止の発送が発話の完了待ちでブロックされた: {:?}",
            started.elapsed()
        );
        // `mark_stop_dispatched` は `tts_speak_effect` の中で同期送信されるため、
        // 戻った時点で必ずチャンネルに入っている（ブロッキング待ちも時間閾値も
        // 要らない）。ただし 1 回目の呼び出しでプロセス共有のワーカーキューへ
        // 渡した発話の完了（`finish_speak`。`speak_text` が None のため状態には
        // 作用しない）が先に届いていることがあるので、`stop_dispatched` が立つ
        // まで溜まった完了を順に適用する。発送順 → キュー投入 → `finish_stop` の
        // 順序は happens-before で固定されるため、`stop_dispatched` を戻す
        // `finish_stop` がこのループ内で先に届くことはない。
        while !state.stop_dispatched {
            rx.try_recv()
                .expect(
                    "同じフレームで mark_stop_dispatched が送られる（停止が発話完了待ちでブロックされている）",
                )(&mut state);
        }
    }
}
