package com.fandhe.androidapp;

import android.Manifest;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothClass;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothManager;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.location.LocationManager;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;
import androidx.core.location.LocationManagerCompat;
import androidx.lifecycle.Lifecycle;

import java.util.Map;
import java.util.Set;

// Bluetooth Classic（Issue #661、親 #130 の b-android）の Android 実装ヘルパー。範囲はペアリング済み機器の
// 一覧（同期 1 回）と、固定窓の検索 1 回のみ。接続・ペアリング・Bluetooth を ON にする操作・アドレスの表示は範囲外。
// MainActivity から生成・委譲される。Rust の `device/android_bluetooth_classic.rs` が
// `MainActivity.bluetoothClassicPaired`（一覧）と `MainActivity.requestBluetoothClassic`（検索）を JNI で呼び、
// 検索の結果は native 2 本で返す。
//
// 並行性: 検索の状態を触る経路は main looper 1 本（`request` は post、BroadcastReceiver も main Handler で
// 受ける）。ロックは持たない。完了は RequestTracker の first-wins で 1 回だけ通知する。窓は main Handler の
// closer だけが閉じる。`ACTION_DISCOVERY_FINISHED` は完了の判定に使わない（置き換え時の旧 cancel の
// FINISHED が新しい要求に混ざるため）。一覧は ApplicationContext だけを使う同期メソッドで保留状態を持たない。
//
// 期限: 窓の長さ(durationMs)は Rust 側 DISCOVERY_WINDOW_MS をそのまま受け取る。JNI を受けた時点
// （post より前）の uptimeMillis を起点にし、UI キューと許可ダイアログの待ちも窓に含める（ダイアログで
// 窓を過ぎたら、許可しても失敗で確定する。守らない範囲として対応表の note に書く）。
//
// ライフサイクル: onPause / onDestroy で保留中の要求をコード 4（中断）で確定し、cancelDiscovery・
// レシーバー解除・タイマー解放を行う（許可ダイアログが起こす onPause は中断扱いにしない）。非前景での
// 開始要求も 4。許可の結果が背景で届いたら onResume まで開始を保留する。再生成後の新インスタンスは
// 空の tracker から始まり、旧 id の完了は Rust 側 reset_pending_tracking と id 照合で捨てられる。
//
// レシーバーは RECEIVER_EXPORTED で登録する。Bluetooth のようなシステムが送る broadcast は
// RECEIVER_NOT_EXPORTED では届かないため（Android 公式 broadcasts ガイド
// https://developer.android.com/develop/background-work/background-tasks/broadcasts ）。
// ACTION_FOUND は protected broadcast で他アプリからは送れないが、受信側でも action と extra の型を検証する。
//
// 結果コードの SSOT は Rust の `fandhe_device::bluetooth_classic::decode_code`。ここは値を合わせるだけで
// 写像の分岐をしない（4・5・6 は Rust 側で Internal）。機器名・アドレスはログへ出さない。
// 守らない範囲: OS や他アプリが inquiry を早く終えた・間引いた場合の結果の違い、API 31 以上で許可を得る前の
// 一覧が「許可されていません」になること、neverForLocation が検索結果へ与える影響（#650 で実機確認）。
final class BluetoothClassicHelper {
    private static final int CODE_OK = 0;
    private static final int CODE_PERMISSION = 1;
    private static final int CODE_DISABLED = 2;
    private static final int CODE_UNSUPPORTED = 3;
    private static final int CODE_INTERRUPTED = 4;
    private static final int CODE_LOCATION_OFF = 5;
    private static final int CODE_OTHER = 6;

    // ponytail: 応答の洪水対策の上限。超えた分は捨てる。上限に当たる環境が出たら入れ替え方を検討する。
    private static final int MAX_FORWARDED = 256;
    private static final int MAX_PAIRED = 256;
    private static final char REC = '\u001E';
    private static final char SEP = '\u001F';

    private static native void nativeOnClassicFound(
            long id, String address, String name, int cod, int rssi, boolean paired);

    private static native void nativeOnClassicFinished(long id, int code);

    private final ComponentActivity activity;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final RequestTracker request = new RequestTracker();
    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity のフィールド初期化子で生成する。
    private final ActivityResultLauncher<String[]> permissionLauncher;

    // 以下は main looper 専用。
    private boolean permissionPending;
    private boolean startPendingResume;
    private long deadline;
    private Runnable closer;
    // startDiscovery を自分が成功させたか。自分が始めていない他アプリの検索は止めない。
    private boolean launched;
    private int forwarded;
    private BroadcastReceiver receiver;

    BluetoothClassicHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.RequestMultiplePermissions(), this::onPermissionResult);
    }

    private static boolean isLe(BluetoothDevice d) {
        try {
            return d.getType() == BluetoothDevice.DEVICE_TYPE_LE;
        } catch (SecurityException e) {
            // 種別不明は残す。
            return false;
        }
    }

    private static BluetoothAdapter adapterOf(Context ctx) {
        BluetoothManager m = (BluetoothManager) ctx.getSystemService(Context.BLUETOOTH_SERVICE);
        return m == null ? null : m.getAdapter();
    }

    private static boolean hasFeature(Context ctx) {
        return ctx.getPackageManager().hasSystemFeature(PackageManager.FEATURE_BLUETOOTH);
    }

    private static String clean(String s) {
        if (s == null) {
            return "";
        }
        return s.replace(String.valueOf(REC), "").replace(String.valueOf(SEP), "");
    }

    // egui スレッドから同期で呼ばれる。形は「コード」単独（失敗）か、
    // 「0 REC 件数 (REC address SEP name SEP cod)*」。null は内部失敗。
    static String paired(Context appContext) {
        if (!hasFeature(appContext)) {
            return String.valueOf(CODE_UNSUPPORTED);
        }
        BluetoothAdapter adapter = adapterOf(appContext);
        if (adapter == null) {
            return String.valueOf(CODE_UNSUPPORTED);
        }
        // 同期経路ではダイアログを出せない。許可は検索ボタンの経路で求める。
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
                && ContextCompat.checkSelfPermission(appContext, Manifest.permission.BLUETOOTH_CONNECT)
                        != PackageManager.PERMISSION_GRANTED) {
            return String.valueOf(CODE_PERMISSION);
        }
        try {
            // オフのとき getBondedDevices は空を返すため、空の成功にせず先に判定する。
            if (!adapter.isEnabled()) {
                return String.valueOf(CODE_DISABLED);
            }
        } catch (SecurityException e) {
            return String.valueOf(CODE_PERMISSION);
        }
        Set<BluetoothDevice> bonded;
        try {
            bonded = adapter.getBondedDevices();
        } catch (SecurityException e) {
            return String.valueOf(CODE_PERMISSION);
        } catch (RuntimeException e) {
            return null;
        }
        if (bonded == null) {
            return null;
        }
        StringBuilder body = new StringBuilder();
        int count = 0;
        for (BluetoothDevice d : bonded) {
            if (count >= MAX_PAIRED) {
                break;
            }
            try {
                if (isLe(d)) {
                    continue;
                }
                String address = d.getAddress();
                if (address == null || address.isEmpty()) {
                    continue;
                }
                String name = d.getName();
                BluetoothClass bc = d.getBluetoothClass();
                int cod = bc == null ? -1 : bc.getDeviceClass();
                body.append(REC).append(clean(address)).append(SEP).append(clean(name))
                        .append(SEP).append(cod);
                count++;
            } catch (SecurityException e) {
                return String.valueOf(CODE_PERMISSION);
            } catch (RuntimeException e) {
                return null;
            }
        }
        return String.valueOf(CODE_OK) + REC + count + body;
    }

    // JNI スレッドから呼ばれる。
    void request(final long id, int durationMs) {
        final long dl = SystemClock.uptimeMillis() + durationMs;
        handler.post(() -> start(id, dl));
    }

    void onPause() {
        if (!permissionPending) {
            interrupt();
        }
    }

    void onResume() {
        if (startPendingResume) {
            startPendingResume = false;
            long id = request.current();
            if (id != RequestTracker.NONE) {
                begin(id);
            }
        }
    }

    void onDestroy() {
        permissionPending = false;
        interrupt();
    }

    private void interrupt() {
        startPendingResume = false;
        long id = request.take();
        stopQuietly();
        if (id != RequestTracker.NONE) {
            nativeOnClassicFinished(id, CODE_INTERRUPTED);
        }
    }

    // 資源の解放と状態の確定を通知より先に行う。旧 id の遅延通知は照合で捨てる。
    private void finish(long id, int code) {
        if (request.complete(id)) {
            stopQuietly();
            nativeOnClassicFinished(id, code);
        }
    }

    private boolean foreground() {
        return !activity.isFinishing() && !activity.isDestroyed()
                && activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED);
    }

    private static String[] permissions() {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
                ? new String[] {
                    Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT
                }
                : new String[] {Manifest.permission.ACCESS_FINE_LOCATION};
    }

    private boolean permitted() {
        for (String p : permissions()) {
            if (ContextCompat.checkSelfPermission(activity, p) != PackageManager.PERMISSION_GRANTED) {
                return false;
            }
        }
        return true;
    }

    private boolean enabled(BluetoothAdapter adapter) {
        try {
            return adapter.isEnabled();
        } catch (SecurityException e) {
            return false;
        }
    }

    // API 30 以下は位置情報がオフだと検索結果が届かない。
    private boolean locationOff() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            return false;
        }
        LocationManager lm = (LocationManager) activity.getSystemService(Context.LOCATION_SERVICE);
        return lm == null || !LocationManagerCompat.isLocationEnabled(lm);
    }

    private void start(long id, long dl) {
        // 再要求は置き換え。旧 id は通知せず捨てる（Rust 側は新しい世代へ移っている）。
        request.take();
        stopQuietly();
        startPendingResume = false;
        deadline = dl;
        // 許可ダイアログ表示中の再要求は id を追跡し直すだけ（結果は最新 id で処理する）。
        if (permissionPending) {
            request.track(id);
            scheduleCloser(id);
            return;
        }
        // 非搭載判定を許可確認より先に行い、非対応端末でダイアログを出さない。
        BluetoothAdapter adapter = hasFeature(activity) ? adapterOf(activity) : null;
        if (adapter == null) {
            nativeOnClassicFinished(id, CODE_UNSUPPORTED);
            return;
        }
        if (!foreground()) {
            nativeOnClassicFinished(id, CODE_INTERRUPTED);
            return;
        }
        // 許可前の isEnabled は API 31 以上で SecurityException になりうるため、許可済みのときだけ判定する。
        if (permitted() && !enabled(adapter)) {
            nativeOnClassicFinished(id, CODE_DISABLED);
            return;
        }
        request.track(id);
        // 期限は許可待ちの間も進める。過ぎていれば即時に走る。
        scheduleCloser(id);
        if (!permitted()) {
            permissionPending = true;
            try {
                permissionLauncher.launch(permissions());
            } catch (RuntimeException e) {
                permissionPending = false;
                finish(id, CODE_OTHER);
            }
            return;
        }
        begin(id);
    }

    // 窓が閉じた。開始できていなければ 6、オフなら 2、位置情報オフ（API 30 以下）なら 5。
    private void scheduleCloser(final long id) {
        closer = () -> {
            // 窓満了後に許可ダイアログの結果を待ち続けず、次の start が通常の判定を通るようにする。
            permissionPending = false;
            int code;
            BluetoothAdapter adapter = adapterOf(activity);
            if (!launched) {
                code = CODE_OTHER;
            } else if (adapter == null || !enabled(adapter)) {
                code = CODE_DISABLED;
            } else if (locationOff()) {
                code = CODE_LOCATION_OFF;
            } else {
                code = CODE_OK;
            }
            finish(id, code);
        };
        handler.postAtTime(closer, deadline);
    }

    private void onPermissionResult(Map<String, Boolean> result) {
        permissionPending = false;
        long id = request.current();
        if (id == RequestTracker.NONE) {
            return;
        }
        // 取消（空 Map）は拒否扱い。必要な許可がすべて true のときだけ進む。
        boolean granted = !result.isEmpty();
        for (String p : permissions()) {
            if (!Boolean.TRUE.equals(result.get(p))) {
                granted = false;
            }
        }
        if (!granted) {
            finish(id, CODE_PERMISSION);
        } else if (activity.isFinishing() || activity.isDestroyed()) {
            finish(id, CODE_INTERRUPTED);
        } else if (!foreground()) {
            // 背景では開始しない。onResume で再開する。
            startPendingResume = true;
        } else {
            begin(id);
        }
    }

    // 許可済み・前景で呼ぶ。
    private void begin(final long id) {
        BluetoothAdapter adapter = adapterOf(activity);
        if (adapter == null) {
            finish(id, CODE_UNSUPPORTED);
            return;
        }
        if (!enabled(adapter)) {
            finish(id, CODE_DISABLED);
            return;
        }
        if (locationOff()) {
            finish(id, CODE_LOCATION_OFF);
            return;
        }
        if (SystemClock.uptimeMillis() >= deadline) {
            // 窓を実行できていないので 0 件の成功にしない。
            finish(id, CODE_OTHER);
            return;
        }
        forwarded = 0;
        BroadcastReceiver r = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                onFound(id, intent);
            }
        };
        // 参照を保存してから登録し、登録から保存までの間に届いた通知を落とさない。
        receiver = r;
        try {
            ContextCompat.registerReceiver(activity, r,
                    new IntentFilter(BluetoothDevice.ACTION_FOUND), null, handler,
                    ContextCompat.RECEIVER_EXPORTED);
        } catch (RuntimeException e) {
            receiver = null;
            finish(id, CODE_OTHER);
            return;
        }
        try {
            if (adapter.startDiscovery()) {
                launched = true;
            } else {
                finish(id, enabled(adapter) ? CODE_OTHER : CODE_DISABLED);
            }
        } catch (SecurityException e) {
            finish(id, CODE_PERMISSION);
        } catch (RuntimeException e) {
            finish(id, CODE_OTHER);
        }
    }

    @SuppressWarnings("deprecation")
    private void onFound(long id, Intent intent) {
        if (request.current() != id || forwarded >= MAX_FORWARDED || intent == null
                || !BluetoothDevice.ACTION_FOUND.equals(intent.getAction())) {
            return;
        }
        BluetoothDevice d;
        try {
            d = intent.getParcelableExtra(BluetoothDevice.EXTRA_DEVICE);
        } catch (RuntimeException e) {
            return;
        }
        if (d == null || isLe(d)) {
            return;
        }
        String address;
        try {
            address = d.getAddress();
        } catch (SecurityException e) {
            finish(id, CODE_PERMISSION);
            return;
        }
        if (address == null) {
            return;
        }
        String name = intent.getStringExtra(BluetoothDevice.EXTRA_NAME);
        int rssi = intent.getShortExtra(BluetoothDevice.EXTRA_RSSI, Short.MIN_VALUE);
        int cod = -1;
        BluetoothClass bc = null;
        try {
            bc = intent.getParcelableExtra(BluetoothDevice.EXTRA_CLASS);
        } catch (RuntimeException e) {
            // 型が違う extra は無視する。
        }
        if (bc != null) {
            cod = bc.getDeviceClass();
        }
        boolean bonded = false;
        try {
            bonded = d.getBondState() == BluetoothDevice.BOND_BONDED;
        } catch (RuntimeException e) {
            // 取れなければ未ペアリング扱い。
        }
        forwarded++;
        nativeOnClassicFound(id, address, name, cod, rssi, bonded);
    }

    // タイマー・検索・レシーバーを解放する。何度呼んでも安全。
    private void stopQuietly() {
        if (closer != null) {
            handler.removeCallbacks(closer);
            closer = null;
        }
        if (launched) {
            // 自分が始めた検索だけ止める。他アプリの検索は止めない。
            try {
                BluetoothAdapter adapter = adapterOf(activity);
                if (adapter != null) {
                    adapter.cancelDiscovery();
                }
            } catch (RuntimeException e) {
                // 権限取り消し等。解放は続ける。
            }
            launched = false;
        }
        if (receiver != null) {
            try {
                activity.unregisterReceiver(receiver);
            } catch (IllegalArgumentException e) {
                // 解除済みは無視する。
            }
            receiver = null;
        }
    }
}
