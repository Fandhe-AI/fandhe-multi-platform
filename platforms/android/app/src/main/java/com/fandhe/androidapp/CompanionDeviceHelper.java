package com.fandhe.androidapp;

import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothManager;
import android.companion.AssociationInfo;
import android.companion.AssociationRequest;
import android.companion.BluetoothLeDeviceFilter;
import android.companion.CompanionDeviceManager;
import android.content.Intent;
import android.content.IntentSender;
import android.content.pm.PackageManager;
import android.location.LocationManager;
import android.os.Build;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResult;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.IntentSenderRequest;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.annotation.RequiresApi;
import androidx.core.content.ContextCompat;
import androidx.core.location.LocationManagerCompat;
import androidx.lifecycle.Lifecycle;

// 付属機器のペアリング（Issue #741、親 #143 の b-android）の Android 実装ヘルパー。範囲は Android 13
// （API 33）以上で OS の機器選択画面を出し、BLE 機器を 1 台選ばせて名前を返すこと。名前を取ったら関連付けを
// 外す（関連付けを残すこと・機器との通信・ボンディングは範囲外）。MainActivity から生成・委譲される。
// Rust の `device/android_companion_device.rs` が `MainActivity.requestCompanionDevicePick` を JNI で呼び、
// 結果を `nativeOnCompanionDeviceResult` で受けて `companion_device::complete_from_os` へ渡す。
// 選択画面を起動する直前に `nativeTryBeginPresent` で Rust 側の期限切れ・差し替えと排他にする。
//
// 流れ: pick（JNI スレッド）→ start（UI スレッド。API・機能・位置情報サービス・Bluetooth を検査）→
// CompanionDeviceManager.associate → onAssociationPending で IntentSender を起動 →
// Activity の結果（RESULT_OK と AssociationInfo が揃った時点）→ disassociate → 結果を Rust へ。
// disassociate は Rust へ結果を送るより前に行う。外せなければ KIND_INTERNAL（成功にしない）。
//
// 結果コードの SSOT は Rust 側 `fandhe_device::companion_device` の契約で、KIND_* は
// `android_companion_device.rs` の定数と同じ値。期限（表示開始まで）は Rust の単調時計だけが持ち、
// ここには持たない。API 33 ではキャンセルと探索の期限切れを結果コードで区別できない場合があり、
// 区別できない結果は KIND_CANCELLED に倒す（嘘の成功ではない）。
//
// ライフサイクル: 状態はインスタンスフィールド。onPause は中断扱いにしない（選択画面を出すと自身が pause
// する）。onDestroy で保留中の要求を KIND_INTERRUPTED で確定し、記録済みの関連付けを外す。再生成後の新
// インスタンスは NONE から始まり、Rust 側 reset_pending_tracking() と対で旧要求を捨てる。旧インスタンス
// 宛の結果が新インスタンスのランチャーへ届いても、起動を記録した要求が無いため関連付けを外して捨て、
// 新しい要求の id は消費しない。
// 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止・旧要求の関連付けの解除。
// 守らない範囲: API 32 以下・関連付けを残すこと・背景での起動・機器との通信とボンディング・
// 選択画面の表示中のプロセス死・複数プロセス。
// ロック順: this のロックを持ったまま nativeTryBeginPresent / nativeOnCompanionDeviceResult を呼ぶ。
// Rust 側は自分のロックの内側から Java を呼ばない（begin() でロックを放してから pick を呼ぶ）ため
// 循環しない。CDM のコールバックは main executor で受け、Activity の結果と同じ UI スレッドに直列化する。
// 機器名・onFailure の文言・例外メッセージはログに出さない。
final class CompanionDeviceHelper {
    static final int KIND_SELECTED = 0;
    static final int KIND_CANCELLED = 1;
    static final int KIND_UNSUPPORTED = 2;
    static final int KIND_LOCATION_OFF = 3;
    static final int KIND_BLUETOOTH_OFF = 4;
    static final int KIND_NOT_FOUND = 5;
    static final int KIND_NOT_FOREGROUND = 6;
    static final int KIND_INTERRUPTED = 7;
    static final int KIND_INTERNAL = 8;

    private static native boolean nativeTryBeginPresent(long requestId);

    private static native void nativeOnCompanionDeviceResult(long requestId, int kind, String name);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity のフィールド初期化子で生成する。
    private final ActivityResultLauncher<IntentSenderRequest> launcher;

    // 以下は this のロックで守る。
    // 選択画面の起動を OS に渡した要求。
    private long launchedId = RequestTracker.NONE;
    // RESULT_OK を受けたが AssociationInfo がまだ届いていない要求。
    private long okWaitingId = RequestTracker.NONE;
    // onAssociationCreated で届いた現在の要求の関連付け（AssociationInfo。API 33 未満では常に null）。
    private Object pendingInfo;
    private boolean destroyed;

    CompanionDeviceHelper(ComponentActivity activity) {
        this.activity = activity;
        this.launcher = activity.registerForActivityResult(
                new ActivityResultContracts.StartIntentSenderForResult(), this::onActivityResult);
    }

    // JNI スレッドから呼ばれる。
    void pick(long id) {
        synchronized (this) {
            request.track(id);
            launchedId = RequestTracker.NONE;
            okWaitingId = RequestTracker.NONE;
            dropPendingInfo();
        }
        activity.runOnUiThread(() -> start(id));
    }

    private synchronized void start(long id) {
        // 差し替え済み・確定済みの旧要求は始めない。
        if (request.current() != id || destroyed) {
            return;
        }
        if (activity.isFinishing() || activity.isDestroyed()) {
            finish(id, KIND_INTERRUPTED, null);
            return;
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            finish(id, KIND_UNSUPPORTED, null);
            return;
        }
        PackageManager pm = activity.getPackageManager();
        if (!pm.hasSystemFeature(PackageManager.FEATURE_COMPANION_DEVICE_SETUP)
                || !pm.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)) {
            finish(id, KIND_UNSUPPORTED, null);
            return;
        }
        // 背景で選択画面を出さない。
        if (!activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            finish(id, KIND_NOT_FOREGROUND, null);
            return;
        }
        startApi33(id);
    }

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private void startApi33(long id) {
        CompanionDeviceManager cdm = activity.getSystemService(CompanionDeviceManager.class);
        if (cdm == null) {
            finish(id, KIND_UNSUPPORTED, null);
            return;
        }
        // 位置情報サービスがオフだと探索できない（権限は不要）。取得できないときもオフ扱い。
        LocationManager lm = activity.getSystemService(LocationManager.class);
        if (lm == null || !LocationManagerCompat.isLocationEnabled(lm)) {
            finish(id, KIND_LOCATION_OFF, null);
            return;
        }
        BluetoothManager bm = activity.getSystemService(BluetoothManager.class);
        BluetoothAdapter adapter = bm == null ? null : bm.getAdapter();
        if (adapter == null) {
            finish(id, KIND_UNSUPPORTED, null);
            return;
        }
        try {
            if (!adapter.isEnabled()) {
                finish(id, KIND_BLUETOOTH_OFF, null);
                return;
            }
        } catch (SecurityException e) {
            // 判定できないときは OS の結果に任せる（成功へは倒れない）。
        }
        try {
            AssociationRequest req = new AssociationRequest.Builder()
                    .addDeviceFilter(new BluetoothLeDeviceFilter.Builder().build())
                    .setSingleDevice(true)
                    .build();
            cdm.associate(req, ContextCompat.getMainExecutor(activity), new Cb(id));
        } catch (RuntimeException e) {
            finish(id, KIND_INTERNAL, null);
        }
    }

    // CDM のコールバック。要求ごとに id を持ち、現在の要求でなければ関連付けを外して捨てる。
    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private final class Cb extends CompanionDeviceManager.Callback {
        private final long id;

        Cb(long id) {
            this.id = id;
        }

        @Override
        public void onAssociationPending(IntentSender sender) {
            onPending(id, sender);
        }

        @Override
        public void onAssociationCreated(AssociationInfo info) {
            onCreated(id, info);
        }

        @Override
        public void onFailure(CharSequence error) {
            // 文言は読まない・ログに出さない。
            onFailed(id);
        }
    }

    private synchronized void onPending(long id, IntentSender sender) {
        if (request.current() != id || destroyed) {
            return;
        }
        if (!activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            finish(id, KIND_NOT_FOREGROUND, null);
            return;
        }
        // false は Rust 側が期限切れを確定済み、または古い要求。結果は送らず追跡だけ外す。
        if (!nativeTryBeginPresent(id)) {
            request.complete(id);
            return;
        }
        try {
            launcher.launch(new IntentSenderRequest.Builder(sender).build());
            launchedId = id;
        } catch (RuntimeException e) {
            finish(id, KIND_INTERNAL, null);
        }
    }

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private synchronized void onCreated(long id, AssociationInfo info) {
        if (request.current() != id || destroyed) {
            // 古い要求・破棄後に作られた関連付けは外して捨てる。
            disassociate(info);
            return;
        }
        if (okWaitingId == id) {
            completeSelected(id, info);
        } else {
            pendingInfo = info;
        }
    }

    private synchronized void onFailed(long id) {
        if (request.current() != id || destroyed) {
            return;
        }
        // 選択画面の表示後は Activity の結果を正とする。表示前の失敗は成功にせず Internal。
        if (launchedId != id) {
            finish(id, KIND_INTERNAL, null);
        }
    }

    // Activity の結果（UI スレッド）。
    private synchronized void onActivityResult(ActivityResult result) {
        long id = launchedId;
        Intent data = result.getData();
        Object info = extractInfo(data);
        if (id == RequestTracker.NONE || request.current() != id) {
            // 起動を記録した要求が無い（作り直し後の旧結果など）。関連付けがあれば外して捨てる。
            disassociateObject(info);
            return;
        }
        int code = result.getResultCode();
        if (code == android.app.Activity.RESULT_OK) {
            if (info == null) {
                info = pendingInfo;
            }
            if (info == null) {
                // AssociationInfo が揃うまで確定しない（onAssociationCreated で続きを行う）。
                okWaitingId = id;
                return;
            }
            completeSelected(id, info);
            return;
        }
        // キャンセル・拒否・探索の期限切れ・内部エラー。記録済みの関連付けは先に外す。
        if (info == null) {
            info = pendingInfo;
        }
        boolean released = info == null || disassociateObject(info);
        finish(id, released ? kindForFailedResult(code) : KIND_INTERNAL, null);
    }

    private static int kindForFailedResult(int code) {
        if (code == android.app.Activity.RESULT_CANCELED) {
            return KIND_CANCELLED;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            if (code == CompanionDeviceManager.RESULT_USER_REJECTED) {
                return KIND_CANCELLED;
            }
            if (code == CompanionDeviceManager.RESULT_DISCOVERY_TIMEOUT) {
                return KIND_NOT_FOUND;
            }
        }
        // 内部エラーと未知のコードは成功へ倒さない。
        return KIND_INTERNAL;
    }

    private Object extractInfo(Intent data) {
        if (data == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            return null;
        }
        return extractInfoApi33(data);
    }

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private static Object extractInfoApi33(Intent data) {
        try {
            return data.getParcelableExtra(CompanionDeviceManager.EXTRA_ASSOCIATION, AssociationInfo.class);
        } catch (RuntimeException e) {
            return null;
        }
    }

    // 選んだ結果の確定。関連付けを外し終えてから成功を返す。
    private void completeSelected(long id, Object info) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            finish(id, KIND_INTERNAL, null);
            return;
        }
        completeSelectedApi33(id, (AssociationInfo) info);
    }

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private void completeSelectedApi33(long id, AssociationInfo info) {
        if (pendingInfo == info) {
            pendingInfo = null;
        }
        String name = null;
        try {
            CharSequence display = info.getDisplayName();
            name = display == null ? null : display.toString();
        } catch (RuntimeException e) {
            name = null;
        }
        if (disassociate(info)) {
            finish(id, KIND_SELECTED, name);
        } else {
            finish(id, KIND_INTERNAL, null);
        }
    }

    private boolean disassociateObject(Object info) {
        if (info == null) {
            return true;
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            return false;
        }
        return disassociate((AssociationInfo) info);
    }

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private boolean disassociate(AssociationInfo info) {
        try {
            CompanionDeviceManager cdm = activity.getSystemService(CompanionDeviceManager.class);
            if (cdm == null) {
                return false;
            }
            cdm.disassociate(info.getId());
            return true;
        } catch (RuntimeException e) {
            return false;
        }
    }

    // 記録済みの関連付けを外して捨てる。
    private void dropPendingInfo() {
        Object info = pendingInfo;
        pendingInfo = null;
        disassociateObject(info);
    }

    // first-wins: 現在の要求だけ確定して Rust へ返す。名前は KIND_SELECTED のときだけ渡す。
    private void finish(long id, int kind, String name) {
        if (request.complete(id)) {
            launchedId = RequestTracker.NONE;
            okWaitingId = RequestTracker.NONE;
            // 確定時に残った関連付けを外す（成功経路の分は completeSelected が外して null にしてある）。
            dropPendingInfo();
            nativeOnCompanionDeviceResult(id, kind, kind == KIND_SELECTED ? name : null);
        }
    }

    synchronized void onDestroy() {
        destroyed = true;
        long id = request.current();
        if (id != RequestTracker.NONE) {
            finish(id, KIND_INTERRUPTED, null);
        } else {
            dropPendingInfo();
        }
    }
}
