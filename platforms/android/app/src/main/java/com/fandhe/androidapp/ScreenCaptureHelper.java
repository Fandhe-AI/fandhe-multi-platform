package com.fandhe.androidapp;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.media.projection.MediaProjectionManager;
import android.os.PowerManager;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResult;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;

// 画面録画（Issue #1337）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_screen_capture.rs` が `MainActivity.requestScreenCaptureStart` /
// `requestScreenCaptureStop` を JNI で呼び、結果を `nativeOnScreenCaptureResult` で受ける）。
// 録画の実体は `ScreenCaptureService`（フォアグラウンドサービス）で、本クラスは
// 同意ダイアログの起動・結果の振り分け・ライフサイクル（onPause / onDestroy）を担う。
//
// 状態遷移はすべて UI スレッド（main looper）で直列化する:
//   同意待ち（consentId）→ サービス起動待ち（STARTING）→ 録画中（ScreenCaptureService.active）。
//
// ライフサイクル: 同意ダイアログ表示中の onPause は破棄しない（ダイアログ自体が起こす）。
// 起動待ち・録画中の onPause と onDestroy は RESULT_INTERRUPTED で確定する（背景で録画を
// 始めない）。`consentId` はインスタンスフィールドなので、Activity 再生成後の新インスタンスは
// NONE から始まり、旧同意結果が再配送されても捨てる。id は Rust 側で単調増加のため、
// 旧結果が新しい要求を消費することはない。
final class ScreenCaptureHelper {
    // 結果コード。SSOT は Rust 側 `device/screen_capture.rs::error_from_code`。
    // RESULT_OK は ScreenCaptureService 内では「成功を希望」の意味にも使う。
    static final int RESULT_OK = 0;
    static final int RESULT_FAILED = 2;
    static final int RESULT_BUSY = 3;
    static final int RESULT_CANCELLED = 4;
    static final int RESULT_INTERRUPTED = 5;
    static final int RESULT_UNSUPPORTED = 6;

    // 同意済みで ScreenCaptureService の起動を待っている要求（UI スレッド専用）。
    static final RequestTracker STARTING = new RequestTracker();

    static native void nativeOnScreenCaptureStarted(long requestId);

    private static native void nativeOnScreenCaptureResult(
            long requestId, int code, long sizeBytes, long durationMs);

    // 結果を Rust へ返す。UI スレッドから呼ぶ。
    static void report(long requestId, int code, long sizeBytes, long durationMs) {
        nativeOnScreenCaptureResult(requestId, code, sizeBytes, durationMs);
    }

    private final ComponentActivity activity;
    private final ActivityResultLauncher<Intent> consentLauncher;
    private long consentId = RequestTracker.NONE;
    // 同意ダイアログが未終了の間 true。stop で consentId を NONE にしても結果を消費するまで
    // 残し、旧ダイアログの結果が次の要求に取り違えられないよう再開始を busy にする。
    private boolean dialogOpen;
    private int pendingMaxMs;

    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity の
    // フィールド初期化子から本クラスを生成してここで登録する。
    ScreenCaptureHelper(ComponentActivity activity) {
        this.activity = activity;
        this.consentLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.StartActivityForResult(), this::onConsent);
    }

    // JNI スレッドから呼ばれる。
    void start(long id, int maxMs) {
        activity.runOnUiThread(() -> doStart(id, maxMs));
    }

    // JNI スレッドから呼ばれる。
    void stop(long id) {
        activity.runOnUiThread(() -> doStop(id));
    }

    // MainActivity.onPause から UI スレッドで呼ぶ。同意ダイアログ待ち（consentId のみ）は残す。
    void onPause() {
        long starting = STARTING.take();
        if (starting != RequestTracker.NONE) {
            report(starting, RESULT_INTERRUPTED, 0, 0);
        }
        // 画面消灯・ロックによる onPause は中断にせず、OS が止める onStop 経路（得られた分を
        // 成功で返す）へ委ねる。画面が点灯したままの背景遷移は従来どおり中断。
        PowerManager pm = (PowerManager) activity.getSystemService(Context.POWER_SERVICE);
        if (pm == null || pm.isInteractive()) {
            ScreenCaptureService.abortActive();
        }
    }

    // MainActivity.onDestroy から UI スレッドで呼ぶ。
    void onDestroy() {
        if (consentId != RequestTracker.NONE) {
            long id = consentId;
            consentId = RequestTracker.NONE;
            report(id, RESULT_INTERRUPTED, 0, 0);
        }
        onPause();
        // Activity 破棄は画面状態に関わらず中断。
        ScreenCaptureService.abortActive();
    }

    private void doStart(long id, int maxMs) {
        if (activity.isFinishing() || activity.isDestroyed()) {
            report(id, RESULT_INTERRUPTED, 0, 0);
            return;
        }
        if (dialogOpen || consentId != RequestTracker.NONE || STARTING.current() != RequestTracker.NONE) {
            report(id, RESULT_BUSY, 0, 0);
            return;
        }
        MediaProjectionManager mpm = (MediaProjectionManager)
                activity.getSystemService(Context.MEDIA_PROJECTION_SERVICE);
        if (mpm == null) {
            report(id, RESULT_UNSUPPORTED, 0, 0);
            return;
        }
        consentId = id;
        dialogOpen = true;
        pendingMaxMs = maxMs;
        try {
            consentLauncher.launch(mpm.createScreenCaptureIntent());
        } catch (ActivityNotFoundException e) {
            dialogOpen = false;
            consentId = RequestTracker.NONE;
            report(id, RESULT_UNSUPPORTED, 0, 0);
        } catch (RuntimeException e) {
            dialogOpen = false;
            consentId = RequestTracker.NONE;
            report(id, RESULT_FAILED, 0, 0);
        }
    }

    private void doStop(long id) {
        if (consentId == id) {
            // 同意ダイアログは残るが、結果は consentId == NONE で捨てる。
            consentId = RequestTracker.NONE;
            report(id, RESULT_CANCELLED, 0, 0);
        } else if (STARTING.complete(id)) {
            // 録画開始前の停止はキャンセル扱い。
            report(id, RESULT_CANCELLED, 0, 0);
        } else {
            ScreenCaptureService.stopActive(id);
        }
    }

    private void onConsent(ActivityResult result) {
        long id = consentId;
        consentId = RequestTracker.NONE;
        dialogOpen = false;
        if (id == RequestTracker.NONE) {
            return;
        }
        Intent data = result.getData();
        if (result.getResultCode() != Activity.RESULT_OK || data == null) {
            report(id, RESULT_CANCELLED, 0, 0);
            return;
        }
        STARTING.track(id);
        try {
            ContextCompat.startForegroundService(activity,
                    new Intent(activity, ScreenCaptureService.class)
                            .putExtra(ScreenCaptureService.EXTRA_ID, id)
                            .putExtra(ScreenCaptureService.EXTRA_MAX_MS, pendingMaxMs)
                            .putExtra(ScreenCaptureService.EXTRA_RESULT_CODE, result.getResultCode())
                            .putExtra(ScreenCaptureService.EXTRA_DATA, data));
        } catch (RuntimeException e) {
            if (STARTING.complete(id)) {
                report(id, RESULT_FAILED, 0, 0);
            }
        }
    }
}
