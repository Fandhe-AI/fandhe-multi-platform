//! Android 向け読み上げ実装（`TextToSpeech`、Issue #866）。`crate::driver` の `platform_tts` が Android 経路で
//! [`AndroidTts`] を返し、Java 側 `TtsHelper.request(Activity, long, int, String)`（static）を JNI で呼んで、
//! `nativeOnTtsAccepted` / `nativeOnTtsResult` で返った結果を 2 本の mailbox で `Tts` の同期契約へ戻す。
//! Activity のライフサイクルへの参加は `TtsLifecycle`（`FeatureLifecycleProvider` 派生）が担う
//! （`TtsHelper` が Activity ごとの状態を持つ。Helper が無い要求は status 2 で確定し、待たせない）。
//!
//! `unsafe` は JNI 受け口の `#[unsafe(no_mangle)]` だけで、JNI 呼び出しは共通層
//! `fandhe_platform_android::with_activity_class` に閉じている。no-op スタブは置かない
//! （どの native も Rust の `request` を経た後でしか呼ばれない）。

#[cfg(target_os = "android")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

#[cfg(target_os = "android")]
use fandhe_platform_android::{JniCallFailed, with_activity_class};

#[cfg(target_os = "android")]
use crate::contract::Tts;
use crate::error::TtsError;

/// 1 件の request/response 往復を仲介する mailbox のスロット。運ぶ値の型 `T` は
/// クリップボード（`Option<String>`）・TTS（`()`）で異なるため汎用化する
/// （Issue #866、`ClipboardMailbox` を一般化したもの。挙動・doc は据え置き）。
enum Slot<T> {
    /// 要求なし。
    Idle,
    /// `id` の要求が進行中（Java 側のコールバック待ち）。
    Pending(u64),
    /// `id` の結果が到着済み。
    Ready(u64, Result<T, TtsError>),
}

/// request/response 往復を仲介する mailbox（`CaptureMailbox` と同型だが、
/// Activity result のライフサイクルを持たない単純な往復専用）。
struct Mailbox<T> {
    slot: Mutex<Slot<T>>,
    condvar: Condvar,
    /// `wait` がタイムアウトした際のエラー文言（用途ごとに変える。例:
    /// `"clipboard timeout"` / `"tts timeout"`）。
    timeout_label: &'static str,
}

impl<T> Mailbox<T> {
    /// 空の mailbox を構築する（`static` 初期化子から呼べるよう `const fn`）。
    const fn new(timeout_label: &'static str) -> Self {
        Self {
            slot: Mutex::new(Slot::Idle),
            condvar: Condvar::new(),
            timeout_label,
        }
    }

    /// 呼び出し元が別の経路（`AndroidTts` の
    /// 共有カウンタ等）で発行済みの ID を引き継ぐ場合に使う。`Idle` のときのみ
    /// `Pending(id)` へ遷移して `true` を返し、進行中なら `false`
    /// （単一飛行の防御）。
    fn begin_at_if_idle(&self, id: u64) -> bool {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, Slot::Idle) {
            *slot = Slot::Pending(id);
            true
        } else {
            false
        }
    }

    /// 現在の状態を無条件に `Pending(id)` で上書きする（単一飛行は呼び出し元が
    /// 別途保証している前提。`AndroidTts` の読み上げ往復（Issue #866 案A）で、
    /// 「受理」の mailbox とは独立に「最終結果」の mailbox を登録する用途に使う。
    /// `begin_at_if_idle` と違い busy を返さない点に注意。
    ///
    /// 上書きされた旧 ID を待っているスレッドがあれば `notify_all` で即座に
    /// 起こす（codex-review P1 再指摘: 通知しないと [`wait_or_ok`](Mailbox::wait_or_ok)
    /// の待機ループが世代の変化に気づくのが次の `wait_timeout` 満了（＝ここでは
    /// 実質タイムアウト全体）まで遅れ、`Tts::wait_outcome` の「置き換わり済みなら
    /// 即座に `Ok(())`」契約に反してしまう）。
    fn begin_at(&self, id: u64) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        *slot = Slot::Pending(id);
        self.condvar.notify_all();
    }

    /// [`deliver`](Self::deliver) と同じだが、`id` が現在の `Pending` と一致しない
    /// 場合は届けずに `false` を返す（`result` は呼び出し元へ返さず破棄する。
    /// 破棄してよい理由は呼び出し元 [`deliver`](Self::deliver) 参照）。`id` が
    /// 「受理」と「最終結果」のどちらの mailbox 宛か呼び出し側では判別できない
    /// 場合に、まず一方へ届けを試み、届かなければもう一方へ回すルーティング用
    /// （`AndroidTts` の `nativeOnTtsResult` 受信処理、Issue #866 案A）。
    fn try_deliver(&self, id: u64, result: Result<T, TtsError>) -> bool {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, Slot::Pending(pending_id) if pending_id == id) {
            *slot = Slot::Ready(id, result);
            self.condvar.notify_all();
            true
        } else {
            false
        }
    }

    /// JNI 呼び出し自体が失敗してコールバックが来ないと分かった時点で `Idle` へ戻す。
    /// `id` が現在の `Pending` と一致しない場合は無視する（別要求のスロットを畳まない）。
    fn cancel(&self, id: u64) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, Slot::Pending(pending_id) if pending_id == id) {
            *slot = Slot::Idle;
        }
    }

    /// Java 側コールバックスレッドから結果を届ける。ID 一致時のみ受理する
    /// （タイムアウト後の遅延到着が新しい要求の結果を上書きしない）。
    /// 一致しない場合は [`try_deliver`](Self::try_deliver) の戻り値（`false`）を
    /// 読み捨てる（届け先が無い＝もう誰も待っていないため、`result` は破棄してよい）。
    fn deliver(&self, id: u64, result: Result<T, TtsError>) {
        let _ = self.try_deliver(id, result);
    }

    /// `begin_at_if_idle` / `begin_at` を呼んだスレッドがその ID の結果を待つ。`timeout` 内に届かなければ
    /// スロットを `Idle` へ戻して（固着防止）エラーを返す。
    fn wait(&self, id: u64, timeout: Duration) -> Result<T, TtsError> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + timeout;
        loop {
            if matches!(*slot, Slot::Ready(ready_id, _) if ready_id == id) {
                let ready = std::mem::replace(&mut *slot, Slot::Idle);
                return match ready {
                    Slot::Ready(_, result) => result,
                    _ => unreachable!("直前に Ready であることを確認済み"),
                };
            }
            let now = Instant::now();
            if now >= deadline {
                if matches!(*slot, Slot::Pending(pending_id) if pending_id == id) {
                    *slot = Slot::Idle;
                }
                return Err(TtsError::Internal(self.timeout_label));
            }
            let (guard, _timeout_result) = self
                .condvar
                .wait_timeout(slot, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            slot = guard;
        }
    }
}

impl Mailbox<()> {
    /// `id` を追跡していなければブロックせず直ちに `Ok(())` を返し、追跡中なら
    /// 結果が届くまで待つ。
    ///
    /// [`crate::contract::Tts::wait_outcome`] の契約「対応する処理が既に無い
    /// （未起動・空発話等で登録されていない、または別の発話へ置き換わり済みの）
    /// 場合は古い待機として即座に `Ok(())` を返す」を [`TTS_OUTCOME_MAILBOX`] 用に
    /// 実装する（Issue #866 案A）。空文字列・空白のみの発話（Java を一切呼ばない）
    /// はこの mailbox に登録されないため、この分岐で即 `Ok(())` になる。
    ///
    /// **時間による打ち切りは行わない**（codex-review P1・Bugbot Medium 是正、
    /// PR #869 3 巡目: 発話には長さの上限が無く、`CommandTts`（macOS）の
    /// `wait_outcome` も完了待ちに時間上限を持たない。固定時間で打ち切ると、
    /// 上限を超えて続く正常な発話が「まだ鳴っているのに失敗表示」になる）。
    /// 終了する条件は次のいずれかのみ:
    /// (1) 対象世代 `id` の結果が届く（[`deliver`](Mailbox::deliver)/
    /// [`try_deliver`](Mailbox::try_deliver)、成功/失敗/`stop` によるキャンセル）、
    /// (2) 対象世代が別の発話 B へ置き換わる（[`begin_at`](Mailbox::begin_at)、
    /// 後続 speak による世代置換）。呼び出し元（`driver.rs::run_speak` が
    /// 切り出す背景スレッド）が固まらないよう、`Tts::stop` は必ずどちらかを
    /// 引き起こす契約（`stop` は現在発話中の世代を `onStop`/`didCancel` 経由で
    /// (1) へ、後続 `speak` は (2) へ倒す）。
    ///
    /// [`wait`](Mailbox::wait) をそのまま使わず専用ループにする理由（codex-review
    /// P1 再指摘）: `wait` は「自分が `begin_at_if_idle` した ID がそのまま `Ready` になる」
    /// 契約のみを見るため、待機中に [`begin_at`](Mailbox::begin_at) が同じ mailbox を
    /// 別の ID（後続の発話 B）へ無条件に差し替えても、それを検知せず古い ID の
    /// `Ready` をひたすら待ち続けてしまう。Java 側は新しい ID にしか応答しない
    /// ため、置き換えられた A 側の待機は目を覚まさず、`wait_outcome` の
    /// 「置き換わり済みなら即座に `Ok(())`」契約が崩れる。本ループは
    /// `notify_all`（`begin_at`/`deliver` 双方が呼ぶ）で起きるたびにスロットの ID を
    /// 再確認し、`id` と異なる `Pending`/`Ready`（＝別の世代に上書きされた）や
    /// `Idle`（＝既に誰かに消費された）を見たら古い待機として即座に `Ok(())` で抜ける。
    fn wait_or_ok(&self, id: u64) -> Result<(), TtsError> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match *slot {
                Slot::Ready(ready_id, _) if ready_id == id => {
                    let ready = std::mem::replace(&mut *slot, Slot::Idle);
                    return match ready {
                        Slot::Ready(_, result) => result,
                        _ => unreachable!("直前に Ready であることを確認済み"),
                    };
                }
                Slot::Pending(pending_id) if pending_id == id => {
                    // まだ自分の世代を追跡中。結果到着まで待つ（時間では諦めない）。
                }
                _ => {
                    // `Idle`、または `id` と異なる世代の `Pending`/`Ready`。
                    // 自分の世代はもう追跡されていない（置き換え済み・古い待機）
                    // ため、他の世代の状態には触れずそのまま即座に成功で抜ける。
                    return Ok(());
                }
            }
            slot = self.condvar.wait(slot).unwrap_or_else(|e| e.into_inner());
        }
    }
}

// --------------------------------------------------------------------------
// 読み上げ（Issue #866、Phase 11 型サンプル「読み上げ」の Android 経路）
// --------------------------------------------------------------------------
//
// `TextToSpeech` はクリップボードと同じく Java 側の system 資源だが、初期化が
// **非同期**（コンストラクタに渡す `OnInitListener` が後から呼ばれる）という
// クリップボードには無い制約を持つ。この往復では「初期化未完了時に `speak` が
// 偽の成功を返す」経路を作らないことが最重要（親 #864・#859 の再発防止点）。
//
// 設計（案A、PR #869 codex-review P1 是正）: 旧実装は `AndroidTts::speak` が
// `onInit` 完了まで同期的にブロックしていた。`speak`/`stop` は `driver.rs`
// の TTS ワーカースレッド（`worker_loop`）が同じ FIFO で逐次処理するため、
// 初期化が長引くと `stop` 要求がキューに残ったまま初期化完了後に
// 「キャンセル済みのテキストを発話してから停止」してしまい、Java 側
// `TtsHelper` の INITIALIZING 中の停止分岐へ通常経路から到達できなかった。
//
// 是正: `speak` は Java が要求を **受理** した時点（[`nativeOnTtsAccepted`]）で
// 即座に戻り、真の結果は [`Tts::wait_outcome`] が [`TTS_OUTCOME_MAILBOX`] を
// 別スレッドから待つ（`CommandTts`/`IosTts` と同じ「`speak` は開始で戻り、
// `wait_outcome` が最終結果を持つ」契約に揃える）。これにより `speak` が
// ワーカースレッドを占有する時間は「受理されるまで」（Java の UI スレッドへ
// ポストする程度、通常はミリ秒オーダー）に短縮され、直後にキューへ積まれた
// `stop` 要求を初期化完了を待たず即座に Java へ届けられる。
//
// mailbox は 2 本に分ける（[`TTS_ACCEPT_MAILBOX`]/[`TTS_OUTCOME_MAILBOX`]）:
// - `TTS_ACCEPT_MAILBOX`: JNI 往復 1 回分の応答を仲介する短命な mailbox。
//   SPEAK では [`nativeOnTtsAccepted`] の受理シグナルのみを待つ（Java は
//   受理を送った後、実処理へ進む前にこの mailbox の役目は終わる）。STOP は
//   受理という概念を持たず、[`nativeOnTtsResult`] の最終結果をそのままここで
//   受け取る（STOP は `onInit` を待たないため二段階に分ける必要が無い）。
// - `TTS_OUTCOME_MAILBOX`: SPEAK が受理された後の真の最終結果（`onInit` の
//   成否・`speak()` 呼び出し自体の成否）を仲介する。`speak` を呼んだ ID を
//   そのまま `wait_outcome` の世代として使う（`Tts::speak` の契約「戻り値は
//   この呼び出しを識別する世代 id」）ため、2 つの mailbox は同じ ID 空間
//   （[`AndroidTts::next_id`]）を共有する。
//
// [`Java_com_fandhe_platform_TtsHelper_nativeOnTtsResult`] は「この ID の
// 最終結果がどちらの mailbox 宛か」を判定せず、まず `TTS_ACCEPT_MAILBOX` へ
// 届けを試み（STOP の最終結果はここで受理される）、一致しなければ
// `TTS_OUTCOME_MAILBOX` へ回す（SPEAK が既に受理済みで、こちらが本来の宛先）。
// SPEAK の ID は受理シグナルの消費と同時に `TTS_ACCEPT_MAILBOX` 側が空くため、
// 受理後に届く最終結果が誤って `TTS_ACCEPT_MAILBOX` 側にマッチすることはない
// （Java は SPEAK について必ず受理 → 最終結果の順で呼ぶため、最終結果が届く
// 時点で受理は既に消費済みか届け済みであり、いずれにせよ `Pending` ではない）。
//
// Java 側（`TtsHelper.request`）は `TTS_STATE_INITIALIZING` 中に STOP が
// 届くと、保留中の SPEAK 要求をキャンセル（status 7）として畳んでから自身の
// 停止処理へ進む。キャンセルは失敗として扱わない（[`decode_tts_result`] 参照、
// `CommandTts::wait_outcome` doc の「ユーザー操作による停止は失敗として
// 扱わない」と同じ判断）。
//
// Activity 再生成時の結果配送（#881）: 再生成中の結果は旧 `ProxyApp` の
// `CompletionSender` へ送られ捨てられるが、`onDestroy` が `currentSpeakingRequestId`
// を確定させて `nativeOnTtsResult` を呼ぶため、新画面が引き継ぐべき実結果は
// 存在しない（`driver.rs` モジュール doc 参照）。
//
// 案A是正の続き（codex-review P1 再指摘、本 PR）: 案Aは「受理」と「結果」を
// 分離したが、Java 側 `speakNow` はなお `TextToSpeech.speak()` の戻り値
// （`SUCCESS`）＝キュー投入成功をそのまま `nativeOnTtsResult(0)` として送っており、
// これは合成完了ではない（`speak()` はキューへ積んだだけで戻る非同期 API、
// `TextToSpeech` のドキュメント）。是正: `TtsHelper` は
// `setOnUtteranceProgressListener` で `onDone`/`onError`/`onStop` を購読し、
// `speak()` の utteranceId（`"fandhe-" + requestId`）から `requestId` を復元して
// `nativeOnTtsResult` を呼ぶ（`TtsHelper.speakNow`/`utteranceListener` 参照）。
// `speakNow` 自身は `speak()` が `ERROR` を返した（キュー投入失敗、コールバックが
// 二度と来ない）場合のみ即座に `nativeOnTtsResult(5)` を送り、投入成功時は
// 何も送らず listener からの通知を待つ。

/// 読み上げ要求の種別（Java 側 `TtsHelper.request` の `op` と対）。
#[cfg(target_os = "android")]
const TTS_OP_SPEAK: i32 = 0;
/// 同上（停止）。
#[cfg(target_os = "android")]
const TTS_OP_STOP: i32 = 1;

/// JNI 往復 1 回分の応答（SPEAK は受理シグナル、STOP は最終結果）を待つ上限。
/// Java 側は `runOnUiThread` へポストした直後、実処理（`onInit` 待ち等）に
/// 入る前に必ずこれを返すため、UI スレッドの通常の込み具合以上には掛からない
/// 想定（ponytail: 実測根拠のない安全弁。実機で timeout の報告が出たら見直す）。
///
/// 「開始まで」（受理されるまで）にのみ掛ける期限であり、「発話完了まで」には
/// 掛けない（codex-review P1・Bugbot Medium 是正、PR #869 3 巡目）。受理後の
/// 真の最終結果（[`TTS_OUTCOME_MAILBOX`]、`wait_outcome`）は時間で打ち切らず、
/// 完了・失敗・キャンセル・世代置換でのみ終了する（`Mailbox::wait_or_ok` doc 参照）。
/// Java 側 `TtsHelper.TTS_INIT_TIMEOUT_MS`（15 秒）は本定数とは別に、
/// 「初期化待ち中に保留した SPEAK 要求」自体の失効判定にのみ使う
/// （`expireStaleTtsRequest` 参照。これも「発話完了まで」ではなく「初期化完了 or
/// 受理まで」の期限）。この期限に達した場合、Java 側は保留を破棄するだけでなく
/// `nativeOnTtsResult(requestId, 3)` を送って [`TTS_OUTCOME_MAILBOX`] の
/// `wait_outcome` を確定させる（codex-review 指摘、PR #869 4 巡目是正: 送らないと
/// `wait_outcome` は時間で打ち切らないため、世代置換まで無期限に残ってしまう）。
#[cfg(target_os = "android")]
const TTS_ACCEPT_TIMEOUT: Duration = Duration::from_secs(5);

/// JNI 往復 1 回分の応答を仲介する mailbox（モジュール doc 参照）。
static TTS_ACCEPT_MAILBOX: Mailbox<()> = Mailbox::new("tts accept timeout");
/// SPEAK が受理された後の真の最終結果を仲介する mailbox（モジュール doc 参照）。
static TTS_OUTCOME_MAILBOX: Mailbox<()> = Mailbox::new("tts timeout");

/// [`TTS_ACCEPT_MAILBOX`]/[`TTS_OUTCOME_MAILBOX`] が共有する ID 空間
/// （モジュール doc 参照）。`AndroidTts` インスタンス毎ではなく static にする
/// 理由（codex-review 指摘、PR #869 5 巡目是正）: 結果の保持先
/// （`TTS_ACCEPT_MAILBOX`/`TTS_OUTCOME_MAILBOX`）はプロセス全体で 1 個の static
/// のため、採番だけをインスタンス毎にすると複数の `AndroidTts` インスタンス
/// （`Tts` は公開 API で誰でも構築できる）が同じ id を独立に払い出しうる。
/// その結果、無関係な 2 インスタンス A・B が同じ id で `speak`/`wait_outcome`
/// を呼ぶと、B の待機が A の結果を誤って回収してしまう（「他の世代の結果を
/// 絶対に回収しない」契約違反）。単調増加のため、mailbox をまたいで再利用されない。
#[cfg(target_os = "android")]
static NEXT_TTS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Android 実読み上げ実装（`TextToSpeech`、Issue #866）。
/// `driver.rs` の TTS ワーカースレッド（`worker_loop`）から呼ばれる。
#[cfg(target_os = "android")]
#[derive(Default)]
pub struct AndroidTts;

#[cfg(target_os = "android")]
impl AndroidTts {
    fn next_id(&self) -> u64 {
        NEXT_TTS_GENERATION.fetch_add(1, Ordering::Relaxed)
    }

    /// Java 側 `TtsHelper.request(Activity, long, int, String)`（static）を JNI で呼ぶだけの
    /// 薄いヘルパー（mailbox の登録・解除は呼び出し元が担う）。JNI 呼び出しは共通層
    /// `fandhe_platform_android::with_activity_class` に閉じており、`unsafe` はここに無い。
    /// `TtsHelper` が無い（生成前・破棄後）要求は Java 側が status 2 で確定させる。
    fn call_tts_request(&self, id: u64, op: i32, text: &str) -> Result<(), TtsError> {
        let Some(app) = fandhe_platform_android::android_app() else {
            return Err(TtsError::Unsupported);
        };
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.TtsHelper"),
            |env, activity, class| -> Result<(), JniCallFailed> {
                let j_text = env.new_string(text)?;
                let r = env.call_static_method(
                    class,
                    jni::jni_str!("request"),
                    jni::jni_sig!((android.app.Activity, long, int, java.lang.String) -> ()),
                    &[
                        jni::objects::JValue::Object(activity),
                        jni::objects::JValue::Long(id as jni::sys::jlong),
                        jni::objects::JValue::Int(op as jni::sys::jint),
                        jni::objects::JValue::Object(&j_text),
                    ],
                );
                if r.is_err() && env.exception_check() {
                    // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                    env.exception_clear();
                }
                r?;
                Ok(())
            },
        )
        .map_err(|_| TtsError::Internal("tts jni"))
    }
}

#[cfg(target_os = "android")]
impl Tts for AndroidTts {
    fn speak(&self, text: &str) -> Result<u64, TtsError> {
        // 内部 NUL を含むテキストは JNI 文字列としては渡せてしまうが、iOS 側
        // （`CString::new` が拒否）と判定を揃えるため Rust 側で先に弾く
        // （#866 実装計画 §3-5、FFI 境界の対称性）。
        if text.contains('\0') {
            return Err(TtsError::InvalidInput("tts text"));
        }

        let id = self.next_id();

        // 空文字列・空白のみは発話するものが無いため OS を呼ばずに戻る（`Tts::speak`
        // の契約）。mailbox には一切登録しないため、`wait_outcome(id)` は
        // 「追跡していない ID」として即座に `Ok(())` を返す（`wait_or_ok` 参照）。
        if text.trim().is_empty() {
            return Ok(id);
        }

        if !TTS_ACCEPT_MAILBOX.begin_at_if_idle(id) {
            // 画面側が `speak_dispatched`/`stop_dispatched` で二重要求を防いでいる
            // ため通常到達しないが、多重呼び出しに対する防御として単一飛行を守る。
            return Err(TtsError::Internal("tts busy"));
        }
        // 受理される前に登録する（JNI 呼び出しが返った直後に Java 側の応答が
        // 届いても取りこぼさない。呼び出しはこの後でようやく Java へ渡る）。
        TTS_OUTCOME_MAILBOX.begin_at(id);

        if let Err(e) = self.call_tts_request(id, TTS_OP_SPEAK, text) {
            // JNI 呼び出し自体が失敗した（メソッド未検出・例外送出等）。コールバックは
            // 来ないため `wait` せず即座に両 mailbox を解放する。
            TTS_ACCEPT_MAILBOX.cancel(id);
            TTS_OUTCOME_MAILBOX.cancel(id);
            return Err(e);
        }

        match TTS_ACCEPT_MAILBOX.wait(id, TTS_ACCEPT_TIMEOUT) {
            Ok(()) => Ok(id),
            Err(e) => {
                // 受理シグナルすら届かなかった（UI スレッド固着等の異常系）。
                // 真の結果を待つ相手がいなくなるため outcome 側も畳む。
                TTS_OUTCOME_MAILBOX.cancel(id);
                Err(e)
            }
        }
    }

    fn stop(&self) -> Result<(), TtsError> {
        let id = self.next_id();
        if !TTS_ACCEPT_MAILBOX.begin_at_if_idle(id) {
            return Err(TtsError::Internal("tts busy"));
        }
        if let Err(e) = self.call_tts_request(id, TTS_OP_STOP, "") {
            TTS_ACCEPT_MAILBOX.cancel(id);
            return Err(e);
        }
        TTS_ACCEPT_MAILBOX.wait(id, TTS_ACCEPT_TIMEOUT)
    }

    /// `speak` が返した ID に対応する真の最終結果を待つ（トレイト doc参照）。
    /// 追跡していない ID（未登録・空発話・既に消費済み）は [`Mailbox::wait_or_ok`]
    /// が即座に `Ok(())` を返す（`Tts::wait_outcome` の「対応する処理が既に無い
    /// 場合は古い待機として即座に `Ok(())`」契約）。時間による打ち切りは行わない
    /// （`Mailbox::wait_or_ok` doc 参照、codex-review P1・Bugbot Medium 是正）。
    fn wait_outcome(&self, generation: u64) -> Result<(), TtsError> {
        TTS_OUTCOME_MAILBOX.wait_or_ok(generation)
    }
}

/// Java 側から届いた読み上げ結果を解釈する（`nativeOnTtsResult` 本体）。
/// `status` の意味は `TtsHelper.request`（Java 側実装）と対で決める
/// （#866 実装計画 §5 の表、案A 是正で 7 を追加）。
fn decode_tts_result(status: i32) -> Result<(), TtsError> {
    match status {
        0 => Ok(()),
        // 1 = `TextToSpeech` を生成できない（engine なし・コンストラクタ例外）。
        // 機種として非対応。
        1 => Err(TtsError::Unsupported),
        // 3 = 非同期初期化（`onInit`）が ERROR。機種としては対応し得るが今回は失敗。
        3 => Err(TtsError::Internal("tts init")),
        // 4 = 初期化待ちの要求が既にある（Java 側の単一飛行防御。Rust 側の
        // mailbox が単一飛行を保証しているため通常到達しない）。
        4 => Err(TtsError::Internal("tts busy")),
        // 5 = `speak()` 自体が `ERROR` を返した（キュー投入失敗）、または投入後に
        // `UtteranceProgressListener#onError` が届いた（発話自体の失敗、PR #869
        // 是正: 旧実装はキュー投入成功を発話成功と混同していた）。
        5 => Err(TtsError::Internal("tts speak")),
        // 6 = `getMaxSpeechInputLength()` 超過。契約違反ではなく入力起因。
        6 => Err(TtsError::InvalidInput("tts text")),
        // 7 = 初期化待ち中に stop が届いたことによるキャンセル（案A 是正）、または
        // 発話開始後に `onStop`（`stop()`・後続 speak の QUEUE_FLUSH による中断）が
        // 届いたことによるキャンセル（PR #869 是正）。ユーザー操作による停止のため
        // 失敗として扱わない（`CommandTts::wait_outcome` doc と同じ判断）。
        7 => Ok(()),
        // 2 = その他の内部エラー（未知の op・実行時例外・Activity 破棄で中断）。
        // 未知の status も安全側で同様に扱う。
        _ => Err(TtsError::Internal("tts")),
    }
}

/// `TtsHelper.nativeOnTtsAccepted`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #866 案A）。Java が SPEAK 要求を受理した直後（実処理へ進む前）に
/// 呼ばれ、[`AndroidTts::speak`] の `TTS_ACCEPT_MAILBOX` 待ちを解放するだけの
/// 単純な仲介。生ポインタへのアクセスを行わないため `unsafe` 操作は含まない。
///
/// # Safety
/// `nativeOnTtsResult`（下記）と同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_TtsHelper_nativeOnTtsAccepted(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
) {
    on_tts_accepted(request_id as u64);
}

/// 受理シグナルの配送本体（JNI 受け口 `nativeOnTtsAccepted` から呼ぶ。ホストのテストでも通せるよう JNI 型を持たない）。
fn on_tts_accepted(id: u64) {
    TTS_ACCEPT_MAILBOX.deliver(id, Ok(()));
}

/// `TtsHelper.nativeOnTtsResult`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #866）。シンボル名の固定規約は `nativeOnCameraResult` と同じ。
///
/// STOP の最終結果、または SPEAK の真の最終結果のいずれかを運ぶ（モジュール doc
/// 「2 本の mailbox」参照）。まず [`TTS_ACCEPT_MAILBOX`] への配送を試み
/// （STOP はここで受理される）、一致しなければ [`TTS_OUTCOME_MAILBOX`] へ回す
/// （SPEAK が既に受理済みで、こちらが本来の宛先）。
///
/// [`TTS_ACCEPT_MAILBOX`] 側に届いた場合（＝ `nativeOnTtsAccepted` を経ない
/// 単相応答。SPEAK が `nativeOnTtsAccepted` を送らず早期 return する分岐
/// （最大文字数超過・`TTS_STATE_FAILED`・未知の op・破棄済み Activity、
/// `TtsHelper.request` 参照。`READY` を含む SPEAK の全経路は PR #869
/// 3 巡目是正で `nativeOnTtsAccepted` → `nativeOnTtsResult` の二相に統一済み
/// のため、現在ここへ届く SPEAK は上記の早期 return 分岐のみ）も、同じ結果を
/// [`TTS_OUTCOME_MAILBOX`] へ重ねて配送する（codex-review P1 再指摘）。
/// `AndroidTts::speak` は [`TTS_OUTCOME_MAILBOX`] を JNI 呼び出し前に必ず
/// 登録済み（`speak` 実装参照）のため、単相応答の場合ここで確定させないと
/// 登録だけが残り、`wait_outcome`（[`Mailbox::wait_or_ok`]、時間では終了しない
/// 契約、PR #869 3 巡目是正）が正常な発話を待ち続けたまま応答しなくなる
/// （嘘の失敗ではなく、確定漏れによる無応答）。STOP はそもそも
/// [`TTS_OUTCOME_MAILBOX`] を登録しないため、この重複配送は追跡されていない
/// ID への配送として無視される（[`Mailbox::deliver`] 参照）。
///
/// `JNIEnv` を使わず（受け取った値を計算して mailbox へ push するのみ）、生ポインタへの
/// アクセスを行わないため関数本体に `unsafe` 操作は含まない
/// （`nativeOnMotionChanged` と同じ形）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` により Rust の名前修飾を
/// 無効化するが、シンボル名は JNI 命名規約上の固定名であり、本クレートは cdylib
/// として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
/// （`nativeOnCameraResult` と同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_TtsHelper_nativeOnTtsResult(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    status: jni::sys::jint,
) {
    on_tts_result(request_id as u64, status);
}

/// 結果の配送本体（JNI 受け口 `nativeOnTtsResult` から呼ぶ。ホストのテストでも通せるよう JNI 型を持たない）。
/// 配送先の振り分けは `nativeOnTtsResult` の doc のとおり。
fn on_tts_result(id: u64, status: i32) {
    if TTS_ACCEPT_MAILBOX.try_deliver(id, decode_tts_result(status)) {
        // 単相応答（上記 doc 参照）。outcome 側が登録されたまま残らないよう
        // 同じ結果で確定させる（未登録の STOP 系 ID には無害に無視される）。
        TTS_OUTCOME_MAILBOX.deliver(id, decode_tts_result(status));
        return;
    }
    TTS_OUTCOME_MAILBOX.deliver(id, decode_tts_result(status));
}

#[cfg(test)]
mod tts_tests {
    use super::{Mailbox, decode_tts_result};
    use crate::error::TtsError;
    use std::sync::Mutex;

    /// JNI エクスポート関数（`nativeOnTtsResult`）を直接呼ぶテストは
    /// static `TTS_ACCEPT_MAILBOX`/`TTS_OUTCOME_MAILBOX` を共有するため、
    /// 並列実行で他テストの `begin_at`/`deliver` と干渉する（codex-review P2 指摘）。
    /// それ以外のテストは `Mailbox` をローカルに構築して使い、この直列化の
    /// 対象外にする（各テストが独立したインスタンスを持つため、テスト間の
    /// ID 採番が衝突しても問題ない）。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn decode_tts_result_maps_status_table() {
        assert!(decode_tts_result(0).is_ok());
        assert!(matches!(decode_tts_result(1), Err(TtsError::Unsupported)));
        assert!(matches!(
            decode_tts_result(2),
            Err(TtsError::Internal("tts"))
        ));
        assert!(matches!(
            decode_tts_result(3),
            Err(TtsError::Internal("tts init"))
        ));
        assert!(matches!(
            decode_tts_result(4),
            Err(TtsError::Internal("tts busy"))
        ));
        assert!(matches!(
            decode_tts_result(5),
            Err(TtsError::Internal("tts speak"))
        ));
        assert!(matches!(
            decode_tts_result(6),
            Err(TtsError::InvalidInput("tts text"))
        ));
        // 7 = stop による保留 speak のキャンセル。失敗として扱わない（案A 是正）。
        assert!(decode_tts_result(7).is_ok());
        // 未知の status も安全側（内部エラー）に畳む。
        assert!(matches!(
            decode_tts_result(99),
            Err(TtsError::Internal("tts"))
        ));
    }

    // 以下は JNI・実機なしで検証できる mailbox の世代識別ロジック（案A の核心）を
    // 固定するホスト実行テスト。static を直接触らないテストはローカルな `Mailbox`
    // インスタンスを構築するため、他テストと並列実行しても干渉しない
    // （codex-review P2 指摘の是正）。

    /// stop による保留 speak のキャンセル（status 7）が `wait_outcome` 相当の
    /// 待ち（`TTS_OUTCOME_MAILBOX.wait_or_ok`）を失敗として扱わないことを固定する。
    #[test]
    fn outcome_mailbox_cancel_status_resolves_as_ok() {
        let outcome: Mailbox<()> = Mailbox::new("test outcome timeout");
        let id = 1;
        outcome.begin_at(id);
        outcome.deliver(id, decode_tts_result(7));
        assert!(outcome.wait_or_ok(id).is_ok());
    }

    /// 未登録（追跡していない）ID は `wait_or_ok` がブロックせず即座に成功する
    /// （空発話が `wait_outcome` を呼んでも待たされないことの根拠、
    /// `Tts::wait_outcome` の「対応する処理が既に無い場合は即座に `Ok(())`」契約）。
    #[test]
    fn outcome_mailbox_wait_or_ok_is_immediate_for_untracked_id() {
        let outcome: Mailbox<()> = Mailbox::new("test outcome timeout");
        let untracked_id = 1;
        let started = std::time::Instant::now();
        assert!(outcome.wait_or_ok(untracked_id).is_ok());
        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "追跡していない ID でブロックした: {:?}",
            started.elapsed()
        );
    }

    /// 別 ID（世代）向けに登録された結果を誤って回収しないことを固定する
    /// （codex-review P1 是正の核心: `CommandTts::wait_outcome` の
    /// `wait_outcome_ignores_result_of_a_later_speak_with_different_generation`
    /// と同型の担保）。id_a の結果を待つ前に id_b で上書きされていても、
    /// id_a の待ちは「追跡していない」として即座に `Ok(())` で抜け、id_b 自身の
    /// 結果は id_b の待ちだけが正しく受け取る。
    #[test]
    fn outcome_mailbox_does_not_collect_result_of_a_different_generation() {
        let outcome: Mailbox<()> = Mailbox::new("test outcome timeout");
        let id_a = 1;
        let id_b = 2;
        outcome.begin_at(id_a);
        // id_a の結果が届く前に id_b へ差し替わる（`begin_at` は無条件上書き）。
        outcome.begin_at(id_b);
        outcome.deliver(id_b, Err(TtsError::Internal("tts speak")));

        // id_a はもう追跡されていないため、id_b の失敗を誤って回収しない。
        assert!(outcome.wait_or_ok(id_a).is_ok());
    }

    /// 対象世代自身の失敗は `wait_or_ok` が正しく `Err` として返すことを固定する。
    #[test]
    fn outcome_mailbox_reports_failure_of_its_own_generation() {
        let outcome: Mailbox<()> = Mailbox::new("test outcome timeout");
        let id = 1;
        outcome.begin_at(id);
        outcome.deliver(id, Err(TtsError::Internal("tts speak")));
        assert!(matches!(
            outcome.wait_or_ok(id),
            Err(TtsError::Internal("tts speak"))
        ));
    }

    /// `nativeOnTtsResult` のルーティング（`try_deliver` で accept 側へ先に届け、
    /// 一致しなければ outcome 側へ回す）を固定する: accept 側が `Pending` の
    /// ID（STOP 相当）は accept 側で受理され、outcome 側には届かない。
    #[test]
    fn accept_mailbox_receives_result_when_pending_and_outcome_is_untouched() {
        let accept: Mailbox<()> = Mailbox::new("test accept timeout");
        let outcome: Mailbox<()> = Mailbox::new("test outcome timeout");
        let id = 1;
        accept.begin_at_if_idle(id);

        let delivered_to_accept = accept.try_deliver(id, Ok(()));
        assert!(delivered_to_accept);

        // accept 側で受理済みのため、outcome 側は未登録のまま
        // （＝待ってもブロックせず即座に成功する）。
        assert!(outcome.wait_or_ok(id).is_ok());
        assert!(accept.wait(id, std::time::Duration::from_secs(1)).is_ok());
    }

    /// accept 側が既に消費済み（`Idle`）の ID への配送は outcome 側へ回ることを
    /// 固定する（SPEAK が受理された後に真の最終結果が届く経路、モジュール doc
    /// 「2 本の mailbox」参照）。
    #[test]
    fn result_routes_to_outcome_mailbox_once_accept_already_consumed() {
        let accept: Mailbox<()> = Mailbox::new("test accept timeout");
        let outcome: Mailbox<()> = Mailbox::new("test outcome timeout");
        let id = 1;
        accept.begin_at_if_idle(id);
        // 受理シグナルを先に消費する（`AndroidTts::speak` の accept 待ちに相当）。
        assert!(accept.try_deliver(id, Ok(())));
        assert!(accept.wait(id, std::time::Duration::from_secs(1)).is_ok());

        outcome.begin_at(id);
        let delivered_to_accept = accept.try_deliver(id, Ok(()));
        assert!(
            !delivered_to_accept,
            "accept 側は既に Idle のため一致しないはず"
        );
        outcome.deliver(id, Ok(()));
        assert!(outcome.wait_or_ok(id).is_ok());
    }

    /// codex-review P1 再指摘（本 PR）の再発防止: 単相応答（`nativeOnTtsAccepted` を
    /// 経ずに `nativeOnTtsResult` が直接 `TTS_ACCEPT_MAILBOX` へ届く、SPEAK の
    /// `READY` 経路の再読み上げ成功等）が届いたとき、`TTS_OUTCOME_MAILBOX` が
    /// `Pending` のまま取り残されないことを固定する。`AndroidTts::speak` は
    /// JNI 呼び出し前に必ず `TTS_OUTCOME_MAILBOX.begin_at` 済み（`speak` 実装参照）
    /// のため、ここでも同じ順序（accept 登録 → outcome 登録 → 結果到着）を再現する。
    ///
    /// 実際の JNI エクスポート関数（`Java_..._nativeOnTtsResult`）を直接呼ぶため
    /// static `TTS_ACCEPT_MAILBOX`/`TTS_OUTCOME_MAILBOX` を経由する。並列実行での
    /// 他テストとの干渉を防ぐため [`TEST_LOCK`] で直列化する（codex-review P2 指摘）。
    #[test]
    fn native_on_tts_result_single_shot_success_also_resolves_outcome_mailbox() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = 2_000_001;
        super::TTS_ACCEPT_MAILBOX.begin_at_if_idle(id);
        super::TTS_OUTCOME_MAILBOX.begin_at(id);

        super::on_tts_result(id, 0);

        // accept 側（本来の宛先）が受理する。
        assert!(
            super::TTS_ACCEPT_MAILBOX
                .wait(id, std::time::Duration::from_secs(1))
                .is_ok()
        );
        // outcome 側も同じ結果で確定済みのため、`wait_or_ok` はブロックせず
        // 即座に成功で戻る（`wait_or_ok` に時間による打ち切りは無いため、
        // ここで確定させ損なうと Pending のまま永久に応答しなくなる）。
        let started = std::time::Instant::now();
        assert!(super::TTS_OUTCOME_MAILBOX.wait_or_ok(id).is_ok());
        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "outcome 側が Pending のまま残っており待たされた: {:?}",
            started.elapsed()
        );
    }

    /// 同上・失敗版: 単相応答の失敗（例: `getMaxSpeechInputLength()` 超過、
    /// status 6）も outcome 側へ同じ `Err` が伝わることを固定する。static を
    /// 経由するため [`TEST_LOCK`] で直列化する（codex-review P2 指摘）。
    #[test]
    fn native_on_tts_result_single_shot_failure_also_resolves_outcome_mailbox() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = 2_000_002;
        super::TTS_ACCEPT_MAILBOX.begin_at_if_idle(id);
        super::TTS_OUTCOME_MAILBOX.begin_at(id);

        super::on_tts_result(id, 6);

        assert!(matches!(
            super::TTS_ACCEPT_MAILBOX.wait(id, std::time::Duration::from_secs(1)),
            Err(TtsError::InvalidInput("tts text"))
        ));
        assert!(matches!(
            super::TTS_OUTCOME_MAILBOX.wait_or_ok(id),
            Err(TtsError::InvalidInput("tts text"))
        ));
    }

    /// 対照実験: STOP のように `TTS_OUTCOME_MAILBOX` を登録しない単相応答では、
    /// 重複配送（本 PR の是正）が無害であることを固定する（未登録 ID への
    /// `deliver` は無視される、`Mailbox::deliver` 参照）。static を経由するため
    /// [`TEST_LOCK`] で直列化する（codex-review P2 指摘）。
    #[test]
    fn native_on_tts_result_stop_style_single_shot_does_not_touch_unregistered_outcome() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = 2_000_003;
        super::TTS_ACCEPT_MAILBOX.begin_at_if_idle(id);
        // STOP は `TTS_OUTCOME_MAILBOX` を登録しない（`AndroidTts::stop` 参照）。

        super::on_tts_result(id, 0);

        assert!(
            super::TTS_ACCEPT_MAILBOX
                .wait(id, std::time::Duration::from_secs(1))
                .is_ok()
        );
        // 追跡していないため即座に成功（＝重複配送で何も壊れていない）。
        assert!(super::TTS_OUTCOME_MAILBOX.wait_or_ok(id).is_ok());
    }

    /// codex-review P1 再指摘（本 PR）の再発防止: `wait_outcome(A)` が
    /// [`Mailbox::wait_or_ok`] の待機ループへ既に入った**後**に発話 B が来て
    /// [`Mailbox::begin_at`] がスロットを上書きしても、（`wait_or_ok` は時間で
    /// 打ち切らないため）`begin_at` の `notify_all` で即座に起こされて短時間で
    /// `Ok(())` を返すことを固定する（この `notify_all` が無いと、`wait_or_ok`
    /// は時間による打ち切りを持たないため B の後続結果が届くまで永久に眠り込む
    /// ことになり、`Tts::stop`/後続 `speak` による即時解決の契約が崩れる）。
    /// B 側（新しい世代）の結果は A の待機に一切影響されないことも合わせて見る。
    /// ローカル `Mailbox` を使うため static を経由せず、他テストと並列実行しても
    /// 干渉しない。
    #[test]
    fn wait_or_ok_wakes_immediately_when_begin_at_replaces_slot_while_waiting() {
        use std::sync::Arc;
        let outcome: Arc<Mailbox<()>> = Arc::new(Mailbox::new("test outcome timeout"));
        let id_a = 1;
        let id_b = 2;

        // `speak(A)` 相当: outcome 側を登録してから `wait_outcome(A)` を別スレッドで
        // 開始する（`driver.rs::run_speak` が背景スレッドへ切り出す形を模す）。
        outcome.begin_at(id_a);

        let waiter_a = {
            let outcome = Arc::clone(&outcome);
            std::thread::spawn(move || outcome.wait_or_ok(id_a))
        };

        // `waiter_a` が実際にロックを取ってループへ入る猶予を与える
        // （取る前に `begin_at(B)` が先行すると、そもそもループの初回チェックで
        // 「追跡していない」と判定され、この再発防止テストの意味が薄れるため）。
        std::thread::sleep(std::time::Duration::from_millis(50));

        // `speak(B)` 相当: A の待機が生きている間に B が outcome 側を上書きする。
        let started = std::time::Instant::now();
        outcome.begin_at(id_b);

        // `notify_all` により即座に起こされて「置き換え済み」を検知し `Ok(())` で
        // 戻る（`wait_or_ok` に時間による打ち切りが無いため、この通知が効いて
        // いなければ `waiter_a.join()` はテストプロセスごと永久にハングする）。
        let result_a = waiter_a.join().expect("panic しない");
        assert!(result_a.is_ok(), "{result_a:?}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "begin_at による置き換えから wait_or_ok が戻るまで長く掛かりすぎている              （notify_all が効いていない疑い）: {:?}",
            started.elapsed()
        );

        // B 自身の結果は A の待機に影響されず、正しく届く。
        outcome.deliver(id_b, Err(TtsError::Internal("tts speak")));
        assert!(matches!(
            outcome.wait_or_ok(id_b),
            Err(TtsError::Internal("tts speak"))
        ));
    }
}
