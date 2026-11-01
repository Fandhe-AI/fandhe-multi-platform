package com.fandhe.androidapp;

import android.app.Activity;
import android.content.Intent;
import android.os.Handler;
import android.os.Looper;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResult;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.IntentSenderRequest;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;
import androidx.lifecycle.Lifecycle;

import com.google.android.gms.common.api.ResolvableApiException;
import com.google.android.gms.tasks.Task;
import com.google.android.gms.wallet.IsReadyToPayRequest;
import com.google.android.gms.wallet.PaymentData;
import com.google.android.gms.wallet.PaymentDataRequest;
import com.google.android.gms.wallet.PaymentsClient;
import com.google.android.gms.wallet.Wallet;
import com.google.android.gms.wallet.WalletConstants;

import java.lang.ref.WeakReference;

// 物・サービスの決済（Issue #1131、親 #114 の b-android）の Android 実装ヘルパー。範囲は Google Pay API の
// TEST 環境で「使えるか」の確認と固定 1 円の決済シートを出し、結果の種別だけを Rust へ返すこと。
// MainActivity から生成・委譲される。Rust の `device/android_payment_request.rs` が
// `MainActivity.requestPayment(id, pay, isReadyJson, paymentDataJson)` を JNI で呼び、結果を
// `nativeOnCheckResult` / `nativeOnPayResult` で受けて `payment_request::bridge::deliver_*` へ渡す。
//
// 要求 JSON は Rust の `fandhe_device::payment_request::ANDROID_*_JSON` を引数で受ける（ここに複製しない）。
// 結果コードの解釈（RESULT_OK かつデータありだけが承認）は Rust の `outcome_from_android_result`（SSOT）が
// 行い、ここでは (resultCode, hasPaymentData) を取り出すだけ。KIND_* は `android_payment_request.rs` の
// 定数と同じ値（SSOT はこの Java 側）。
//
// 環境は ENVIRONMENT_TEST に固定する。PRODUCTION へ切り替える分岐・設定は作らない。返った PaymentData は
// 有無の判定にだけ使い、toJson() を呼ばず、保持せず、ログにも出さない（課金できるトークンを扱わない）。
//
// ライフサイクル: Activity は WeakReference で持つ。onPause / onStop では取り消さない（決済シートが自身を
// pause させる）。onDestroy で追跡を外し結果は送らない（trait doc の「破棄」）。再生成後の新インスタンスは
// NONE から始まり、Rust 側 reset_pending_tracking() と対で旧要求を捨てる。旧結果が新ランチャーへ再配送されても
// 起動を記録した要求が無いため捨て、新しい要求の id は消費しない。
// 守る範囲: 1 プロセス内の要求と完了の順序・嘘の成功の禁止。守らない範囲: プロセス死をまたぐ結果・
// GMS の Task が返らない場合・複数プロセス。期限は掛けない（利用者操作の UI）。
// ロック順: this のロック内で完結し、他のロックを取らない。Rust 側は自分のロックの内側から Java を呼ばない。
final class PaymentRequestHelper {
    static final int KIND_READY = 0;
    static final int KIND_NOT_READY = 1;
    static final int KIND_ACTIVITY_RESULT = 2;
    static final int KIND_UNSUPPORTED = 3;
    static final int KIND_FAILED = 4;

    private static native void nativeOnCheckResult(long requestId, int kind);

    private static native void nativeOnPayResult(
            long requestId, int kind, int resultCode, boolean hasPaymentData);

    private final WeakReference<ComponentActivity> activityRef;
    private final RequestTracker request = new RequestTracker();
    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity のフィールド初期化子で生成する。
    private final ActivityResultLauncher<IntentSenderRequest> launcher;
    private final Handler uiHandler = new Handler(Looper.getMainLooper());

    // 以下は this のロックで守る。
    private long launchedId = RequestTracker.NONE;
    private boolean pay;
    private String paymentDataJson;
    private boolean destroyed;

    PaymentRequestHelper(ComponentActivity activity) {
        this.activityRef = new WeakReference<>(activity);
        this.launcher = activity.registerForActivityResult(
                new ActivityResultContracts.StartIntentSenderForResult(), this::onActivityResult);
    }

    // JNI スレッドから呼ばれる。pay が false のとき paymentDataJson は null。
    void request(long id, boolean pay, String isReadyJson, String paymentDataJson) {
        synchronized (this) {
            request.track(id);
            launchedId = RequestTracker.NONE;
            this.pay = pay;
            this.paymentDataJson = paymentDataJson;
        }
        uiHandler.post(() -> start(id, isReadyJson));
    }

    private synchronized void start(long id, String isReadyJson) {
        if (request.current() != id || destroyed) {
            return;
        }
        ComponentActivity activity = activityRef.get();
        if (activity == null || activity.isFinishing() || activity.isDestroyed()) {
            finish(id, KIND_FAILED, 0, false);
            return;
        }
        try {
            // SUCCESS 以外（無い・古い・無効）は非対応（GmsAvailability の doc 参照）。
            if (!GmsAvailability.isAvailable(activity)) {
                finish(id, KIND_UNSUPPORTED, 0, false);
                return;
            }
            PaymentsClient client = Wallet.getPaymentsClient(
                    activity,
                    new Wallet.WalletOptions.Builder()
                            .setEnvironment(WalletConstants.ENVIRONMENT_TEST)
                            .build());
            client.isReadyToPay(IsReadyToPayRequest.fromJson(isReadyJson))
                    .addOnCompleteListener(ContextCompat.getMainExecutor(activity),
                            task -> onReadyToPay(id, client, task));
        } catch (RuntimeException e) {
            finish(id, KIND_FAILED, 0, false);
        }
    }

    // Task のコールバック（UI スレッド）。再開後に現在の要求かを照合し直す。
    private synchronized void onReadyToPay(long id, PaymentsClient client, Task<Boolean> task) {
        if (request.current() != id || destroyed) {
            return;
        }
        // Task 失敗は「使えない」に畳まず失敗にする。
        if (!task.isSuccessful()) {
            finish(id, KIND_FAILED, 0, false);
            return;
        }
        boolean ready = Boolean.TRUE.equals(task.getResult());
        if (!pay) {
            finish(id, ready ? KIND_READY : KIND_NOT_READY, 0, false);
            return;
        }
        if (!ready) {
            finish(id, KIND_UNSUPPORTED, 0, false);
            return;
        }
        ComponentActivity activity = activityRef.get();
        if (activity == null || activity.isFinishing() || activity.isDestroyed()) {
            finish(id, KIND_FAILED, 0, false);
            return;
        }
        // 背景で決済シートを出さない。
        if (!activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            finish(id, KIND_FAILED, 0, false);
            return;
        }
        try {
            client.loadPaymentData(PaymentDataRequest.fromJson(paymentDataJson))
                    .addOnCompleteListener(ContextCompat.getMainExecutor(activity),
                            t -> onPaymentData(id, t));
        } catch (RuntimeException e) {
            finish(id, KIND_FAILED, 0, false);
        }
    }

    private synchronized void onPaymentData(long id, Task<PaymentData> task) {
        if (request.current() != id || destroyed) {
            return;
        }
        if (task.isSuccessful()) {
            // 決済シートを挟まず返った場合。中身は読まず有無だけ。
            finish(id, KIND_ACTIVITY_RESULT, Activity.RESULT_OK, task.getResult() != null);
            return;
        }
        Exception e = task.getException();
        if (!(e instanceof ResolvableApiException)) {
            finish(id, KIND_FAILED, 0, false);
            return;
        }
        ComponentActivity activity = activityRef.get();
        if (activity == null || activity.isFinishing() || activity.isDestroyed()) {
            finish(id, KIND_FAILED, 0, false);
            return;
        }
        // 待機中にホームへ移動した場合に背景で決済シートを出さない。
        if (!activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            finish(id, KIND_FAILED, 0, false);
            return;
        }
        try {
            launcher.launch(new IntentSenderRequest.Builder(
                    ((ResolvableApiException) e).getResolution()).build());
            launchedId = id;
        } catch (RuntimeException ex) {
            finish(id, KIND_FAILED, 0, false);
        }
    }

    // 決済シートの結果（UI スレッド）。コードの解釈は Rust 側。
    private synchronized void onActivityResult(ActivityResult result) {
        long id = launchedId;
        if (id == RequestTracker.NONE || request.current() != id) {
            // 起動を記録した要求が無い（作り直し後の旧結果など）。データは読まず捨てる。
            return;
        }
        launchedId = RequestTracker.NONE;
        boolean hasData = false;
        Intent data = result.getData();
        // 起動失敗（SendIntentException）は RESULT_CANCELED で返るため、取り消しと区別して失敗にする。
        if (data != null && data.hasExtra(
                ActivityResultContracts.StartIntentSenderForResult.EXTRA_SEND_INTENT_EXCEPTION)) {
            finish(id, KIND_FAILED, 0, false);
            return;
        }
        if (data != null) {
            try {
                hasData = PaymentData.getFromIntent(data) != null;
            } catch (RuntimeException e) {
                hasData = false;
            }
        }
        finish(id, KIND_ACTIVITY_RESULT, result.getResultCode(), hasData);
    }

    // first-wins: complete に成功したときだけ Rust へ送る。
    private void finish(long id, int kind, int resultCode, boolean hasData) {
        if (!request.complete(id)) {
            return;
        }
        if (pay) {
            nativeOnPayResult(id, kind, resultCode, hasData);
        } else {
            nativeOnCheckResult(id, kind);
        }
    }

    // Activity 破棄。結果は送らず追跡を外す（Rust 側が slot を捨てる）。
    synchronized void onDestroy() {
        destroyed = true;
        request.take();
        launchedId = RequestTracker.NONE;
        paymentDataJson = null;
    }
}
