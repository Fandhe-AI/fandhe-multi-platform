package com.fandhe.platform;

import android.Manifest;
import android.app.Activity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.speech.RecognitionListener;
import android.speech.RecognizerIntent;
import android.speech.SpeechRecognizer;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;

import java.util.ArrayList;
import java.util.Map;
import java.util.HashMap;

// 音声認識（Issue #1026）の Android 実装ヘルパー（Issue #1354 で `MainActivity` から機能 crate へ移した）。
// `SpeechRecognitionLifecycle` が Activity ごとに生成する（Rust の `fandhe-cap-speech-recognition/src/android.rs` が static メソッド
// `requestStart(Activity, long, int)` / `requestStop(Activity, long)` を JNI で呼び、結果を `nativeOnSpeechStarted` /
// `nativeOnSpeechResult` / `nativeOnSpeechError` で受ける）。
//
// helper が無い（生成前・破棄後）要求は、成功にも待ちにもせず内部エラー（code 2）で確定する（結果が来ないまま待たせない）。
// 許可ランチャーは `attach`（`onHostCreated`、STARTED より前）で登録する。キーは従来の `MainActivity` の文字列のまま。
// 前面かどうか（`resumed`）は `SpeechRecognitionLifecycle` の `onHostResumed` / `onHostPaused` が写す。どちらも `super.onResume()` /
// `super.onPause()` の中で届き、従来の `MainActivity` が `super` の後で更新していたのとは前後だけが違う。許可ダイアログの応答との
// 到着順（契約表 1）は、応答側が `resumed` を見て保留（`speechStartPendingResume`）にし、次の `onHostResumed` で再開する形で保つ。
//
// 再生成（回転等の構成変更）: 認識器（`speechRecognizer`）・リスナー・タイマー・要求 id・ロックは static で、新しい helper へそのまま
// 引き継がれる。認識器は旧 Activity でなくアプリケーション Context で作り（Activity で作ると破棄時にサービス接続が漏れとして
// 外される）、リスナーは static メソッドで作るため旧 helper / Activity を参照しない。結果は static な要求 id の照合で配送される。
// 構成変更では聞き取りを中断しない。背景遷移（構成変更でない停止）では `onHostStopped` が中断（5）する（背景で聞き取らない、契約表 1）。
// 中断を `onPause` でなく `onStop` で行うのは、`isChangingConfigurations()` が `onStop` / `onDestroy` で確実に読めるため
// （`Activity.isChangingConfigurations` の文書は `onStop` での利用を想定している。`onPause` 時点の値は文書で保証されていない）。
// 権限ダイアログは透過 Activity で `onPause` だけを起こし `onStop` は起こさないため、許可待ちを誤って中断しない。
// `RecognitionService` の検出（Android 11 以降のパッケージ可視性）は宣言の `android.queries` が担う。
final class SpeechRecognitionHelper {
    private final Activity activity;
    // UI スレッドだけが読み書きする前面の写し（従来の `MainActivity.activityResumed`）。`attach` の時点では前面ではない。
    private boolean resumed;
    // 音声認識の実行時許可要求（マイク録音の許可と同型）。許可が下りたら `startSpeechRecognizer` を呼ぶ。
    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、`attach`（`onHostCreated`、onCreate 内）で生成する。
    private final ActivityResultLauncher<String> requestSpeechPermission;

    // Activity ごとの helper。`SpeechRecognitionLifecycle`（`FeatureLifecycleProvider`）が生成時に付け、破棄時に外す。
    private static final Map<Activity, SpeechRecognitionHelper> HELPERS = new HashMap<>();

    static void attach(ComponentActivity activity) {
        // 再生成: 要求 id・権限ダイアログの照合状態・上限・保留フラグ（いずれも static）は捨てない。権限ダイアログの結果は
        // ActivityResultRegistry が同じキーの新しい許可ランチャーへ配送するため、新 helper の callback が引き継いで聞き取りを始める
        // （結果は同じ id で 1 度だけ届く）。聞き取り中だった場合も認識器は static なので、そのまま引き継ぐ（回転で止めない）。
        SpeechRecognitionHelper helper = new SpeechRecognitionHelper(activity);
        synchronized (SpeechRecognitionHelper.class) {
            HELPERS.put(activity, helper);
        }
        // 要求の発行元が構成変更・システム破棄で消えて引き継ぎ待ちなら、この helper が新しい発行元になる。
        synchronized (speechLock) {
            if (speechOwnerOrphaned) {
                speechOwner = helper;
                speechOwnerOrphaned = false;
            }
        }
    }

    // 追跡中の要求（権限ダイアログ待ち・聞き取り中）を発行した helper（`speechLock` の内側でのみ読み書きする）。背景遷移の中断・
    // 終了時の中断は、この発行元の Activity の `stopped` / `detach` だけが行う（別の Activity が止まっても触らない）。
    // 構成変更・システム破棄で発行元が破棄されたときは `speechOwnerOrphaned` を立て、次に `attach` された helper が引き継ぐ。
    private static SpeechRecognitionHelper speechOwner;
    private static boolean speechOwnerOrphaned;

    // Activity 破棄時。この Activity が要求の発行元（`speechOwner`）でなければ何もしない（別 Activity の要求を巻き込まない）。
    // 発行元の破棄では、構成変更なら聞き取りも保留中の要求も新しい helper へ引き継ぐ（`speechOwnerOrphaned`）。構成変更でない破棄では
    // 聞き取り中なら中断（5）で確定する（`onHostStopped` を経由しない稀な経路への備え）。終了（`isFinishing()`）では registry ごと消え、
    // 権限ダイアログの結果は二度と届かないため、保留中の要求を中断（5）として 1 度だけ確定し、権限ダイアログの照合状態を下ろす。
    // それ以外（システム破棄）は復元後の helper が結果を受けるので引き継ぐ。`take` が first-wins なので、許可コールバックとの
    // 二重通知にならない。
    static void detach(Activity activity) {
        SpeechRecognitionHelper helper;
        synchronized (SpeechRecognitionHelper.class) {
            helper = HELPERS.remove(activity);
        }
        if (helper == null) {
            return;
        }
        synchronized (speechLock) {
            if (speechOwner != helper) {
                return;
            }
            speechOwner = null;
            if (activity.isChangingConfigurations()) {
                speechOwnerOrphaned = true;
                return;
            }
            interruptListeningLocked();
            if (activity.isFinishing()) {
                speechOwnerOrphaned = false;
                speechStartPendingResume = false;
                speechPermissionPending = false;
                speechPermissionRequestId = RequestTracker.NONE;
                long id = speechRequest.take();
                if (id != RequestTracker.NONE) {
                    nativeOnSpeechResult(id, SPEECH_RESULT_INTERRUPTED, null);
                }
            } else {
                speechOwnerOrphaned = true;
            }
        }
    }

    // Rust 側 `reset_pending_tracking()`（`android_main`）から JNI で呼ばれる。Rust は保留中の要求を既に捨てて待っていないため、
    // Java 側も同じ要求を結果を届けずに捨てる: 追跡 id・保留フラグを下ろし、聞き取りが走っていれば止めて資源を解放する。
    // 権限ダイアログの許可コールバック（main スレッド）とは前後どちらでもよい。コールバックが先なら開始済みの聞き取りをここで止め、
    // こちらが先ならコールバックは追跡 id が一致しないことを見て何も始めない（その際ダイアログの照合状態はコールバックが下ろす
    // ため、ここでは触らない）。判定と更新は `speechLock` の内側で行う。`SpeechRecognizer` は main スレッドでしか触れないため、
    // 認識器の破棄だけ main へ積む（追跡 id は先に捨てているので、積んだ処理が走るまでの間に届く認識器の通知は照合で捨てられる）。
    static void abortPending() {
        synchronized (speechLock) {
            speechStartPendingResume = false;
            speechOwner = null;
            speechOwnerOrphaned = false;
            speechRequest.take();
            discardRecognizer();
        }
    }

    // 結果を届けずに認識器を破棄する（`speechLock` の内側で呼ぶ）。
    private static void discardRecognizer() {
        cancelSpeechTimeout();
        final SpeechRecognizer recognizer = speechRecognizer;
        speechRecognizer = null;
        if (recognizer != null) {
            speechTimeoutHandler.post(() -> {
                try {
                    recognizer.cancel();
                } catch (RuntimeException e) {
                    // 破棄が目的のため無視して destroy へ進む。
                }
                try {
                    recognizer.destroy();
                } catch (RuntimeException e) {
                    // 参照は既に捨てている。
                }
            });
        }
    }

    // 前面へ戻った（`SpeechRecognitionLifecycle.onHostResumed`）: 許可結果が非前景中に届き `startSpeechRecognizer` を保留していた
    // 要求を再開する（マイク録音の `micStartPendingResume` と同型）。
    static void resumed(Activity activity) {
        SpeechRecognitionHelper helper = of(activity);
        if (helper == null) {
            return;
        }
        helper.resumed = true;
        synchronized (speechLock) {
            if (speechStartPendingResume) {
                speechStartPendingResume = false;
                long id = speechRequest.current();
                if (id != RequestTracker.NONE && helper.speechRecognizer == null) {
                    helper.startSpeechRecognizer(id, speechRequestedMaxListenMs);
                }
            }
        }
    }

    // 前面を離れた（`SpeechRecognitionLifecycle.onHostPaused`）: 前面の写しを下ろすだけで、聞き取りは止めない。権限ダイアログ表示中
    // （`speechRecognizer == null` だが `speechRequest` は追跡中）も、構成変更中の聞き取りも、ここでは触らない。
    // 背景遷移での中断は `stopped` が行う（クラス冒頭コメント参照）。
    static void paused(Activity activity) {
        SpeechRecognitionHelper helper = of(activity);
        if (helper != null) {
            helper.resumed = false;
        }
    }

    // 見えなくなった（`SpeechRecognitionLifecycle.onHostStopped`）: 構成変更でなく、この Activity が要求の発行元なら背景遷移なので、
    // 聞き取り中なら中断（5）する（`cancel()` で OS 側の結果確定を求めない。背景で聞き取らない、device-bridge-contracts.md
    // 「ライフサイクル境界」節）。権限ダイアログ待ちは誤破棄しない（ダイアログは透過 Activity のため `onStop` を起こさない）。
    // 発行元でない Activity の停止では触らない。
    static void stopped(Activity activity) {
        SpeechRecognitionHelper helper = of(activity);
        if (helper == null || activity.isChangingConfigurations()) {
            return;
        }
        synchronized (speechLock) {
            if (speechOwner == helper) {
                interruptListeningLocked();
            }
        }
    }

    private static void interruptListeningLocked() {
        if (speechRecognizer != null) {
            abortSpeech(SPEECH_RESULT_INTERRUPTED);
        }
    }

    private static synchronized SpeechRecognitionHelper of(Activity activity) {
        return HELPERS.get(activity);
    }

    private SpeechRecognitionHelper(ComponentActivity activity) {
        this.activity = activity;
        this.requestSpeechPermission =
                activity.getActivityResultRegistry().register("fandhe:MainActivity:3", activity, new ActivityResultContracts.RequestPermission(), granted -> {
                synchronized (speechLock) {
                    long id = speechPermissionRequestId;
                    speechPermissionRequestId = RequestTracker.NONE;
                    // 届いたら即座に解除する。これ以降 `requestSpeechRecognitionStart` が
                    // 新しいダイアログを起動できるようになる（上記 `speechPermissionPending`
                    // doc 参照）。
                    speechPermissionPending = false;
                    if (id == RequestTracker.NONE || speechRequest.current() != id) {
                        // 自分が起動したダイアログの結果ではない（旧要求が「停止」で
                        // 破棄された後に新要求が追跡された、または既に破棄済み）。
                        // 新要求を壊さないよう何もしない。
                        return;
                    }
                    if (!granted) {
                        speechRequest.take();
                        nativeOnSpeechResult(id, SPEECH_RESULT_PERMISSION_DENIED, null);
                        return;
                    }
                    if (activity.isFinishing() || activity.isDestroyed()) {
                        // 真の破棄。`onPause` 止まりの非前景とは区別する（直下参照）。
                        speechRequest.take();
                        nativeOnSpeechResult(id, SPEECH_RESULT_INTERRUPTED, null);
                        return;
                    }
                    if (!this.resumed) {
                        // `RequestPermission` ダイアログの起動自体が Activity を pause
                        // するため、初回許可はほぼ必ずここを通る（`requestMicPermission`
                        // の同じ分岐、PR #962 是正と同じ理由）。「保留」として扱い、次の
                        // `onResume`（`speechStartPendingResume` 参照）で
                        // `startSpeechRecognizer` を実行する。
                        speechStartPendingResume = true;
                        return;
                    }
                    if (speechRecognizer != null) {
                        // 権限確認中に別経路から既に聞き取りが始まっている（通常到達
                        // しない防御的分岐）。
                        return;
                    }
                    startSpeechRecognizer(id, speechRequestedMaxListenMs);
                }
                });
    }

    // JNI から呼ばれる入口。即時 return・結果は `nativeOnSpeechStarted` / `nativeOnSpeechResult` / `nativeOnSpeechError`。
    // helper が無い（生成前・破棄後）ときは内部エラーで確定する（結果が来ないまま待たせない）。
    static void requestStart(Activity activity, long requestId, int maxListenMs) {
        SpeechRecognitionHelper helper = of(activity);
        if (helper == null) {
            nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
            return;
        }
        helper.requestSpeechRecognitionStart(requestId, maxListenMs);
    }

    // JNI から呼ばれる入口（停止）。helper が無ければ止めるものが無く、旧要求の遅延到着と同じく no-op。
    static void requestStop(Activity activity, long requestId) {
        SpeechRecognitionHelper helper = of(activity);
        if (helper != null) {
            helper.requestSpeechRecognitionStop(requestId);
        }
    }

    // 音声認識（Issue #1026、Phase 11 型サンプル「音声認識」の Android 経路）。
    // `SpeechRecognizer` trait は通らない（`fandhe-cap-speech-recognition/src/driver.rs` モジュール
    // doc「macOS/Windows/Android は pull 型 bridge」参照）。Rust 側は
    // `SpeechRecognitionHelper.requestStart(Activity, long, int)`/
    // `requestStop(Activity, long)`（提示依頼）を呼び、結果は
    // `nativeOnSpeechStarted`/`nativeOnSpeechResult`/`nativeOnSpeechError` で非同期に
    // 返す（`micRecordRequest`/`requestMicRecordStart` と同型）。
    private static native void nativeOnSpeechStarted(long requestId);

    // code の意味（本ファイルが SSOT、Rust 側 `speech_recognition::error_from_code` の
    // doc と対）: 0=成功（text 有効）／1=権限拒否／2=内部エラー／3=二重開始（busy）／
    // 4=許可確認中にキャンセル／5=ライフサイクル中断（背景遷移の `onHostStopped`・終了の `onHostDestroyed`・非前景での
    // 開始）／6=`isRecognitionAvailable == false`。
    private static native void nativeOnSpeechResult(long requestId, int code, String text);

    // `RecognitionListener.onError(int)` の生値をそのまま渡す（Java 側で分岐せず、
    // Rust 側 `driver::outcome_from_android_error`（SSOT）へ写像を
    // 一元化する、device-bridge-contracts.md「FFI 境界」節）。
    private static native void nativeOnSpeechError(long requestId, int error);

    private static final int SPEECH_RESULT_SUCCESS = 0;
    private static final int SPEECH_RESULT_PERMISSION_DENIED = 1;
    private static final int SPEECH_RESULT_INTERNAL_ERROR = 2;
    private static final int SPEECH_RESULT_BUSY = 3;
    private static final int SPEECH_RESULT_CANCELLED = 4;
    private static final int SPEECH_RESULT_INTERRUPTED = 5;
    private static final int SPEECH_RESULT_UNAVAILABLE = 6;

    // 手動停止後、`onResults`/`onError` が届かないまま要求を固着させないための保険の
    // 期限（読み上げの `TTS_ACCEPT_TIMEOUT_MS` と同型。PR #1030 レビュー指摘: `stopListening()` 後に
    // 認識サービスが応答しない場合、Rust 側の `tick` は `Stopping` では上限判定を
    // 進めないため、Android 側で決着させる必要がある）。
    private static final long SPEECH_STOP_RESULT_GRACE_MS = 5_000L;

    // `startListening()` 呼び出しから `onReadyForSpeech`/`onError` のどちらも届かない
    // まま画面が Listening 固着するのを防ぐ保険の期限（PR #1030 レビュー指摘）。
    // `onReadyForSpeech` 到達後は本来の聞き取り上限タイマー（`maxListenMs`）へ
    // `cancelSpeechTimeout()` 経由で切り替わるため、本タイマーは準備段階にのみ効く。
    private static final long SPEECH_READY_TIMEOUT_MS = 10_000L;

    // 「いま待っている要求 id」（`micRecordRequest` と同じ位置づけ）。UI スレッドのみで
    // 読み書きするが、`SpeechRecognizer`（`speechRecognizer`）とまとめて `speechLock` の
    // 内側で判定・更新する（並行性の契約を単一ロックへ集約する、
    // device-bridge-contracts.md「並行性・世代」節）。
    private static final RequestTracker speechRequest = new RequestTracker();

    // `speechLock` の内側でのみ読み書きする状態（ライフサイクル・並行性の契約を単一
    // ロックへ集約する、device-bridge-contracts.md 参照）。
    private static final Object speechLock = new Object();
    private static SpeechRecognizer speechRecognizer;
    private static final Handler speechTimeoutHandler = new Handler(Looper.getMainLooper());
    private static Runnable speechTimeout;
    // `onReadyForSpeech` 到達済みか（`startSpeechRecognizer` で false に初期化し、
    // `onReadyForSpeech` で true にする）。「聞き始めていない段階の停止は Cancelled」
    // という `fandhe-cap-speech-recognition` の `contract` の契約を守るため、
    // `requestSpeechRecognitionStop` がこの前後で分岐する（device-bridge-contracts.md
    // 「キャンセル・正常終了・失敗の三値」節）。`speechLock` の内側でのみ読み書きする。
    private static boolean speechStarted;
    // 停止要求（自動期限タイマー・`requestSpeechRecognitionStop` のいずれか）を既に
    // 認識器へ送ったか（`startSpeechRecognizer` で false に初期化する）。`speechStarted`
    // は結果到着まで true のままのため、この別フラグが無いと自動停止タイマーの発火後に
    // 手動「停止」が届いた場合（またはその逆）、同じ認識器へ `stopListening()` を二重に
    // 送ってしまい、かつ先に仕掛けた決着用の保険タイマー（`scheduleSpeechStopGrace`）を
    // 無条件に解除・再設定してしまう（PR #1030 レビュー指摘）。自動・手動どちらが先でも
    // 停止要求と保険タイマーの設定を一度きりにするための番人。`speechLock` の内側で
    // のみ読み書きする。
    private static boolean speechStopRequested;
    // 権限ダイアログ表示中に `requestSpeechPermission` コールバックが読む上限
    // （`requestSpeechRecognitionStart` が受け取った値をそのまま退避する）。
    private static volatile int speechRequestedMaxListenMs;
    // 許可結果が非前景中に届いたため `startSpeechRecognizer` の実行を次の `onResume` へ
    // 保留していることを示すフラグ（`micStartPendingResume` と同型）。`speechLock` の
    // 内側でのみ読み書きする。
    private static boolean speechStartPendingResume;

    // 権限ダイアログを起動した時点の要求 id（`requestSpeechRecognitionStart` が launch
    // 直前にここへ退避する）。コールバックは `speechRequest.current()` を直接読まない
    // （ダイアログ表示中に「停止」で `speechRequest.take()` された後、許可結果が届く前に
    // 新しい「話す」要求が追跡されると、`current()` は新要求の id を返してしまい、旧
    // ダイアログへの応答が新要求へ誤って適用される）。コールバック内でこの退避値と
    // `current()` を照合し、一致する場合のみ自分が起動したダイアログの結果とみなす。
    // `speechLock` の内側でのみ読み書きする。
    private static long speechPermissionRequestId = RequestTracker.NONE;

    // 権限ダイアログの結果がまだ届いていないか（launch 直前に true、コールバック冒頭で
    // 必ず false に戻す）。`speechPermissionRequestId` の単純な id 照合だけでは、
    // ダイアログ表示中に「停止」（`speechRequest.take()`）→ 新しい「話す」要求が追跡
    // される、という順でもう 1 回ダイアログを起動してしまうと、2 回目の launch が
    // `speechPermissionRequestId` を新要求の id で上書きし、まだ届いていない旧ダイアログの
    // コールバックが新要求の id と一致してしまい、旧ダイアログへの許可・拒否結果が新要求
    // のものとして誤適用される（PR #1030 レビュー指摘、device-bridge-contracts.md
    // 「並行性・世代」節）。本フラグが true の間は `requestSpeechRecognitionStart` が
    // 新しいダイアログを起動せず BUSY を返し、先行する権限要求が決着する（コールバックが
    // 届く）まで再起動を保留する。`speechLock` の内側でのみ読み書きする。
    private static boolean speechPermissionPending;


    // Rust 側 bridge（`fandhe-cap-speech-recognition/src/driver.rs`）の `present`
    // （Android 分岐）から `requestStart`（static の入口）経由で呼ばれる（Issue #1026）。
    // マイク録音の `requestMicRecordStart` と同じ「即時 return・結果は非同期通知」の形。`maxListenMs` は Rust 側
    // `MAX_LISTEN_MS`（30 秒固定）をそのまま `Handler.postDelayed` へ渡す（期限を
    // 複製しない、device-bridge-contracts.md「期限・キャンセル」節）。
    private void requestSpeechRecognitionStart(long requestId, int maxListenMs) {
        activity.runOnUiThread(() -> {
            synchronized (speechLock) {
                if (speechRecognizer != null || speechRequest.current() != RequestTracker.NONE
                        || speechPermissionPending) {
                    // 既に聞き取り中、権限確認待ち中、または先行する権限ダイアログの
                    // 結果がまだ届いていない（`speechPermissionPending` doc 参照。
                    // 「停止」で `speechRequest` 側だけ先に解除されていても、ダイアログ
                    // 自体はまだ閉じていないため新規に起動しない）の二重開始（現在の
                    // 要求は壊さない）。
                    nativeOnSpeechResult(requestId, SPEECH_RESULT_BUSY, null);
                    return;
                }
                speechRequest.track(requestId);
                speechOwner = this;
                speechOwnerOrphaned = false;
                speechRequestedMaxListenMs = maxListenMs;
                if (activity.isFinishing() || activity.isDestroyed() || !resumed) {
                    // `requestMicRecordStart` と同じ非前景ガード。
                    speechRequest.take();
                    nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERRUPTED, null);
                    return;
                }
                // 非対応端末では権限ダイアログを出す前に弾く（実装計画 §2.4 手順 3。
                // 権限確認より先に判定する）。
                if (!SpeechRecognizer.isRecognitionAvailable(activity)) {
                    speechRequest.take();
                    nativeOnSpeechResult(requestId, SPEECH_RESULT_UNAVAILABLE, null);
                    return;
                }
                if (ContextCompat.checkSelfPermission(activity, Manifest.permission.RECORD_AUDIO)
                        == PackageManager.PERMISSION_GRANTED) {
                    startSpeechRecognizer(requestId, maxListenMs);
                    return;
                }
                try {
                    // launch 直前に id を退避する（上記 `speechPermissionRequestId`
                    // doc 参照。コールバックはこの値で現在の要求と照合する）。
                    // `speechPermissionPending` も同時に立て、この結果が届くまで
                    // 次の要求が新しいダイアログを起動しないようにする。
                    speechPermissionRequestId = requestId;
                    speechPermissionPending = true;
                    requestSpeechPermission.launch(Manifest.permission.RECORD_AUDIO);
                } catch (RuntimeException e) {
                    // `launch()` 自体が例外を送出した場合（`requestMicRecordStart` と
                    // 同じ理由）。コールバックは二度と来ないため、ここで直接失敗を
                    // 通知し、保留フラグも解除する。
                    speechPermissionRequestId = RequestTracker.NONE;
                    speechPermissionPending = false;
                    speechRequest.take();
                    nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
                }
            }
        });
    }

    // 権限確認後（`requestSpeechRecognitionStart`/`requestSpeechPermission` 許可
    // コールバックのいずれか）に呼ぶ。呼び出し元が `speechLock` の内側であることを
    // 保証する。`SpeechRecognizer`/`RecognitionListener` のコールバックはいずれも
    // `createSpeechRecognizer` を呼んだスレッド（本メソッドは常に `runOnUiThread`
    // 経由で呼ばれる）の Looper から届く契約のため、UI スレッドに閉じて動作する
    // （`speechTimeoutHandler` も `Looper.getMainLooper()` で同じスレッド、
    // device-bridge-contracts.md「並行性・世代」節）。
    private void startSpeechRecognizer(long requestId, int maxListenMs) {
        SpeechRecognizer recognizer;
        try {
            recognizer = SpeechRecognizer.createSpeechRecognizer(activity.getApplicationContext());
        } catch (RuntimeException e) {
            speechRequest.take();
            nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
            return;
        }
        recognizer.setRecognitionListener(newListener(requestId, maxListenMs));

        Intent intent = new Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH);
        intent.putExtra(
                RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM);
        intent.putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1);
        intent.putExtra(RecognizerIntent.EXTRA_CALLING_PACKAGE, activity.getPackageName());
        try {
            recognizer.startListening(intent);
        } catch (RuntimeException e) {
            try {
                recognizer.destroy();
            } catch (RuntimeException ignored) {
                // 要求の解除と結果配送は続ける（`abortSpeech` と同じ方針）
            }
            speechRequest.take();
            nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
            return;
        }
        speechRecognizer = recognizer;
        speechStartPendingResume = false;
        // `onReadyForSpeech` 未到達を明示する初期値（聞き取り上限タイマーは同
        // コールバックで開始する。上記コメント参照）。
        speechStarted = false;
        // 新しい要求ごとに停止要求の送出有無をリセットする（`speechStopRequested`
        // doc 参照）。
        speechStopRequested = false;
        // 準備待ちの保険タイマー（`SPEECH_READY_TIMEOUT_MS` doc 参照）。
        // `onReadyForSpeech`/`onError` のどちらも届かないまま画面が Listening
        // 固着するのを防ぐ（PR #1030 レビュー指摘）。`onReadyForSpeech` 到達時は
        // 上の分岐で `cancelSpeechTimeout()` により解除される。
        speechTimeout = () -> {
            synchronized (speechLock) {
                if (speechRequest.current() == requestId && !speechStarted) {
                    if (finishSpeech(requestId)) {
                        nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
                    }
                }
            }
        };
        speechTimeoutHandler.postDelayed(speechTimeout, SPEECH_READY_TIMEOUT_MS);
    }

    // 認識リスナーを作る。static にして旧 helper / Activity を参照させない（再生成をまたいで生きるため）。
    private static RecognitionListener newListener(final long requestId, final int maxListenMs) {
        return new RecognitionListener() {
            @Override
            public void onReadyForSpeech(Bundle params) {
                synchronized (speechLock) {
                    if (speechRequest.current() == requestId) {
                        // 準備待ちの保険タイマー（`SPEECH_READY_TIMEOUT_MS`）を解除して
                        // から本来の聞き取り上限タイマーへ切り替える。解除せず
                        // `speechTimeout` を上書きするだけだと、保険タイマーの
                        // Runnable が Handler のキューに残り続け、聞き取り中に不意に
                        // 発火してしまう。
                        cancelSpeechTimeout();
                        speechStarted = true;
                        // 期限は `onReadyForSpeech`（認識サービスの準備完了）起点で
                        // 数える。Rust 側 `nativeOnSpeechStarted` も同じ到達を起点に
                        // 経過時間を数える契約のため、`startListening()` 直後を起点に
                        // すると準備待ちの遅延ぶん OS 側の期限が先に来てしまう
                        // （PR #1030 レビュー指摘）。
                        speechTimeout = () -> {
                            synchronized (speechLock) {
                                if (speechRequest.current() == requestId
                                        && speechRecognizer != null
                                        && !speechStopRequested) {
                                    // 手動 `requestSpeechRecognitionStop` と停止要求の
                                    // 送出を一度きりにする番人（PR #1030 レビュー指摘。
                                    // `speechStopRequested` doc 参照）。
                                    speechStopRequested = true;
                                    if (!stopListeningOrFail(requestId)) {
                                        return;
                                    }
                                    // 自動停止後も手動停止と同じ保険期限を掛ける
                                    // （PR #1030 レビュー指摘: 自動停止だけ
                                    // `onResults`/`onError` 未到達の決着期限が無く、
                                    // 要求が永久に残っていた）。
                                    scheduleSpeechStopGrace(requestId);
                                }
                            }
                        };
                        speechTimeoutHandler.postDelayed(speechTimeout, maxListenMs);
                        nativeOnSpeechStarted(requestId);
                    }
                }
            }

            @Override
            public void onResults(Bundle results) {
                synchronized (speechLock) {
                    ArrayList<String> matches =
                            results.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION);
                    String text = (matches == null || matches.isEmpty()) ? "" : matches.get(0);
                    if (finishSpeech(requestId)) {
                        nativeOnSpeechResult(requestId, SPEECH_RESULT_SUCCESS, text);
                    }
                }
            }

            @Override
            public void onError(int error) {
                synchronized (speechLock) {
                    if (finishSpeech(requestId)) {
                        nativeOnSpeechError(requestId, error);
                    }
                }
            }

            @Override
            public void onBeginningOfSpeech() {}

            @Override
            public void onRmsChanged(float rmsdB) {}

            @Override
            public void onBufferReceived(byte[] buffer) {}

            @Override
            public void onEndOfSpeech() {}

            @Override
            public void onPartialResults(Bundle partialResults) {}

            @Override
            public void onEvent(int eventType, Bundle params) {}
        };
    }

    // Rust 側（`fandhe-cap-speech-recognition` の driver の「停止」ボタンハンドラ）から `requestStop` 経由で
    // 呼ばれる（Issue #1026）。「停止」＝「話し終わった」の合図として OS に結果の確定を
    // 求めるだけで、確定結果は `onResults`/`onError` が別途届ける
    // （`fandhe-cap-speech-recognition` の `contract` の doc「停止ボタンの意味」節）。
    // `requestId` が現在の要求と一致しない場合は no-op（旧要求の遅延到着、
    // `requestMicRecordStop` の Android 版）。
    private void requestSpeechRecognitionStop(long requestId) {
        activity.runOnUiThread(() -> {
            synchronized (speechLock) {
                if (speechRequest.current() != requestId) {
                    return;
                }
                if (speechRecognizer == null) {
                    // 権限確認中（ダイアログ表示中）の「停止」。正常なキャンセル。
                    speechRequest.take();
                    nativeOnSpeechResult(requestId, SPEECH_RESULT_CANCELLED, null);
                    return;
                }
                if (!speechStarted) {
                    // `startListening()` 直後〜`onReadyForSpeech` 到達前の「停止」。
                    // まだ聞き取りを始めていないためキャンセル扱い（契約は
                    // `fandhe-cap-speech-recognition` の `contract` の「聞き始めていない段階の
                    // 停止は Cancelled」）。`stopListening()` を呼ぶと後続の
                    // `onError` が Cancelled ではなくエラーへ写ってしまうため、
                    // `onStop`/`onDestroy` と同じ `cancel()`→`destroy()` で終える
                    // （PR #1030 レビュー指摘）。
                    abortSpeech(SPEECH_RESULT_CANCELLED);
                    return;
                }
                // 手動停止は結果確定を OS に求めるだけで、確定は `onResults`/`onError`
                // が届けて `finishSpeech` がタイマーを解除する。自動停止タイマーが
                // 既に発火して停止要求を送出済みの場合（`speechStopRequested`）は
                // 何もしない。ここで `stopListening()` を再送すると同じ認識器への
                // 二重呼び出しになり、かつ既に仕掛けた決着用の保険タイマーを
                // 無条件に解除・再設定してしまう（PR #1030 レビュー指摘）。
                if (speechStopRequested) {
                    return;
                }
                // 結果到着前に期限タイマーが発火すると同じ認識器へ
                // `stopListening()` を二重に呼んでしまう（PR #1030 レビュー指摘）
                // ため、手動停止の時点でタイマーを解除し、停止要求を一度だけ送る。
                speechStopRequested = true;
                cancelSpeechTimeout();
                if (!stopListeningOrFail(requestId)) {
                    return;
                }
                scheduleSpeechStopGrace(requestId);
            }
        });
    }

    // 停止要求（手動「停止」ボタン・自動停止タイマーの両方）の送出直後に呼ぶ、保険の
    // 期限（`SPEECH_STOP_RESULT_GRACE_MS`）を仕掛ける（`speechLock` の内側であることを
    // 呼び出し元が保証する）。認識サービスが `stopListening()` に対して
    // `onResults`/`onError` のどちらも返さない場合、要求が永久に完了せず画面が固着
    // する（PR #1030 レビュー指摘。Rust 側 `tick` は `Stopping` では上限判定を進めない
    // ため Android 側で決着させる必要がある。自動停止だけこの保険が無いと再発した
    // ため、手動停止と共用の 1 箇所にまとめる）。期限に達してもまだ現在の要求のままなら
    // 内部エラーとして畳んで破棄する。呼び出し元が既に `speechTimeout` を使い切っている
    // 前提（直前に `cancelSpeechTimeout()` 済み、または `stopListening()` 直前の
    // タイマー発火自身からの呼び出し）で上書きする。
    private static void scheduleSpeechStopGrace(long requestId) {
        speechTimeout = () -> {
            synchronized (speechLock) {
                if (finishSpeech(requestId)) {
                    nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
                }
            }
        };
        speechTimeoutHandler.postDelayed(speechTimeout, SPEECH_STOP_RESULT_GRACE_MS);
    }

    // 聞き取りセッションを 1 つ決着させる（`onResults`/`onError`/`onStop`/`onDestroy`
    // のいずれの経路からも呼ぶ、`speechLock` の内側であることを呼び出し元が保証する）。
    // `speechRequest.complete(requestId)` が false（旧世代・二重完了）なら false を
    // 返し、呼び出し元は結果を届けない。結果を届ける前に `destroy`・タイマー解除・
    // フィールドの null 化を済ませる（device-bridge-contracts.md「並行性・世代」節
    // 「結果を届ける前に状態を確定させる」）。
    private static boolean finishSpeech(long requestId) {
        if (!speechRequest.complete(requestId)) {
            return false;
        }
        cancelSpeechTimeout();
        try {
            speechRecognizer.destroy();
        } catch (RuntimeException e) {
            // 認識サービス側の障害で destroy が失敗しても、参照は捨てて結果は届ける
            // （ここで抜けると要求が永久に残る）。
        }
        speechRecognizer = null;
        speechStartPendingResume = false;
        return true;
    }

    // 聞き取り中の認識器を結果確定なしで破棄し、現在の要求を `resultCode` で完了させる
    // （「停止」の未開始分岐・`onStop`・`onDestroy` から、`speechLock` の内側で
    // `speechRecognizer != null` を確かめてから呼ぶ）。認識サービスの障害で `cancel()` /
    // `destroy()` が `RuntimeException` を送出しても、状態を解除して結果を必ず届ける
    // （ここで抜けると要求が永久に残り以後の開始が busy になる。PR #1030 レビュー指摘、
    // `finishSpeech` と同じ方針）。
    private static void abortSpeech(int resultCode) {
        long id = speechRequest.take();
        cancelSpeechTimeout();
        try {
            speechRecognizer.cancel();
        } catch (RuntimeException e) {
            // destroy と結果配送は続ける
        }
        try {
            speechRecognizer.destroy();
        } catch (RuntimeException e) {
            // 参照は捨てて結果は届ける
        }
        speechRecognizer = null;
        speechStartPendingResume = false;
        nativeOnSpeechResult(id, resultCode, null);
    }

    // `stopListening()` を呼ぶ（手動停止・自動停止の両方から、`speechLock` の内側で呼ぶ）。
    // 認識サービスの障害で `RuntimeException` が出ると UI スレッドの処理が未捕捉例外で
    // 終了するため、捕捉して認識器とタイマーを破棄し、要求を内部エラーとして完了させる
    // （PR #1030 codex 指摘）。false を返したら呼び出し元は保険タイマーを掛けずに抜ける。
    private static boolean stopListeningOrFail(long requestId) {
        try {
            speechRecognizer.stopListening();
            return true;
        } catch (RuntimeException e) {
            if (finishSpeech(requestId)) {
                nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
            }
            return false;
        }
    }

    // `speechTimeout`（期限タイマー／手動停止後の保険タイマーの共用フィールド）を
    // 解除する（`speechLock` の内側であることを呼び出し元が保証する）。`speechTimeout`
    // は `onReadyForSpeech` 到達後にのみ設定されるため、到達前の停止・エラー経路
    // （`requestSpeechRecognitionStop` の未開始分岐・`onError` 経由の `finishSpeech`・
    // `onStop`/`onDestroy`）では null のままになる。`Handler.removeCallbacks(null)` を
    // 無条件に呼ばない（PR #1030 レビュー指摘）。
    private static void cancelSpeechTimeout() {
        if (speechTimeout != null) {
            speechTimeoutHandler.removeCallbacks(speechTimeout);
            speechTimeout = null;
        }
    }
}
