package com.fandhe.platform;

import android.app.Activity;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.speech.tts.TextToSpeech;
import android.speech.tts.UtteranceProgressListener;

import java.util.Map;
import java.util.WeakHashMap;

// 読み上げ（TTS、Issue #866）の Android 実装ヘルパー（Issue #1354 で `MainActivity` から機能 crate へ移した）。
// `TtsLifecycle` が Activity ごとに生成する（Rust の `fandhe-cap-tts/src/android.rs` が static メソッド
// `request(Activity, long, int, String)` を JNI で呼び、結果は `nativeOnTtsAccepted` / `nativeOnTtsResult` で受ける）。
//
// helper が無い（生成前・破棄後）要求は成功にも待ちにもせず、status 2（内部エラー）で確定する。Rust 側は
// 単相応答として受理と最終結果の両方を確定させるため、結果が来ないまま待たされない。
// `TextToSpeech` と保留要求・期限・破棄フラグは Activity ごとの helper のインスタンスが持つ（UI スレッドだけが読み書きする）。
final class TtsHelper {
    private final Activity activity;

    private TtsHelper(Activity activity) {
        this.activity = activity;
    }

    // Activity ごとの helper。`TtsLifecycle`（`FeatureLifecycleProvider`）が生成時に付け、破棄時に外す。
    private static final Map<Activity, TtsHelper> HELPERS = new WeakHashMap<>();

    static void attach(Activity activity) {
        TtsHelper helper = new TtsHelper(activity);
        synchronized (TtsHelper.class) {
            HELPERS.put(activity, helper);
        }
    }

    private static synchronized TtsHelper of(Activity activity) {
        return HELPERS.get(activity);
    }

    // `TextToSpeech` はプロセス内リソース（音声エンジンへの接続）を持つため、Activity 破棄時に明示的に手放す
    // （Issue #866）。保留中の要求（`INITIALIZING` 中に Activity が破棄されたケース）は `onInit` が届かなくなるため、
    // ここで内部エラーとして畳んでおく（Rust 側 mailbox がタイムアウトまで待たされずに済む）。
    // Activity 再生成後は次の要求で新しい helper の `ttsRequest` が `TextToSpeech` を作り直す。
    //
    // 従来は `MainActivity.onDestroy` が `super.onDestroy()` の前に破棄フラグを立てていた。provider の通知は
    // `super.onDestroy()` の中で届くため、前後だけが違う。破棄フラグを立てるのはこの `detach` の先頭で、
    // 以後に実行される `ttsRequest` の Runnable と `onTtsInit`（どれも同じ UI スレッドの直列キュー上）が
    // フラグを見て拒否する経路は変わらない。
    static void detach(Activity activity) {
        TtsHelper helper;
        synchronized (TtsHelper.class) {
            helper = HELPERS.remove(activity);
        }
        if (helper != null) {
            helper.destroy();
        }
    }

    // JNI から呼ばれる入口。即時 return・結果は `nativeOnTtsAccepted` / `nativeOnTtsResult`。
    // helper が無い（生成前・破棄後）ときは内部エラー（status 2）で確定する。
    static void request(Activity activity, long requestId, int op, String text) {
        TtsHelper helper = of(activity);
        if (helper == null) {
            nativeOnTtsResult(requestId, 2);
            return;
        }
        helper.ttsRequest(requestId, op, text);
    }

    private void destroy() {
        // 破棄状態をまず確定する（codex-review 指摘 P1）。以後に実行される `ttsRequest` の Runnable・`onTtsInit` は
        // このフラグを見て要求を拒否する（`ttsActivityDestroyed` doc 参照）。
        ttsActivityDestroyed = true;
        long pendingTtsRequestId = pendingTtsRequest.take();
        if (pendingTtsRequestId != RequestTracker.NONE) {
            nativeOnTtsResult(pendingTtsRequestId, 2);
            pendingTtsText = null;
        }
        if (tts != null) {
            // 発話中の utterance があれば `onStop` を促してから破棄する。
            // `shutdown()` 単体で `onStop` が確実に呼ばれる保証は `TextToSpeech`
            // のドキュメントに無いため、明示的に `stop()` してから `shutdown()` する。
            tts.stop();
            // `stop()` が `onStop` を発火させる保証はないため、`shutdown()` の前に
            // 直接確定させる（codex-review 指摘、PR #869 4 巡目是正）。
            // `take()` で読み取りとクリアを 1 操作にすることで、この直後に
            // 本物の `onStop` が届いても `deliverUtteranceOutcome` 側の
            // `complete` が不一致になり二重送信にならない
            // （`speakingRequest` doc 参照）。status 7 = キャンセル
            // （Activity 破棄による停止のため失敗として扱わない）。
            long stillSpeakingRequestId = speakingRequest.take();
            if (stillSpeakingRequestId != RequestTracker.NONE) {
                nativeOnTtsResult(stillSpeakingRequestId, TTS_STATUS_CANCELLED);
            }
            tts.shutdown();
            tts = null;
            ttsInitState = TTS_STATE_NONE;
        }
    }


    // Rust 側（crates/fandhe-cap-tts/src/android.rs）の JNI エクスポート関数
    // （Issue #866、Phase 11 型サンプル「読み上げ」の Android 経路に限り追加。
    // Phase 8 の feature `device` 切り離し時に削除する）。
    // requestId: `ttsRequest(long, int, String)` が受け取った値をそのまま返す
    // （Rust 側 mailbox が ID 一致を検査し、タイムアウト後の遅延到着を新要求の結果と
    // 取り違えないため。クリップボード経路と同じ理由）。
    // status: 0=成功（キュー投入ではなく `UtteranceProgressListener#onDone` による
    // 真の発話完了、`utteranceListener` 参照。codex-review P1 是正、PR #869）／
    // 1=`TextToSpeech` を生成できない／2=その他の内部エラー（stop() が ERROR を
    // 返した場合を含む）／3=非同期初期化（onInit）が ERROR／
    // 4=（未使用。旧: 初期化待ちの要求が既にある。INITIALIZING 中の要求は
    // 拒否せず最新の要求で保留を置き換えるよう改めたため、Rust 側は互換のため
    // 引き続き decode_tts_result で内部エラーへ写像する）／
    // 5=speak() 自体が ERROR を返した（キュー投入失敗）、または投入後に
    // `onError`（発話自体の失敗）が届いた（PR #869 是正: 旧実装はキュー投入成功を
    // 発話成功と混同していた）／6=最大文字数超過／
    // 7=stop または後続 speak による保留中 speak 要求のキャンセル、あるいは
    // 発話開始後に `onStop`（ユーザー操作による中断）が届いた（いずれも失敗として
    // 扱わない、`decode_tts_result` 参照）
    // （Rust 側 `decode_tts_result` と対で決める）。
    private static native void nativeOnTtsResult(long requestId, int status);

    // codex-review P1 是正（Issue #866 案A）: `ttsRequest` は SPEAK 要求が
    // `TextToSpeech` の初期化待ち（`TTS_STATE_NONE`/`TTS_STATE_INITIALIZING`）へ
    // 進む直前にこれを呼ぶ。Rust 側 `AndroidTts::speak` はこの通知だけを待って
    // 戻り、`onInit` 完了や `speak()` 呼び出し自体の結果は待たない
    // （`nativeOnTtsResult` を `wait_outcome` が別途待つ）。これにより
    // `speak`/`stop` が同じ FIFO（`fandhe-cap-tts/src/driver.rs` の `worker_loop`）上で
    // 直列処理されていても、初期化待ち中に届いた stop 要求が速やかに Java へ
    // 届くようになる（旧実装は `speak` 自体が `onInit` 完了までブロックしていた
    // ため、stop がキューに残ったまま初期化完了後に「キャンセル済みのテキストを
    // 発話してから停止」してしまっていた）。STOP・即座に確定する分岐
    // （破棄済み Activity・未知の op・最大文字数超過・`FAILED`・`READY`）には
    // 送らない（いずれも二段階に分ける必要が無い一発応答のため、Rust 側の
    // `TTS_ACCEPT_MAILBOX` がそのまま最終結果を受理する）。
    private static native void nativeOnTtsAccepted(long requestId);

    // 読み上げ要求の種別（Rust 側 `TTS_OP_SPEAK` / `TTS_OP_STOP` と対）。
    private static final int TTS_OP_SPEAK = 0;
    private static final int TTS_OP_STOP = 1;
    // `nativeOnTtsResult` の status 7（stop による保留 speak 要求のキャンセル）。
    // Rust 側はキャンセルを失敗として扱わない（`decode_tts_result` 参照）。
    private static final int TTS_STATUS_CANCELLED = 7;


    // 読み上げ（TTS、Issue #866）。`TextToSpeech` は最初の要求で遅延生成し（`onCreate`
    // では作らない。読み上げを 1 度も使わない画面ではエンジン起動コストを払わない）、
    // `OnInitListener` による非同期初期化の完了まで要求を 1 件だけ保留する
    // （Rust 側 mailbox が単一飛行を保証するため保留は常に高々 1 件で足りる。
    // ttsRequest doc 参照）。フィールドはすべて UI スレッド（`runOnUiThread`）
    // からのみ読み書きする。
    private TextToSpeech tts;

    // `speakNow` が `speak()` へ渡す utteranceId のプレフィックス。`utteranceListener`
    // はこのプレフィックスを剥がして `requestId`（long）へ戻すことで、どの発話に
    // 対する完了・失敗・中断通知かを識別する（codex-review P1 是正、PR #869）。
    private static final String UTTERANCE_ID_PREFIX = "fandhe-";

    // 現在 `tts.speak()` へ投入済み（＝完了・失敗・中断のいずれかを
    // `utteranceListener` が確定させるまで Rust 側 `wait_outcome` が待ち続けている）
    // requestId（無ければ `RequestTracker.NONE`）。`speakNow`（UI スレッド）が
    // 設定し、`deliverUtteranceOutcome`（`utteranceListener` のコールバックスレッド、
    // UI スレッドとは別）が確定時にクリアする。異なるスレッドから読み書きするため
    // `RequestTracker`（内部 `AtomicLong`）にする（codex-review 指摘、PR #869 4 巡目
    // 是正。id の保持・照合の集約は Issue #944）。
    //
    // `onDestroy`（UI スレッド）がこれを読み、発話中のまま Activity が破棄される
    // ケースで `onStop` を待たず `nativeOnTtsResult` を確定させるための唯一の用途。
    // `take` で読み取りとクリアを 1 操作にすることで、直後に本物の `onStop`/
    // `onDone` が届いても（`deliverUtteranceOutcome` 側の `complete` が
    // 不一致で no-op になり）二重送信にならない。
    private final RequestTracker speakingRequest = new RequestTracker();

    // `speakNow` が `speak()` に登録する完了通知（Issue #866、PR #869 是正）。
    // `TextToSpeech.speak()` の戻り値（`SUCCESS`）はキュー投入成功に過ぎず発話完了を
    // 意味しない（codex-review P1 指摘）ため、真の最終結果はこのリスナーの
    // `onDone`/`onError`/`onStop` で確定させ、`nativeOnTtsResult` を呼ぶ。
    //
    // コールバックは `TextToSpeech` 内部のワーカースレッドから呼ばれる契約
    // （`UtteranceProgressListener` のドキュメント）。ここで触るのは
    // `nativeOnTtsResult`（Rust 側の mailbox 実装が呼び出しスレッドを問わず
    // 安全、`fandhe-cap-tts/src/android.rs` の `Mailbox` 参照）と `utteranceId` の文字列解析
    // のみで、`pendingTtsRequest` 等の UI スレッド専用フィールドには一切触れない
    // ため、追加の同期は不要。
    private final UtteranceProgressListener utteranceListener =
            new UtteranceProgressListener() {
                @Override
                public void onStart(String utteranceId) {
                    // 発話開始の通知は使わない（受理は `nativeOnTtsAccepted` が別途担う）。
                }

                @Override
                public void onDone(String utteranceId) {
                    deliverUtteranceOutcome(utteranceId, 0);
                }

                @Override
                @Deprecated
                public void onError(String utteranceId) {
                    // API 21 相当の後方互換オーバーロード。API 23+ では下の
                    // `onError(String, int)` が呼ばれるため通常は到達しない。
                    deliverUtteranceOutcome(utteranceId, 5);
                }

                @Override
                public void onError(String utteranceId, int errorCode) {
                    // エラーコードの種別は問わず一律「speak 失敗」に畳む
                    // （Rust 側 `decode_tts_result` の既存分類に合わせる）。
                    deliverUtteranceOutcome(utteranceId, 5);
                }

                @Override
                public void onStop(String utteranceId, boolean interrupted) {
                    // 停止（`stop()`・後続 speak の QUEUE_FLUSH による中断）は
                    // 失敗として扱わない（`decode_tts_result` の status 7 と同じ判断）。
                    deliverUtteranceOutcome(utteranceId, TTS_STATUS_CANCELLED);
                }
            };

    // `UTTERANCE_ID_PREFIX` を剥がして `requestId` を復元し、`nativeOnTtsResult` を
    // 呼ぶ（`utteranceListener` の各コールバックから共通で使う）。プレフィックス
    // 不一致・数値変換失敗（契約違反を検出した経路。通常到達しない）は無視する。
    private void deliverUtteranceOutcome(String utteranceId, int status) {
        if (utteranceId == null || !utteranceId.startsWith(UTTERANCE_ID_PREFIX)) {
            return;
        }
        try {
            long requestId =
                    Long.parseLong(utteranceId.substring(UTTERANCE_ID_PREFIX.length()));
            // この requestId がまだ「発話中」として追跡されていれば追跡を外す
            // （一致しなければ no-op。`onDestroy` が `take` で先に確定・
            // クリア済みの場合や、QUEUE_FLUSH で追い越された旧 utterance の
            // 遅延コールバックの場合はここで不一致になる、`speakingRequest`
            // doc 参照）。
            speakingRequest.complete(requestId);
            nativeOnTtsResult(requestId, status);
        } catch (NumberFormatException e) {
            // 数値へ変換できない utteranceId（契約違反。通常到達しない）。
        }
    }

    // `OnInitListener` を必ず非同期で処理するための Handler（cursor Bugbot 指摘）。
    // `ttsRequest` は既に UI スレッドの `runOnUiThread` 内で走るため、
    // `Activity#runOnUiThread` をそのまま使うと「呼び出し元スレッドが UI スレッドなら
    // 即時同期実行する」契約により、エンジンがコンストラクタ内から `onInit` を
    // 同期的に呼ぶ実装（一部端末で確認）の場合、`tts = new TextToSpeech(...)` の
    // 代入が完了する前に `onTtsInit` が走り、`speakNow` が未代入の `tts`（null）へ
    // `speak()` を呼んで NPE になる。`Handler#post` はメインルーパーのキューへ
    // 積むだけで同一スレッドでも同期実行しないため、`onTtsInit` は必ず
    // `new TextToSpeech(...)` 呼び出し（代入含む）が完了した後に走る。
    private final Handler mainHandler = new Handler(Looper.getMainLooper());

    // Java 側だけが持つ安全弁（PR #869 3 巡目是正: Rust 側 `wait_outcome`
    // （`TTS_OUTCOME_MAILBOX.wait_or_ok`）は完了・失敗・キャンセル・世代置換でのみ
    // 終了し、時間による打ち切りを持たない。`nativeOnTtsAccepted` は初期化待ちへ
    // 進む直前に既に送出済みのため、Rust 側は本値に対応するタイムアウトを持たない
    // 独立の値）。`TextToSpeech` の初期化（`onInit`）が極端に遅延・欠落した場合に
    // 保留中の SPEAK 要求を無期限に持ち続けないための Java ローカルな期限
    // （`pendingTtsDeadlineUptimeMs`/`expireStaleTtsRequest` 参照）。期限到来時は
    // `nativeOnTtsResult(requestId, 3)` を送って `wait_outcome` を確定させる
    // ため、次の speak/stop を待たずに解決する（PR #869 4 巡目是正、実測根拠の
    // ない安全弁である点は変わらず。実機で誤発火の報告が出たら見直す）。
    private static final long TTS_INIT_TIMEOUT_MS = 15_000L;
    // Rust 側 `fandhe-cap-tts/src/android.rs` の `TTS_ACCEPT_TIMEOUT`（5 秒）と揃える
    // （codex-review P1 是正・案A）。SPEAK は経路によらず `nativeOnTtsAccepted` →
    // `nativeOnTtsResult` の二相に統一されている（PR #869 3 巡目是正: 以前は
    // `READY` 経路が受理を送らない一発応答だったため、完了コールバック化後は
    // 必ずここでタイムアウトしていた不具合があった）ため、Rust 側 `speak` は
    // 常に `TTS_ACCEPT_MAILBOX` の受理待ち（`TTS_ACCEPT_TIMEOUT`）だけを見る。
    // `nativeOnTtsAccepted` を送らず一発で `nativeOnTtsResult` を返す分岐
    // （最大文字数超過・`TTS_STATE_FAILED`・未知の op・破棄済み Activity）でも、
    // `runOnUiThread` の実行遅延がこの期限を超えていれば呼び出し元は既に
    // タイムアウト失敗を返した後（`TTS_INIT_TIMEOUT_MS` ではなくこちらを使う）。
    private static final long TTS_ACCEPT_TIMEOUT_MS = 5_000L;
    private static final int TTS_STATE_NONE = 0;
    private static final int TTS_STATE_INITIALIZING = 1;
    private static final int TTS_STATE_READY = 2;
    private static final int TTS_STATE_FAILED = 3;
    private int ttsInitState = TTS_STATE_NONE;
    // 初期化待ち中に保留している SPEAK 要求の id（`RequestTracker.NONE` なら保留なし）。
    // 旧 `ttsRequestPending` boolean は `current() != RequestTracker.NONE` で表現する
    // （id の保持・照合の集約は Issue #944）。
    private final RequestTracker pendingTtsRequest = new RequestTracker();
    private String pendingTtsText;
    // 保留要求の実期限（`SystemClock.uptimeMillis()` 基準、codex-review 指摘 P1）。
    // `ttsRequest` の先頭（JNI 呼び出し受信時点。Rust 側 `TTS_MAILBOX.begin()` の
    // 起点とほぼ同時刻）で確定させ、`runOnUiThread` のキュー処理が UI スレッド
    // 混雑で遅延しても期限の起点がずれないようにする。`expireStaleTtsRequest` の
    // 予約（`postAtTime`）と `onTtsInit` の発話直前検査の両方がこの値を参照する。
    private long pendingTtsDeadlineUptimeMs;

    // `onDestroy` 完了後に届く TTS 要求を拒否するための破棄フラグ（codex-review
    // 指摘 P1）。`ttsRequest` は `runOnUiThread` で Runnable を UI スレッドキューへ
    // 積むため、その Runnable の実行前に `onDestroy` が完了すると、`onDestroy` は
    // まだキューに積まれただけで未実行の要求を検査できない。その後 Runnable が
    // 走ると `ttsInitState` は `onDestroy` が `NONE` へ戻した後のため、破棄済み
    // Activity 上で `TextToSpeech` を再生成して発話してしまい、このインスタンスには
    // 以後 `onDestroy` が来ないため接続も解放されない。フラグは `onDestroy` の
    // 冒頭（`TtsLifecycle.onHostDestroyed` → `detach`）で真にし、`ttsRequest` の Runnable と
    // `onTtsInit`（いずれも同じ UI スレッドの直列キュー上で実行されるため
    // volatile 不要）の先頭で検査して、破棄後の要求はエラーで完了させる。
    private boolean ttsActivityDestroyed;


    // Rust 側の TTS ワーカースレッド（`fandhe-cap-tts/src/android.rs` の `AndroidTts`）から
    // JNI 経由で呼ばれる（Issue #866）。`TextToSpeech` の操作は UI スレッドへ
    // 委譲する（クリップボードと同じ理由に加え、`OnInitListener` のコールバックが
    // UI スレッドで届く契約に揃えるため）。
    //
    // 初期化は非同期（`onInit` が後から呼ばれる）ため、初期化未完了時に本メソッドが
    // 偽の成功を返す経路を作らない: `NONE` なら生成して要求を保留し、`onInit` の
    // 結果が出た時点で初めて `nativeOnTtsResult` を呼ぶ（`onTtsInit` 参照）。
    // `INITIALIZING` 中に 2 件目の要求が来ることもある（Rust 側 `TTS_ACCEPT_MAILBOX` は
    // 受理だけで即座に空くため、初期化待ち中でも次の speak/stop が Rust 側から
    // 続けて届き得る。詳細は本体内コメント）。その場合は拒否せず、保留中の要求が
    // あればキャンセル（status 7）として畳んでから最新の要求で保留を置き換える。
    //
    // codex-review P1 是正（案A）: SPEAK が初期化待ち（`NONE`/`INITIALIZING`）へ
    // 進む直前に [`nativeOnTtsAccepted`] を呼ぶ。Rust 側 `AndroidTts::speak` は
    // この通知だけで戻るため、`speak`/`stop` を同じ FIFO で処理する Rust 側
    // ワーカーが `onInit` 完了を待たされず、直後の stop 要求を速やかに本メソッドへ
    // 届けられる（旧実装は `speak` 自体が `onInit` 完了までブロックしていた）。
    //
    // 例外メッセージは読み上げ内容を含み得るためログ出力しない（security.md A09）。
    private void ttsRequest(long requestId, int op, String text) {
        // JNI 呼び出しを受けた時点（Rust 側 `TTS_ACCEPT_MAILBOX.begin_at_if_idle()`
        // 直後）で期限の起点を確定させる（codex-review 指摘 P1）。`runOnUiThread`
        // の実行が UI スレッド混雑で遅延すると、期限の起点をそこに置いた場合
        // Rust 側の受理待ち（`TTS_ACCEPT_TIMEOUT`、5 秒、JNI 呼び出し後起点）より
        // Java 側の期限が後ろへずれ、Rust が既に受理タイムアウト失敗を返した
        // 要求を Java が後から `nativeOnTtsAccepted` してしまう（SPEAK が受理後の
        // 完了待ちに入ってしまうと、`wait_outcome` は時間で打ち切らないため
        // Rust 側は既にタイムアウト済みの要求として二度と拾わない）。
        long requestArrivalUptimeMs = SystemClock.uptimeMillis();
        activity.runOnUiThread(
                () -> {
                    try {
                        // `runOnUiThread` で積んだ本 Runnable の実行前に `onDestroy` が
                        // 完了しているケース（codex-review 指摘 P1）。`tts` を触らず
                        // 内部エラーで畳む（破棄済み Activity 上で `TextToSpeech` を
                        // 再生成しない）。
                        if (ttsActivityDestroyed) {
                            nativeOnTtsResult(requestId, 2);
                            return;
                        }
                        if (op == TTS_OP_STOP) {
                            long canceledRequestId = pendingTtsRequest.take();
                            if (canceledRequestId != RequestTracker.NONE) {
                                // 初期化待ち中の保留 speak 要求をキャンセルとして畳む
                                // （P1 是正: stop がここへ届くようになったことで初めて
                                // 到達できる分岐。`onTtsInit` は後で必ず呼ばれるが、
                                // ここで先に `take()` しておくことで `hadPending` が
                                // false になり二重に結果を送らない）。
                                pendingTtsText = null;
                                nativeOnTtsResult(canceledRequestId, TTS_STATUS_CANCELLED);
                            }
                            if (tts != null && ttsInitState == TTS_STATE_READY) {
                                // `stop()` の失敗（ERROR）を握りつぶさない（codex-review
                                // 指摘）。戻り値を検査し、非 SUCCESS を内部エラーへ
                                // 変換して Rust 側へ伝える（status 2 = その他の内部
                                // エラー。既存の分類に「停止失敗」を新設するほどの
                                // 呼び分けは Rust 側に無いため、既存の catch-all を使う）。
                                int stopResult = tts.stop();
                                nativeOnTtsResult(requestId, stopResult == TextToSpeech.SUCCESS ? 0 : 2);
                                return;
                            }
                            // `READY` 未満（NONE/INITIALIZING/FAILED）はまだ一度も
                            // `speak()` を呼んでいないため OS 側で止めるものが無い。
                            // INITIALIZING 中に保留していた発話があれば、ここで破棄する
                            // （codex-review 指摘 P1: 停止済みの要求が遅延 onInit 後に
                            // 発話されてしまう問題）。`pendingTtsRequest` を `take()` で
                            // 落とすことで `onTtsInit` の `hadPending` 判定が false になり、
                            // 後から初期化が成功しても発話されない。
                            pendingTtsRequest.take();
                            pendingTtsText = null;
                            nativeOnTtsResult(requestId, 0);
                            return;
                        }
                        if (op != TTS_OP_SPEAK) {
                            // 未知の op（Rust 側との契約違反）。内容を推測せず内部エラーにする。
                            nativeOnTtsResult(requestId, 2);
                            return;
                        }

                        // ここから先は SPEAK。SPEAK は受理 → 結果の二相を経路によらず
                        // 統一する（codex-review P1 再指摘・Bugbot High 是正、PR #869
                        // 3 巡目: 「受理通知は初期化待ちへ進む場合のみ」という前回の
                        // 限定が誤りだった。`READY`（発話まで即座に進む経路）でも
                        // `nativeOnTtsAccepted` を送らずに `speakNow` していたため、
                        // 完了コールバック化（本 PR 1 巡目）後は `speak()` の投入成功が
                        // 何も返さず `onDone` まで待つようになった結果、Rust 側
                        // `TTS_ACCEPT_MAILBOX` の受理待ちが `TTS_ACCEPT_TIMEOUT`
                        // （5 秒）で必ずタイムアウトし、5 秒を超えて続く正常な発話が
                        // 失敗表示になっていた）。単相応答（`nativeOnTtsAccepted` を
                        // 送らず一発で `nativeOnTtsResult` を返す）は下記の早期 return
                        // する失敗系（最大文字数超過・`FAILED`）のみに限定する。
                        if (text.length() > TextToSpeech.getMaxSpeechInputLength()) {
                            // `getMaxSpeechInputLength()` は static メソッド（`tts`
                            // インスタンスの有無に関わらず呼べる）のため、初回要求
                            // （`tts` 未生成）でも検査できる。
                            nativeOnTtsResult(requestId, 6);
                            return;
                        }
                        if (ttsInitState == TTS_STATE_FAILED) {
                            nativeOnTtsResult(requestId, 1);
                            return;
                        }
                        if (ttsInitState == TTS_STATE_READY) {
                            // `READY` でも `runOnUiThread` 自体がキュー混雑で遅延し得るため、
                            // ここでも `requestArrivalUptimeMs` 起点の期限を検査する
                            // （codex-review 指摘 P1: READY 経路が期限未検査だった）。
                            // 期限の基準は受理まで（`TTS_ACCEPT_TIMEOUT`、5 秒、
                            // `TTS_ACCEPT_TIMEOUT_MS`）であり、`nativeOnTtsAccepted` を
                            // 送った後の発話完了待ちには掛からない（Rust 側
                            // `TTS_OUTCOME_MAILBOX.wait_or_ok` は完了・失敗・キャンセル・
                            // 世代置換でのみ終了し、時間では終了しない、PR #869 3 巡目
                            // 是正）。期限超過なら Rust 側 `TTS_ACCEPT_MAILBOX` は既に
                            // タイムアウトで諦めているため `nativeOnTtsAccepted`/
                            // `nativeOnTtsResult` のいずれも送らず破棄する
                            // （`expireStaleTtsRequest`/`onTtsInit` と同じ扱い）。
                            if (SystemClock.uptimeMillis()
                                    >= requestArrivalUptimeMs + TTS_ACCEPT_TIMEOUT_MS) {
                                return;
                            }
                            // 受理を通知してから発話する（codex-review P1 再指摘・
                            // Bugbot High 是正: 旧実装はここで受理を送らず一発応答扱い
                            // にしていたため、完了コールバック化後は `TTS_ACCEPT_MAILBOX`
                            // の受理待ちが必ずタイムアウトしていた）。`speak()` が
                            // `ERROR` を返した場合の `nativeOnTtsResult(5)`
                            // （`speakNow` 参照）は受理後の失敗として `TTS_OUTCOME_MAILBOX`
                            // 側へ届く（`nativeOnTtsResult` の accept→outcome
                            // ルーティング、`Java_..._nativeOnTtsResult` doc 参照）。
                            nativeOnTtsAccepted(requestId);
                            speakNow(requestId, text);
                            return;
                        }
                        // ここに来るのは NONE か INITIALIZING。案A 是正（本 PR）により
                        // `nativeOnTtsAccepted` 送出後の Rust 側 `AndroidTts::speak` は
                        // 受理のみで戻るため、`TTS_ACCEPT_MAILBOX` は初期化待ち中でも
                        // 即座に空く。つまり INITIALIZING 中に複数回ここへ到達し得る
                        // （cursor Bugbot 指摘: 従来は status 4 で拒否し続け、新規発話が
                        // 回復不能になっていた）。旧保留要求（あれば）は Rust 側
                        // `TTS_OUTCOME_MAILBOX` がまだ待っている可能性があるため、
                        // 黙って捨てず `TTS_STATUS_CANCELLED` として確定させてから
                        // 最新の要求で保留を置き換える（旧: 単一 mailbox 前提で
                        // 「もう待っていない」と仮定して応答を省略していたが、案Aでは
                        // 受理と結果待ちが分離されているためこの仮定が成り立たない）。
                        //
                        // ただしこの要求 (requestId) 自体が `runOnUiThread` の遅延で
                        // 既に Rust 側の受理待ち（`TTS_ACCEPT_TIMEOUT`、5 秒）を
                        // 超えて到着していることがある（codex-review P1 再指摘。
                        // `READY` 経路の同種チェックと同じ判断、`TTS_ACCEPT_TIMEOUT_MS`
                        // 参照）。この場合 Rust 側は既に `nativeOnTtsAccepted` を
                        // 待たずタイムアウト失敗を返し `TTS_OUTCOME_MAILBOX` の登録も
                        // 自ら畳んでいるため、ここで受理を送っても誰も待っていない
                        // （無害だが無意味）。既存の保留要求を巻き込んで置き換える前に
                        // 検査し、期限切れなら requestId 自身を何もせず破棄する
                        // （既存の保留要求には触れない）。
                        if (SystemClock.uptimeMillis()
                                >= requestArrivalUptimeMs + TTS_ACCEPT_TIMEOUT_MS) {
                            return;
                        }
                        long canceledRequestId = pendingTtsRequest.take();
                        if (canceledRequestId != RequestTracker.NONE) {
                            nativeOnTtsResult(canceledRequestId, TTS_STATUS_CANCELLED);
                        }
                        pendingTtsRequest.track(requestId);
                        pendingTtsText = text;
                        // 新しい保留要求の受理を通知する（案A、`nativeOnTtsAccepted` doc
                        // 参照）。`onInit` 完了・`TextToSpeech` 構築の成否はまだ関係ない。
                        nativeOnTtsAccepted(requestId);
                        // `TTS_INIT_TIMEOUT_MS`（Java 側だけが持つ安全弁、上記 doc 参照）
                        // を仕込む（codex-review 指摘 P1）。期限は `requestArrivalUptimeMs`
                        // （JNI 呼び出し受信時点）起点で確定済み（`ttsRequest` 冒頭・
                        // `pendingTtsDeadlineUptimeMs` doc 参照）。`postDelayed` では
                        // なく `postAtTime` で絶対時刻指定にすることで、この
                        // `runOnUiThread` 自体の実行遅延を期限へ持ち込まない。
                        // `onInit` がこの期限を超えて遅延した場合、既に受理済み
                        // （`nativeOnTtsAccepted` 送出済み）のため Rust 側
                        // `wait_outcome` は時間で諦めず待ち続けている。Java 側の
                        // 保留（`pendingTtsRequest`/`pendingTtsText`）を期限到来時点で
                        // 破棄し（`expireStaleTtsRequest` 参照）、`nativeOnTtsResult`
                        // で status 3（init 失敗相当）を送って `wait_outcome` を
                        // 確定させる（codex-review 指摘、PR #869 4 巡目是正: 送らないと
                        // 世代置換まで無期限に残る）。期限到来時点で `requestId` が
                        // 変わっていなければ（＝新しい要求で上書きされていなければ）
                        // 保留を破棄し、遅れて届く `onInit(SUCCESS)` が `speakNow` を
                        // 呼ばないようにする。
                        pendingTtsDeadlineUptimeMs = requestArrivalUptimeMs + TTS_INIT_TIMEOUT_MS;
                        mainHandler.postAtTime(
                                () -> expireStaleTtsRequest(requestId),
                                pendingTtsDeadlineUptimeMs);
                        if (ttsInitState == TTS_STATE_NONE) {
                            ttsInitState = TTS_STATE_INITIALIZING;
                            try {
                                tts = new TextToSpeech(
                                        activity.getApplicationContext(),
                                        status -> mainHandler.post(() -> onTtsInit(status)));
                                // `onInit` 完了前に設定してよい（`speak()` を呼ぶまでは
                                // コールバックが発生しないため）。`speakNow` が使う
                                // utteranceId 経由の完了通知はこのリスナーが担う
                                // （`utteranceListener` doc 参照、PR #869 是正）。
                                tts.setOnUtteranceProgressListener(utteranceListener);
                            } catch (RuntimeException e) {
                                // コンストラクタ自体が例外を投げた（engine 未搭載端末等）。
                                ttsInitState = TTS_STATE_FAILED;
                                pendingTtsRequest.take();
                                pendingTtsText = null;
                                nativeOnTtsResult(requestId, 1);
                            }
                        }
                        // `INITIALIZING` 中はここで戻り、`onTtsInit`（または上記の
                        // stop によるキャンセル分岐）が保留要求を処理する。
                    } catch (RuntimeException e) {
                        // 受理を送出済みでも、最終結果は必ず送って outcome 側の
                        // 待ちを解放する（`nativeOnTtsAccepted` 後の想定外例外）。
                        pendingTtsRequest.take();
                        pendingTtsText = null;
                        nativeOnTtsResult(requestId, 2);
                    }
                });
    }

    // `ttsRequest` が `mainHandler.postDelayed` で `TTS_INIT_TIMEOUT_MS` 後に予約する
    // 保留要求の期限切れ処理（codex-review 指摘 P1）。`requestId` が積んだ時点から
    // 変わっていない（＝新しい要求で上書きされていない）場合のみ保留を破棄する。
    // `onTtsInit` が先に届いていれば `pendingTtsRequest` は既に `NONE` のため
    // `complete` は false になり無害。
    private void expireStaleTtsRequest(long requestId) {
        if (pendingTtsRequest.complete(requestId)) {
            pendingTtsText = null;
            // 受理済み（`nativeOnTtsAccepted` 送出済み）のため Rust 側
            // `wait_outcome`（`TTS_OUTCOME_MAILBOX.wait_or_ok`）は時間で
            // 打ち切らない。ここで送らないと世代置換（次の speak/stop）まで
            // 無期限に残ってしまうため、init 失敗相当（status 3）で確定させる
            // （codex-review 指摘、PR #869 4 巡目是正）。`onTtsInit` 側が
            // 同じ requestId を先に処理済みなら `pendingTtsRequest` は既に
            // `NONE` のためここへは来ない（二重送信にならない）。
            nativeOnTtsResult(requestId, 3);
        }
    }

    // `TextToSpeech` の非同期初期化コールバック（UI スレッドで実行、`ttsRequest` 参照）。
    // 保留していた要求（あれば）をここで初めて確定させる。
    private void onTtsInit(int status) {
        // `onDestroy` 完了後に届く初期化コールバック（codex-review 指摘 P1）。
        // 保留要求はあれば `onDestroy` が既にエラーで畳んで `tts` を shutdown/null
        // 済みのため、ここで `ttsInitState`/`tts` へ触れず素通しする。
        if (ttsActivityDestroyed) {
            return;
        }
        long requestId = pendingTtsRequest.take();
        boolean hadPending = requestId != RequestTracker.NONE;
        String text = pendingTtsText;
        pendingTtsText = null;

        // `postAtTime` で予約した `expireStaleTtsRequest` がまだキューで順番待ちの
        // 間に `onInit` が先に届くことがある（同じ UI スレッドキュー上でも、期限
        // ちょうどの前後では投入順・処理順が入れ替わり得る）。`hadPending`
        // だけに頼らず、発話直前に実期限を検査する（codex-review 指摘 P1:
        // 「初期化完了時・READY 経路の発話直前に検査する」）。期限超過の場合は
        // `expireStaleTtsRequest` と同じ扱い（status 3 で `wait_outcome` を
        // 確定させる、PR #869 4 巡目是正）にする。`pendingTtsRequest` は既に
        // 上で `take()` 済み（`NONE`）のため、後から実行される
        // `expireStaleTtsRequest`（未実行ならキューに残っている）は
        // `complete` が不一致になり二重送信にならない。
        if (hadPending && SystemClock.uptimeMillis() >= pendingTtsDeadlineUptimeMs) {
            hadPending = false;
            nativeOnTtsResult(requestId, 3);
        }

        if (status == TextToSpeech.SUCCESS) {
            ttsInitState = TTS_STATE_READY;
            if (hadPending) {
                speakNow(requestId, text);
            }
        } else {
            ttsInitState = TTS_STATE_FAILED;
            if (hadPending) {
                nativeOnTtsResult(requestId, 3);
            }
        }
    }

    // `tts` が `READY` になった後の実発話（`ttsRequest`/`onTtsInit` の共通経路）。
    // `QUEUE_FLUSH` で直前の発話を中断してから始める（`Tts::speak` の契約
    // 「直前の発話が続いていれば中断してから始める」に対応）。中断された前の発話は
    // `utteranceListener#onStop` が別途キャンセルとして確定させる。
    //
    // `speak()` の戻り値（`SUCCESS`）はキュー投入成功に過ぎず発話完了ではない
    // （codex-review P1 指摘、PR #869）。ここでは投入失敗（`ERROR`）のときのみ
    // 即座に `nativeOnTtsResult` を送る（コールバックが二度と来ないため）。
    // 投入成功時は何も送らず、真の完了・失敗・中断は `utteranceListener` が
    // utteranceId（`UTTERANCE_ID_PREFIX + requestId`）経由で後から確定させる。
    private void speakNow(long requestId, String text) {
        // `tts.speak()` 呼び出し前に「発話中」として記録する（`onDone` 等が
        // 呼び出し中・直後に別スレッドで届いても取りこぼさないよう、呼び出し後
        // ではなく前に track する。`speakingRequest` doc 参照）。
        speakingRequest.track(requestId);
        int result = tts.speak(
                text, TextToSpeech.QUEUE_FLUSH, null, UTTERANCE_ID_PREFIX + requestId);
        if (result != TextToSpeech.SUCCESS) {
            // 投入自体が失敗しコールバックは二度と来ないため、「発話中」の
            // 記録を戻す。
            speakingRequest.complete(requestId);
            nativeOnTtsResult(requestId, 5);
        }
    }
}
