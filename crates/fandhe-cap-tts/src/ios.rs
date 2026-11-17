//! iOS 向け読み上げ実装（`AVSpeechSynthesizer`、Issue #866）。`crate::driver` の `platform_tts` が iOS 経路で
//! [`IosTts`] を返す。`ios/fandhe_ios_tts.m` を `unsafe extern "C"` 2 関数で呼び、`AVSpeechSynthesizerDelegate` の完了を
//! `fandhe_rust_tts_outcome`（`.m` から呼ばれる受け口）で受けて [`TtsOutcome`] を解決する。呼び出し元は driver の
//! TTS ワーカースレッド（`worker_loop`）。`unsafe` は FFI の呼び出しと受け口の `#[unsafe(no_mangle)]` に限る。

#[cfg(target_os = "ios")]
use std::sync::atomic::AtomicU64;
use std::sync::{Condvar, Mutex};

#[cfg(target_os = "ios")]
use crate::contract::Tts;
use crate::error::TtsError;

// SAFETY: `ios/fandhe_ios_tts.m` が定義する C ABI（`build.rs` が iOS ターゲットの
// みコンパイル・リンクする）。契約は下記 `IosTts` の
// 各メソッドのコメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_tts_speak(text: *const core::ffi::c_char, request_generation: u64) -> i32;
    fn fandhe_ios_tts_stop() -> i32;
}

/// [`fandhe_ios_tts_speak`] が返す世代 id ごとの「真の最終結果が届いたか」を
/// 仲介する（Android `android.rs` の `Mailbox` と同じ動機だが、
/// `AVSpeechSynthesizerDelegate` は成功と中断（`Ok(())` に写像、`IosTts` doc 参照）
/// しか通知しないため、運ぶ値は無く「到着したか」だけを追跡すれば足りる）。
struct TtsOutcome {
    state: Mutex<TtsOutcomeState>,
    condvar: Condvar,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TtsOutcomeState {
    /// 追跡していない（未登録・既に消費済み）。
    Idle,
    /// `generation` の結果待ち。
    Pending(u64),
    /// `generation` の結果が到着済み（未消費）。
    Ready(u64),
}

impl TtsOutcome {
    const fn new() -> Self {
        Self {
            state: Mutex::new(TtsOutcomeState::Idle),
            condvar: Condvar::new(),
        }
    }

    /// `generation` の結果待ちを無条件に登録する（Android `Mailbox::begin_at` と
    /// 同じく、単一飛行の保証は呼び出し元（`IosTts::speak` は TTS ワーカースレッド
    /// 1 本に直列化済み）に委ねる）。登録済みの旧世代の待機があれば `notify_all` で
    /// 即座に起こし、「別世代に置き換わったら即座に `Ok(())`」契約
    /// （`Tts::wait_outcome` doc 参照）を満たす。
    fn begin(&self, generation: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *state = TtsOutcomeState::Pending(generation);
        self.condvar.notify_all();
    }

    /// [`begin`](Self::begin) で登録した世代の結果を待たず終わらせる
    /// （`fandhe_ios_tts_speak` 自体が同期的に失敗し、コールバックが二度と来ない
    /// と分かった場合に使う。Android `Mailbox::cancel` と同じ動機）。
    fn cancel(&self, generation: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *state == TtsOutcomeState::Pending(generation) {
            *state = TtsOutcomeState::Idle;
        }
    }

    /// `.m` 側の delegate コールバック（[`fandhe_rust_tts_outcome`]）から呼ぶ。
    /// 現在 `Pending` な世代と一致する場合のみ `Ready` へ進める。不一致（既に
    /// 別世代へ置き換わった・タイムアウトで畳まれた等）は古いコールバックとして
    /// 無視する。
    fn resolve(&self, generation: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *state == TtsOutcomeState::Pending(generation) {
            *state = TtsOutcomeState::Ready(generation);
            self.condvar.notify_all();
        }
    }

    /// `generation` の結果が届くまでブロッキングで待つ。追跡していない
    /// （未登録・空発話で一度も [`begin`](Self::begin) されていない・既に別世代へ
    /// 置き換わった）場合は古い待機として即座に `Ok(())` を返す
    /// （`Tts::wait_outcome` の既定実装と同じ契約）。
    ///
    /// **時間による打ち切りは行わない**（codex-review P1・Bugbot Medium 是正、
    /// PR #869 3 巡目: 発話には長さの上限が無く、`CommandTts`（macOS）の
    /// `wait_outcome` も完了待ちに時間上限を持たない。固定時間で打ち切ると、
    /// 上限を超えて続く正常な発話が「まだ鳴っているのに失敗表示」になる）。
    /// 終了する条件は次のいずれかのみ: (1) 対象世代の結果が届く（[`resolve`]、
    /// 発話完了 or `stop`/後続 speak によるキャンセル）、(2) 対象世代が別の発話へ
    /// 置き換わる（[`begin`]、後続 speak による世代置換）。
    ///
    /// ponytail: `AVSpeechSynthesizerDelegate` のコールバック自体が欠落する
    /// ケース（オーディオセッションの強制割り込み等、理論上あり得るが未確認）への
    /// 安全網は持たない。delegate が来なくても、以後に `speak`/`stop` が
    /// 1 回でも発行されれば世代置換（(2)）で復帰する。実機で恒久的な滞留の報告が
    /// 出たら見直す。
    fn wait(&self, generation: u64) -> Result<(), TtsError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match *state {
                TtsOutcomeState::Ready(ready) if ready == generation => {
                    *state = TtsOutcomeState::Idle;
                    return Ok(());
                }
                TtsOutcomeState::Pending(pending) if pending == generation => {
                    // まだ自分の世代を追跡中。結果到着まで待つ（時間では諦めない）。
                }
                _ => return Ok(()),
            }
            state = self.condvar.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
}

static TTS_OUTCOME: TtsOutcome = TtsOutcome::new();

/// `ios/fandhe_ios_tts.m` の `AVSpeechSynthesizerDelegate` から呼ばれる
/// `extern "C"` エクスポート関数（Issue #866 是正、PR #869）。
/// `AVSpeechSynthesizerDelegate` の `didFinishSpeechUtterance:`（`cancelled == 0`）/
/// `didCancelSpeechUtterance:`（`cancelled != 0`）の双方から呼ばれ、いずれも
/// [`TTS_OUTCOME`] を `Ok(())` として解決する（`AVSpeechSynthesizerDelegate` は
/// 発話失敗を通知する API を持たないため、`IosTts::speak` が返した世代の
/// 非同期失敗は存在しない前提。`.m` ファイル冒頭コメント参照）。
///
/// 呼び出しスレッドは AVFoundation の delegate 実行スレッド（メインスレッド想定だが
/// Apple のドキュメントに保証の明記が無い）。[`TtsOutcome`] は `Mutex`/`Condvar` で
/// 保護されているため、どのスレッドから呼ばれても安全（`unsafe` 操作を含まない。
/// `generation`/`cancelled` は素の整数値でポインタを経由しない）。
///
/// `#[allow(unsafe_code)]` は `#[unsafe(no_mangle)]`（シンボル名の固定）が `lib.rs` の
/// iOS `deny(unsafe_code)` 対象になるための宣言上の要請であり、関数本体は
/// `unsafe` 操作を一切含まない（Android 側 `android.rs` の JNI エクスポート
/// 関数と同じ扱い、coding-rust.md の FFI 例外規定）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "C" fn fandhe_rust_tts_outcome(generation: u64, _cancelled: i32) {
    TTS_OUTCOME.resolve(generation);
}

/// iOS 実読み上げ実装（`AVSpeechSynthesizer`、Issue #866）。
/// `driver.rs` の TTS ワーカースレッド（`worker_loop`、proxy の FIFO キューを
/// 処理する唯一のスレッド）から呼ばれる。Rust → `.m` の呼び出し（`speak`/`stop`）は
/// この 1 本のスレッドに直列化される契約のため、本実装自体は追加の排他制御を
/// 持たない。ただし `.m` 側は `isSpeaking` 時にメインキューへ積む遅延発話ブロック
/// （`fandhe_ios_tts_speak` 参照）が別スレッド（メインキュー）で走るため、ワーカー
/// スレッドとの間で `g_tts_lock` による排他制御を持つ（`.m` ファイルの
/// `g_tts_lock` 宣言部コメント参照）。
///
/// `IosMotion` と違い `is_main_thread` 検査を付けない:
/// `UIPasteboard`/`CMMotionManager` と異なり `AVSpeechSynthesizer` は main
/// スレッド専用の契約を Apple のドキュメントに持たない。加えて、`stop_shared_tts()`
/// はアプリ終了時に main スレッドから TTS ワーカーへ要求を送りその完了を待つため
/// （`driver.rs::stop_shared_tts` 参照）、ここで `dispatch_sync(main, ...)` の
/// ような main スレッドへの同期委譲を挟むと、main が完了を待つ一方でワーカーの
/// 処理が main の空き待ちになる相互待ち（デッドロック）が起こり得る
/// （#866 実装計画 §3-2）。
///
/// 世代 id の採番元は `IosTts` インスタンス毎ではなく static
/// [`NEXT_TTS_GENERATION`] にする（codex-review 指摘、PR #869 5 巡目是正）:
/// 結果の保持先 [`TTS_OUTCOME`] はプロセス全体で 1 個の static のため、採番だけ
/// インスタンス毎にすると、複数の `IosTts` インスタンス（`Tts` は公開 API で
/// 誰でも構築できる）が同じ id を独立に払い出しうる。その結果、無関係な
/// 2 インスタンス A・B が同じ id で `speak`/`wait_outcome` を呼ぶと、B の待機が
/// A の結果を誤って回収してしまう（「他の世代の結果を絶対に回収しない」契約違反）。
#[cfg(target_os = "ios")]
#[derive(Default)]
pub struct IosTts;

/// [`IosTts::speak`] が払い出す世代 id の採番元（`IosTts` doc 参照）。
#[cfg(target_os = "ios")]
static NEXT_TTS_GENERATION: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "ios")]
impl Tts for IosTts {
    #[allow(unsafe_code)]
    fn speak(&self, text: &str) -> Result<u64, TtsError> {
        // 内部 NUL を含むテキストは C 文字列にできない（`fandhe-cap-clipboard` の `IosClipboard::set_text` と
        // 同じ判定。`CommandTts::speak` の契約と対称に揃える）。
        if text.contains('\0') {
            return Err(TtsError::InvalidInput("tts text"));
        }

        // 世代 id は `Tts::speak` の契約により単調増加で払い出す。`.m` 側へそのまま
        // 渡し、`AVSpeechUtterance` に紐付けた上で delegate コールバック
        // （[`fandhe_rust_tts_outcome`]）から [`TTS_OUTCOME`] を解決する識別子として
        // 使う（codex-review 指摘 PR #869 是正、`TtsOutcome` doc 参照）。
        let generation = NEXT_TTS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // 空文字列・空白のみは発話するものが無いため OS を呼ばずに戻る（`Tts::speak` の契約）。
        // `TTS_OUTCOME` には一切登録しないため、`wait_outcome(generation)` は
        // 「追跡していない ID」として即座に `Ok(())` を返す。
        if text.trim().is_empty() {
            return Ok(generation);
        }

        let Ok(c_text) = std::ffi::CString::new(text) else {
            // 上の NUL 検査で弾いているため到達しないはずだが、契約違反を検出した
            // 経路として安全側で扱う。
            return Err(TtsError::InvalidInput("tts text"));
        };

        // コールバックが呼ばれる前に登録する（JNI 版 `AndroidTts::speak` と同じ理由:
        // `fandhe_ios_tts_speak` が返った直後に delegate コールバックが届いても
        // 取りこぼさない）。
        TTS_OUTCOME.begin(generation);

        // SAFETY: `fandhe_ios_tts_speak` の契約（`.m` ファイル冒頭コメント）:
        // 引数は NUL 終端の UTF-8 文字列へのポインタで、ObjC 側は内部でコピーを
        // 取るため本関数の戻り後に `c_text` を解放してよい（所有権は移らない）。
        // `c_text` はこの呼び出しの間スコープ内で生存する。
        let code = unsafe { fandhe_ios_tts_speak(c_text.as_ptr(), generation) };

        match code {
            // 0 はキュー投入成功に過ぎない。真の完了・中断は
            // `fandhe_rust_tts_outcome` が非同期に届けるため、ここでは確定させない
            // （codex-review 指摘 PR #869: 旧実装はここで即成功にしていた）。
            0 => Ok(generation),
            1 => {
                // コールバックは二度と来ないため、登録した待ちを畳んでおく
                // （Android `Mailbox::cancel` と同じ理由）。
                TTS_OUTCOME.cancel(generation);
                Err(TtsError::Unsupported)
            }
            // 2 やその他未知のコードも安全側で内部エラーに畳む（`.m` の契約）。
            _ => {
                TTS_OUTCOME.cancel(generation);
                Err(TtsError::Internal("tts speak"))
            }
        }
    }

    #[allow(unsafe_code)]
    fn stop(&self) -> Result<(), TtsError> {
        // SAFETY: `fandhe_ios_tts_stop` は引数を取らず、副作用は発話停止のみ
        // （`.m` ファイル冒頭コメント）。停止によって中断された発話があれば
        // `AVSpeechSynthesizerDelegate` の `didCancelSpeechUtterance:` が自動的に
        // 発火し、その世代の `TTS_OUTCOME` を解決する（`.m` 側 doc 参照）。
        match unsafe { fandhe_ios_tts_stop() } {
            0 => Ok(()),
            _ => Err(TtsError::Internal("tts stop")),
        }
    }

    /// `speak` が返した世代の真の最終結果（発話完了 or 中断）を待つ
    /// （`Tts::wait_outcome` の契約）。追跡していない世代（未登録・空発話・既に
    /// 別世代へ置き換わり済み）は [`TtsOutcome::wait`] が即座に `Ok(())` を返す。
    /// 時間による打ち切りは行わない（`TtsOutcome::wait` doc 参照、codex-review
    /// P1・Bugbot Medium 是正）。
    fn wait_outcome(&self, generation: u64) -> Result<(), TtsError> {
        TTS_OUTCOME.wait(generation)
    }
}

#[cfg(test)]
mod tts_outcome_tests {
    use super::TtsOutcome;
    use std::time::Duration;

    /// ObjC 側の delegate コールバックを経由しない、`TtsOutcome` 単体のロジック
    /// 固定テスト（`.m` のビルドを要さず iOS ターゲットでの `cargo test` で回せる）。
    /// static `TTS_OUTCOME` は使わずローカルに構築する（Android `tts_tests` の
    /// P2 是正と同じ理由: 並列実行時の他テストとの干渉を避ける）。

    #[test]
    fn untracked_generation_resolves_immediately() {
        let outcome = TtsOutcome::new();
        assert!(outcome.wait(1).is_ok());
    }

    #[test]
    fn resolve_completes_matching_generation() {
        let outcome = TtsOutcome::new();
        outcome.begin(1);
        outcome.resolve(1);
        assert!(outcome.wait(1).is_ok());
    }

    #[test]
    fn cancel_stops_tracking_without_resolve() {
        // 同期失敗（`fandhe_ios_tts_speak` が非 0）時の経路: コールバックが二度と
        // 来ないため `cancel` で畳み、以後の `wait` は「追跡していない」として
        // 即座に成功する。
        let outcome = TtsOutcome::new();
        outcome.begin(1);
        outcome.cancel(1);
        assert!(outcome.wait(1).is_ok());
    }

    #[test]
    fn resolve_ignores_mismatched_generation() {
        // 古いコールバック（既に別世代へ置き換わった後に届く finish/cancel）は
        // 無視され、新しい世代の追跡状態を壊さない。
        let outcome = TtsOutcome::new();
        outcome.begin(1);
        outcome.begin(2);
        outcome.resolve(1);
        assert!(
            outcome.wait(1).is_ok(),
            "1 は追跡されていないため即座に成功"
        );
        // 2 はまだ resolve されていないため、別スレッドで待つ。
    }

    #[test]
    fn begin_wakes_up_a_waiter_blocked_on_the_replaced_generation() {
        // `wait_outcome(A)` が待機中に `speak(B)` が来て `begin` が世代を
        // 上書きした場合、（`wait` は時間で打ち切らないため）`notify_all` により
        // 即座に起こされて `Ok(())` で戻ることを固定する（Android
        // `wait_or_ok_wakes_immediately...` と同型の担保。この通知が効いて
        // いなければ `waiter.join()` はテストプロセスごと永久にハングする）。
        use std::sync::Arc;
        let outcome = Arc::new(TtsOutcome::new());
        outcome.begin(1);

        let waiter = {
            let outcome = Arc::clone(&outcome);
            std::thread::spawn(move || outcome.wait(1))
        };
        std::thread::sleep(Duration::from_millis(50));

        let started = std::time::Instant::now();
        outcome.begin(2);
        assert!(waiter.join().expect("panic しない").is_ok());
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "begin による置き換えから wait が戻るまで長く掛かりすぎている: {:?}",
            started.elapsed()
        );
    }
}
