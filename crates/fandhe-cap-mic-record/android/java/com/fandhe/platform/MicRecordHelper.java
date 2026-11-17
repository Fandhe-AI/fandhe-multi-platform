package com.fandhe.platform;

import android.Manifest;
import android.app.Activity;
import android.content.pm.PackageManager;
import android.media.MediaRecorder;
import android.os.SystemClock;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.util.Map;
import java.util.WeakHashMap;

// マイク録音（Issue #957）の Android 実装ヘルパー（Issue #1354 で `MainActivity` から機能 crate へ移した）。
// `MicRecordLifecycle` が Activity ごとに生成する（Rust の `fandhe-cap-mic-record/src/android.rs` が static メソッド
// `requestStart(Activity, long, int)` / `requestStop(Activity, long)` を JNI で呼び、結果を `nativeOnMicRecordStarted` /
// `nativeOnMicRecordResult` で受ける）。
//
// helper が無い（生成前・破棄後）要求は、成功にも待ちにもせず内部エラー（code 2）で確定する（結果が来ないまま待たせない）。
// 許可ランチャーは `attach`（`onHostCreated`、STARTED より前）で登録する。キーは従来の `MainActivity` の文字列のまま。
// 前面かどうか（`resumed`）は `MicRecordLifecycle` の `onHostResumed` / `onHostPaused` が写す。どちらも `super.onResume()` /
// `super.onPause()` の中で届き、従来の `MainActivity` が `super` の後で更新していたのとは前後だけが違う。許可ダイアログの応答との
// 到着順（契約表 1）は、応答側が `resumed` を見て保留（`micStartPendingResume`）にし、次の `onHostResumed` で再開する形で保つ。
final class MicRecordHelper {
    private final Activity activity;
    // UI スレッドだけが読み書きする前面の写し（従来の `MainActivity.activityResumed`）。`attach` の時点では前面ではない。
    private boolean resumed;
    // マイク録音の実行時許可要求。許可が下りたら `startMicRecorder` を呼ぶ。`registerForActivityResult` は STARTED より前に
    // 呼ぶ契約のため、`attach`（`onHostCreated`、onCreate 内）で生成する。
    private final ActivityResultLauncher<String> requestMicPermission;

    // Activity ごとの helper。`MicRecordLifecycle`（`FeatureLifecycleProvider`）が生成時に付け、破棄時に外す。
    private static final Map<Activity, MicRecordHelper> HELPERS = new WeakHashMap<>();

    static void attach(ComponentActivity activity) {
        // 再生成: 要求 id・上限・保留フラグ（いずれも static）は捨てない。権限ダイアログの結果は ActivityResultRegistry が同じキーの
        // 新しい許可ランチャーへ配送するため、新 helper の callback が引き継いで録音を始める（結果は同じ id で 1 度だけ届く）。
        // 録音中だった場合は `onHostPaused` が先に中断（5）で確定済みで、追跡は空になっている。
        MicRecordHelper helper = new MicRecordHelper(activity);
        synchronized (MicRecordHelper.class) {
            HELPERS.put(activity, helper);
        }
    }

    // Activity 破棄時: 録音中なら破棄する（`onHostPaused` で既に破棄済みなら `micRecorder == null` で no-op）。`onPause` を経由せず
    // 直接 `onDestroy` へ来るケース（Android の契約上は稀）に備えて同じ処理を置く。
    // 終了による破棄（`isFinishing()`）では registry ごと消え、権限ダイアログの結果は二度と届かない。保留中の要求を中断（5）として
    // 1 度だけ確定する。終了でない破棄（再生成・システム破棄）は結果が復元後の helper へ届くため何も捨てない。
    // `complete` / `take` が first-wins なので、許可コールバックとの二重通知にならない。
    static void detach(Activity activity) {
        MicRecordHelper helper;
        synchronized (MicRecordHelper.class) {
            helper = HELPERS.remove(activity);
        }
        if (helper != null) {
            helper.interruptIfRecording();
        }
        if (activity.isFinishing()) {
            synchronized (micLock) {
                micStartPendingResume = false;
                long id = micRecordRequest.take();
                if (id != RequestTracker.NONE) {
                    nativeOnMicRecordResult(id, MIC_RESULT_INTERRUPTED, null, null, 0);
                }
            }
        }
    }

    // Rust 側 `reset_pending_tracking()`（`android_main`）から JNI で呼ばれる。Rust は保留中の要求を既に捨てて待っていないため、
    // Java 側も同じ要求を結果を届けずに捨てる: 追跡 id・保留フラグを下ろし、録音が走っていれば止めて資源を解放する。
    // 権限ダイアログの許可コールバック（main スレッド）とは前後どちらでもよい。コールバックが先なら開始済みの録音をここで止め、
    // こちらが先ならコールバックは追跡 id が無いことを見て何も始めない。判定と更新は `micLock` の内側で行う。
    static void abortPending() {
        synchronized (micLock) {
            micStartPendingResume = false;
            micRecordRequest.take();
            MicRecordHelper[] helpers;
            synchronized (MicRecordHelper.class) {
                helpers = HELPERS.values().toArray(new MicRecordHelper[0]);
            }
            for (MicRecordHelper helper : helpers) {
                helper.discardRecorder();
            }
        }
    }

    // 結果を届けずに録音を破棄する（`micLock` の内側で呼ぶ）。`stop()` の失敗は握りつぶすが `release` と一時ファイル削除は必ず行う。
    private void discardRecorder() {
        MediaRecorder recorder = micRecorder;
        File tempFile = micTempFile;
        micRecorder = null;
        micTempFile = null;
        if (recorder != null) {
            try {
                recorder.stop();
            } catch (RuntimeException e) {
                // 破棄が目的のため、停止の失敗は無視して解放へ進む。
            }
            recorder.release();
        }
        deleteQuietly(tempFile);
    }

    // 前面へ戻った（`MicRecordLifecycle.onHostResumed`）: 許可結果が非前景中に届き `startMicRecorder` を保留していた要求を
    // 再開する（cursor Bugbot 指摘、PR #962。`micStartPendingResume` doc 参照）。
    static void resumed(Activity activity) {
        MicRecordHelper helper = of(activity);
        if (helper == null) {
            return;
        }
        helper.resumed = true;
        synchronized (micLock) {
            if (micStartPendingResume) {
                micStartPendingResume = false;
                long id = micRecordRequest.current();
                if (id != RequestTracker.NONE && helper.micRecorder == null) {
                    helper.startMicRecorder(id, micRequestedMaxDurationMs);
                }
            }
        }
    }

    // 前面を離れた（`MicRecordLifecycle.onHostPaused`）: バックグラウンド遷移で録音中なら破棄する（保留要求を残さない、
    // device-bridge-contracts.md「ライフサイクル境界」節）。権限ダイアログ表示中（`micRecorder == null` だが `micRecordRequest` は
    // 追跡中）は誤破棄しない（ダイアログ自体は Activity 再生成の対象外で表示され続ける契約）。
    static void paused(Activity activity) {
        MicRecordHelper helper = of(activity);
        if (helper == null) {
            return;
        }
        helper.resumed = false;
        helper.interruptIfRecording();
    }

    private void interruptIfRecording() {
        synchronized (micLock) {
            if (micRecorder != null) {
                finishMicRecord(micRecordRequest.current(), MIC_RESULT_INTERRUPTED);
            }
        }
    }

    private static synchronized MicRecordHelper of(Activity activity) {
        return HELPERS.get(activity);
    }

    private MicRecordHelper(ComponentActivity activity) {
        this.activity = activity;
        this.requestMicPermission =
                activity.getActivityResultRegistry().register("fandhe:MainActivity:2", activity, new ActivityResultContracts.RequestPermission(), granted -> {
                synchronized (micLock) {
                    long id = micRecordRequest.current();
                    if (id == RequestTracker.NONE) {
                        // 既に破棄済み（`onPause`/`onDestroy`/再生成、または二重開始で
                        // 弾かれた後）。
                        return;
                    }
                    if (!granted) {
                        micRecordRequest.take();
                        nativeOnMicRecordResult(id, MIC_RESULT_PERMISSION_DENIED, null, null, 0);
                        return;
                    }
                    if (activity.isFinishing() || activity.isDestroyed()) {
                        // 真の破棄。`onPause` 止まりの非前景とは区別する（直下参照）。
                        micRecordRequest.take();
                        nativeOnMicRecordResult(id, MIC_RESULT_INTERRUPTED, null, null, 0);
                        return;
                    }
                    if (!this.resumed) {
                        // cursor Bugbot 是正（PR #962）: `RequestPermission` ダイアログの
                        // 起動自体が Activity を pause するため、初回許可はほぼ必ず
                        // ここを通る。旧実装はこれを `MIC_RESULT_INTERRUPTED` として
                        // 畳み、正常な許可でも `startMicRecorder` を一度も呼んでい
                        // なかった。location の `locationStartRequestPending` と同型に
                        // 「保留」として扱い、次の `onResume`（`micStartPendingResume`
                        // 参照）で `startMicRecorder` を実行する。
                        micStartPendingResume = true;
                        return;
                    }
                    if (micRecorder != null) {
                        // 権限確認中に別経路から既に録音が始まっている（通常到達しない
                        // 防御的分岐）。
                        return;
                    }
                    startMicRecorder(id, micRequestedMaxDurationMs);
                }
                });
    }

    // JNI から呼ばれる入口。即時 return・結果は `nativeOnMicRecordStarted` / `nativeOnMicRecordResult`。helper が無い
    // （生成前・破棄後）ときは内部エラーで確定する（結果が来ないまま待たせない）。
    static void requestStart(Activity activity, long requestId, int maxDurationMs) {
        MicRecordHelper helper = of(activity);
        if (helper == null) {
            nativeOnMicRecordResult(requestId, MIC_RESULT_INTERNAL_ERROR, null, null, 0);
            return;
        }
        helper.requestMicRecordStart(requestId, maxDurationMs);
    }

    // JNI から呼ばれる入口（停止）。helper が無ければ止めるものが無く、旧要求の遅延到着と同じく no-op（Rust 側は
    // 開始時に helper の有無を結果で受けている）。
    static void requestStop(Activity activity, long requestId) {
        MicRecordHelper helper = of(activity);
        if (helper != null) {
            helper.requestMicRecordStop(requestId);
        }
    }

    // マイク録音（Issue #957、Phase 11 型サンプル「マイク録音」の Android 経路）。
    // `MicRecorder` trait は通らない（`fandhe-cap-mic-record/src/driver.rs` モジュール doc の
    // 「macOS/Android は pull 型 bridge」参照）。Rust 側は
    // `MicRecordHelper.requestStart(Activity, long, int)`/`requestStop(Activity, long)`
    // （提示依頼）を呼び、結果は `nativeOnMicRecordStarted`/`nativeOnMicRecordResult`
    // で非同期に返す（`BiometricAuthHelper` と同型）。
    private static native void nativeOnMicRecordStarted(long requestId);

    // code の意味（本ファイルが SSOT、Rust 側 `mic_record::error_from_code` の doc と
    // 対）: 0=成功（bytes/mime/durationMs 有効）／1=権限拒否／2=内部エラー／
    // 3=二重開始（busy）／4=許可確認中にキャンセル／5=ライフサイクル中断
    // （`onHostPaused`/`onHostDestroyed`/Activity 再生成で破棄）。
    private static native void nativeOnMicRecordResult(
            long requestId, int code, byte[] bytes, String mime, long durationMs);

    private static final int MIC_RESULT_SUCCESS = 0;
    private static final int MIC_RESULT_PERMISSION_DENIED = 1;
    private static final int MIC_RESULT_INTERNAL_ERROR = 2;
    private static final int MIC_RESULT_BUSY = 3;
    private static final int MIC_RESULT_CANCELLED = 4;
    private static final int MIC_RESULT_INTERRUPTED = 5;

    // 「いま待っている要求 id」（生体認証の要求 id と同じ位置づけ）。UI スレッドのみで
    // 読み書きするが、`MediaRecorder`（`micRecorder`/`micTempFile`/`micStartedAt`）と
    // まとめて `micLock` の内側で判定・更新する（並行性の契約を単一ロックへ集約する、
    // device-bridge-contracts.md「並行性・世代」節）。
    private static final RequestTracker micRecordRequest = new RequestTracker();

    // `micLock` の内側でのみ読み書きする状態（ライフサイクル・並行性の契約を単一
    // ロックへ集約する、device-bridge-contracts.md 参照）。ロックと、再生成をまたいで引き継ぐ
    // `micRequestedMaxDurationMs` / `micStartPendingResume` は static（新 helper が同じ値で再開できるように）。
    private static final Object micLock = new Object();
    private MediaRecorder micRecorder;
    private File micTempFile;
    private long micStartedAt;
    // 権限ダイアログ表示中に `requestMicPermission` コールバックが読む上限
    // （`requestMicRecordStart` が受け取った値をそのまま退避する）。
    private static volatile int micRequestedMaxDurationMs;

    // 許可結果が非前景中に届いたため `startMicRecorder` の実行を次の `onResume` へ
    // 保留していることを示すフラグ（cursor Bugbot 指摘、PR #962。location の
    // `locationStartRequestPending` と同型）。`micLock` の内側でのみ読み書きする。
    private static boolean micStartPendingResume;

    // マイク録音の実行時許可要求（`CameraCaptureHelper` の `requestCameraPermission`/`requestLocationPermission`
    // と同型）。許可が下りたら `startMicRecorder` を呼ぶ。

    // Rust 側 bridge（`fandhe-cap-mic-record/src/driver.rs`）の `present`（Android 分岐）から
    // `requestStart`（static の入口）経由で呼ばれる（Issue #957）。「即時 return・結果は
    // 非同期通知」の形。`maxDurationMs` は Rust 側 `MAX_RECORD_MS`（30 秒固定）を
    // そのまま `MediaRecorder.setMaxDuration` へ渡す（期限を複製しない、
    // device-bridge-contracts.md「期限・キャンセル」節）。
    private void requestMicRecordStart(long requestId, int maxDurationMs) {
        activity.runOnUiThread(() -> {
            synchronized (micLock) {
                if (micRecorder != null || micRecordRequest.current() != RequestTracker.NONE) {
                    // 既に録音中、または権限確認待ち中の二重開始（現在の要求は壊さない）。
                    nativeOnMicRecordResult(requestId, MIC_RESULT_BUSY, null, null, 0);
                    return;
                }
                micRecordRequest.track(requestId);
                micRequestedMaxDurationMs = maxDurationMs;
                if (activity.isFinishing() || activity.isDestroyed() || !resumed) {
                    // biometric/permission コールバックと同じ非前景ガード
                    // （`requestMicPermission` 参照）。ここで弾いておけば、
                    // 権限確認へ進んだ場合もダイアログ表示時点で前景が保証される。
                    micRecordRequest.take();
                    nativeOnMicRecordResult(requestId, MIC_RESULT_INTERRUPTED, null, null, 0);
                    return;
                }
                if (ContextCompat.checkSelfPermission(activity, Manifest.permission.RECORD_AUDIO)
                        == PackageManager.PERMISSION_GRANTED) {
                    startMicRecorder(requestId, maxDurationMs);
                    return;
                }
                try {
                    requestMicPermission.launch(Manifest.permission.RECORD_AUDIO);
                } catch (RuntimeException e) {
                    // `launch()` 自体が例外を送出した場合（カメラ撮影の `begin` と同じ
                    // 理由）。コールバックは二度と来ないため、ここで直接失敗を通知する。
                    micRecordRequest.take();
                    nativeOnMicRecordResult(requestId, MIC_RESULT_INTERNAL_ERROR, null, null, 0);
                }
            }
        });
    }

    // 権限確認後（`requestMicRecordStart`/`requestMicPermission` 許可コールバックの
    // いずれか）に呼ぶ。呼び出し元が `micLock` の内側であることを保証する。
    // `new MediaRecorder()`（無引数）は API 31 で `MediaRecorder(Context)` に非推奨化
    // されたが、`MediaRecorder(Context)` は API 31 未満（minSdk 24）の実機に存在しない
    // ため使えない（新規追加 API を分岐なしで呼ぶと旧 OS で `NoSuchMethodError`）。
    @SuppressWarnings("deprecation")
    private void startMicRecorder(long requestId, int maxDurationMs) {
        File tempFile;
        try {
            tempFile = File.createTempFile("fandhe_mic_", ".m4a", activity.getCacheDir());
        } catch (IOException e) {
            micRecordRequest.take();
            nativeOnMicRecordResult(requestId, MIC_RESULT_INTERNAL_ERROR, null, null, 0);
            return;
        }
        MediaRecorder recorder = new MediaRecorder();
        try {
            recorder.setAudioSource(MediaRecorder.AudioSource.MIC);
            recorder.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4);
            recorder.setAudioEncoder(MediaRecorder.AudioEncoder.AAC);
            recorder.setAudioChannels(1);
            recorder.setAudioEncodingBitRate(64_000);
            recorder.setAudioSamplingRate(44_100);
            recorder.setOutputFile(tempFile.getAbsolutePath());
            recorder.setMaxDuration(maxDurationMs);
            // リスナーは `MediaRecorder` を生成したスレッド（本メソッドは常に
            // `runOnUiThread` 経由で呼ばれる、`requestMicRecordStart`/
            // `requestMicPermission` 参照）の Looper から届く契約。`finishMicRecord` は
            // `micLock` の内側であることを要求するため、ここでも明示的に取る
            // （UI スレッド前提に依存しない防御、device-bridge-contracts.md
            // 「並行性・世代」節）。
            recorder.setOnInfoListener((mr, what, extra) -> {
                if (what == MediaRecorder.MEDIA_RECORDER_INFO_MAX_DURATION_REACHED) {
                    synchronized (micLock) {
                        // `MEDIA_RECORDER_INFO_MAX_DURATION_REACHED` の時点で
                        // `MediaRecorder` は既に内部で停止済み（Cursor Bugbot 指摘）。
                        // ここで再度 `stop()` を呼ぶと `IllegalStateException` になり、
                        // 完成した録音を内部エラー扱いで捨ててしまう。
                        finishMicRecord(requestId, MIC_RESULT_SUCCESS, true);
                    }
                }
            });
            recorder.setOnErrorListener((mr, what, extra) -> {
                synchronized (micLock) {
                    finishMicRecord(requestId, MIC_RESULT_INTERNAL_ERROR);
                }
            });
            recorder.prepare();
            recorder.start();
        } catch (IOException | RuntimeException e) {
            recorder.release();
            deleteQuietly(tempFile);
            micRecordRequest.take();
            nativeOnMicRecordResult(requestId, MIC_RESULT_INTERNAL_ERROR, null, null, 0);
            return;
        }
        micRecorder = recorder;
        micTempFile = tempFile;
        micStartedAt = SystemClock.elapsedRealtime();
        nativeOnMicRecordStarted(requestId);
    }

    // Rust 側（`fandhe-cap-mic-record` の driver の「停止」ボタンハンドラ）から `requestStop` 経由で呼ばれる
    // （Issue #957）。`requestId` が現在の要求と一致しない場合は no-op（旧要求の遅延
    // 到着、`apple.rs::request_stop` の Android 版）。
    private void requestMicRecordStop(long requestId) {
        activity.runOnUiThread(() -> {
            synchronized (micLock) {
                if (micRecordRequest.current() != requestId) {
                    return;
                }
                if (micRecorder == null) {
                    // 権限確認中（ダイアログ表示中）の「停止」。正常なキャンセル。
                    micRecordRequest.take();
                    nativeOnMicRecordResult(requestId, MIC_RESULT_CANCELLED, null, null, 0);
                    return;
                }
                finishMicRecord(requestId, MIC_RESULT_SUCCESS);
            }
        });
    }

    // 録音セッションを 1 つ決着させる（自動停止・明示停止・`onPause`/`onDestroy` の
    // いずれの経路からも呼ぶ、`micLock` の内側であることを呼び出し元が保証する）。
    // `code` は「録音が正常に走っていた場合」の決着コード（`MIC_RESULT_SUCCESS`/
    // `MIC_RESULT_INTERRUPTED`）。`stop()` が失敗すれば `MIC_RESULT_INTERNAL_ERROR` へ
    // 昇格する。
    private void finishMicRecord(long requestId, int code) {
        finishMicRecord(requestId, code, false);
    }

    // `recorderAlreadyStopped` は `MEDIA_RECORDER_INFO_MAX_DURATION_REACHED` 経路専用
    // （その時点で `MediaRecorder` は内部で停止済みのため `stop()` を呼ばない。
    // 呼ぶと `IllegalStateException` になり成功結果を内部エラーへ落としてしまう）。
    private void finishMicRecord(long requestId, int code, boolean recorderAlreadyStopped) {
        if (!micRecordRequest.complete(requestId)) {
            // 旧世代・二重完了（`take()` 済みの要求への遅延呼び出し）。
            return;
        }
        MediaRecorder recorder = micRecorder;
        File tempFile = micTempFile;
        long startedAt = micStartedAt;
        micRecorder = null;
        micTempFile = null;
        if (!recorderAlreadyStopped) {
            try {
                recorder.stop();
            } catch (RuntimeException e) {
                // 自動停止直後の `stop()` 例外等。0 バイト判定に委ねるため成功コードを
                // 維持しない。
                code = MIC_RESULT_INTERNAL_ERROR;
            }
        }
        recorder.release();

        byte[] bytes = null;
        String mime = null;
        long durationMs = 0;
        if (code == MIC_RESULT_SUCCESS) {
            try {
                bytes = readFileFully(tempFile);
                if (bytes.length == 0) {
                    code = MIC_RESULT_INTERNAL_ERROR;
                    bytes = null;
                } else {
                    mime = "audio/mp4";
                    durationMs = SystemClock.elapsedRealtime() - startedAt;
                }
            } catch (IOException e) {
                code = MIC_RESULT_INTERNAL_ERROR;
                bytes = null;
            }
        }
        deleteQuietly(tempFile);
        nativeOnMicRecordResult(requestId, code, bytes, mime, durationMs);
    }

    // `tempFile` を全読み込みする（`finishMicRecord` 専用）。`java.nio.file.Files` は
    // API 26 以降専用で minSdk 24/25 端末では `NoClassDefFoundError` になるため
    // （codex-review 指摘）、`FileInputStream` で読む。
    private static byte[] readFileFully(File file) throws IOException {
        long length = file.length();
        if (length < 0 || length > Integer.MAX_VALUE) {
            throw new IOException("読み込み対象のサイズが不正: " + length);
        }
        byte[] data = new byte[(int) length];
        try (FileInputStream in = new FileInputStream(file)) {
            int offset = 0;
            while (offset < data.length) {
                int read = in.read(data, offset, data.length - offset);
                if (read < 0) {
                    break;
                }
                offset += read;
            }
            if (offset != data.length) {
                throw new IOException("ファイルの読み込みが途中で終了: expected="
                        + data.length + " actual=" + offset);
            }
        }
        return data;
    }

    // `micTempFile` の後始末（`finishMicRecord`/`startMicRecorder` の失敗経路の両方から
    // 呼ぶ）。削除失敗はアプリ専有キャッシュ領域のため致命的ではなく、握りつぶす。
    private static void deleteQuietly(File file) {
        if (file != null) {
            file.delete();
        }
    }
}
