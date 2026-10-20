package com.fandhe.androidapp;

import android.Manifest;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.net.wifi.aware.AttachCallback;
import android.net.wifi.aware.DiscoverySession;
import android.net.wifi.aware.DiscoverySessionCallback;
import android.net.wifi.aware.PeerHandle;
import android.net.wifi.aware.PublishConfig;
import android.net.wifi.aware.PublishDiscoverySession;
import android.net.wifi.aware.SubscribeConfig;
import android.net.wifi.aware.SubscribeDiscoverySession;
import android.net.wifi.aware.WifiAwareManager;
import android.net.wifi.aware.WifiAwareSession;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.annotation.RequiresApi;
import androidx.core.content.ContextCompat;
import androidx.lifecycle.Lifecycle;

import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

// 近くの端末との直接通信（Issue #513、親 #110、Wi-Fi Aware のメッセージのみ）の Android 実装ヘルパー。
// MainActivity から生成・委譲される（Rust の `device/android_local_peer.rs` が
// `MainActivity.requestLocalPeer` を JNI で呼び、受信と完了を native 2 本で返す）。
//
// 通信: 両端末が同じサービス名で publish（unsolicited）と subscribe（passive）を同時に行い、
// 発見した相手へ「自分の表示名(15 バイト固定) + 送信文字列」を 1 通送り、受け取った相手へも返信する。
// データパス（requestNetwork）と GMS 依存の Nearby Connections は使わない。
// 受信データの検証（名前の形式・UTF-8・200 バイト・制御文字）は Rust の `Pending::offer` に一本化し、
// ここでは長さで名前と本文に分けて渡すだけ。ペイロードは表示用の短文で秘密値ではない（平文）。
//
// 並行性: 状態を触る経路は main looper 1 本（attach / publish / subscribe / receiver に main Handler を
// 渡し、JNI 受付は post）。ロックは持たない。完了は RequestTracker の first-wins で 1 回だけ通知する。
//
// 期限: 窓の長さは Rust 側 EXCHANGE_WINDOW_MS を `present` 経由で受け取り、JNI を受けた時点（post より前）
// の uptimeMillis を起点にする。UI キューや許可ダイアログでの待ちも窓に含め、過ぎていれば窓を実行して
// いないのでコード 6（空の成功にしない）。
//
// ライフサイクル: onPause（許可ダイアログ待ちを除く）・onDestroy・非前景での開始はコード 4。非前景で許可が
// 届いたら onResume まで開始を保留する。再生成後の新インスタンスは空の tracker から始まり、旧 id の
// 遅延通知は Rust 側 reset_pending_tracking と PENDING の id 照合で捨てられる。
//
// 結果コードの SSOT は Rust の `fandhe_device::local_peer::decode_code`。ここは値を合わせるだけ。
// 表示名・受信文字列・peerHandle はログへ出さない。
// 守らない範囲: コア doc のとおり。加えて API 32 以下で位置情報サービスがオフのときの挙動は未確認。
final class LocalPeerHelper {
    private static final class Outgoing {
        final Object session;
        final PeerHandle peer;
        final boolean reply;
        final int retries;

        Outgoing(Object session, PeerHandle peer, boolean reply, int retries) {
            this.session = session;
            this.peer = peer;
            this.reply = reply;
            this.retries = retries;
        }
    }

    private static final int CODE_OK = 0;
    private static final int CODE_PERMISSION = 1;
    private static final int CODE_DISABLED = 2;
    private static final int CODE_UNSUPPORTED = 3;
    private static final int CODE_INTERRUPTED = 4;
    private static final int CODE_OTHER = 6;

    // SSOT は Rust の `fandhe_device::local_peer::SERVICE_TYPE`。
    private static final String SERVICE_NAME = "fandhe-p2p";
    // 表示名の長さ（`fandhe-` + 8 桁）。Rust の is_valid_peer_name と同じ。
    private static final int NAME_BYTES = 15;
    // Rust の MAX_TEXT_BYTES。超える受信は JNI を呼ぶ前に捨てる。
    private static final int MAX_TEXT_BYTES = 200;
    // ponytail: 洪水対策で 1 窓の送信数に上限を置く。3 台以上の選び方は守らない範囲。
    private static final int MAX_SENDS = 8;
    // 受理後の返信の上限。発見起点の送信（MAX_SENDS）と枠を分け、返信が枯渇しないようにする。
    private static final int MAX_REPLIES = 8;
    // 非同期の送信失敗（onMessageSendFailed）を同じ相手へ再送する回数の上限（1 通あたり）。
    private static final int MAX_RETRIES = 3;

    private static native boolean nativeOnLocalPeerMessage(long id, String name, byte[] text);

    private static native void nativeOnLocalPeerFinished(long id, int code);

    private final ComponentActivity activity;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final RequestTracker request = new RequestTracker();
    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity のフィールド初期化子で生成する。
    private final ActivityResultLauncher<String[]> permissionLauncher;

    // 以下は main looper 専用。Aware の型は API 26 未満でクラス解決しないよう Object で持つ。
    private boolean permissionPending;
    private boolean startPendingResume;
    private String ownName;
    private String outgoing;
    private long deadline;
    private Runnable closer;
    private boolean launched;
    private boolean accepted;
    private int sends;
    private int replies;
    // 送信中のメッセージ（messageId → 送信先と試行回数）。失敗通知から再送するために持つ。
    private final Map<Integer, Outgoing> inflight = new HashMap<>();
    private int sendOk;
    private int nextMsgId;
    private byte[] payload;
    private Object awareSession;
    private Object publishSession;
    private Object subscribeSession;
    private BroadcastReceiver receiver;

    LocalPeerHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.RequestMultiplePermissions(), this::onPermissionResult);
    }

    // JNI スレッドから呼ばれる。
    void request(final long id, final String ownName, final String outgoing, int windowMs) {
        final long dl = SystemClock.uptimeMillis() + windowMs;
        handler.post(() -> start(id, ownName, outgoing, dl));
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
            nativeOnLocalPeerFinished(id, CODE_INTERRUPTED);
        }
    }

    private void finish(long id, int code) {
        // 旧 id の遅延通知が現行要求の資源を止めないよう、照合が通ったときだけ解放する。
        if (request.complete(id)) {
            stopQuietly();
            nativeOnLocalPeerFinished(id, code);
        }
    }

    private boolean foreground() {
        return !activity.isFinishing() && !activity.isDestroyed()
                && activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED);
    }

    // API 33 以降は NEARBY_WIFI_DEVICES。API 32 以下は FINE が要るが、Android 12 系では FINE の単独要求が
    // 無視される端末があるため COARSE と同時に要求する（MainActivity の位置情報許可と同じ、契約表 7 節）。
    private static String[] permissions() {
        return Build.VERSION.SDK_INT >= 33
                ? new String[] {Manifest.permission.NEARBY_WIFI_DEVICES}
                : new String[] {Manifest.permission.ACCESS_COARSE_LOCATION,
                        Manifest.permission.ACCESS_FINE_LOCATION};
    }

    // 利用に必要な許可（API 33 以降は NEARBY、以前は FINE）。
    private static String required() {
        return Build.VERSION.SDK_INT >= 33
                ? Manifest.permission.NEARBY_WIFI_DEVICES
                : Manifest.permission.ACCESS_FINE_LOCATION;
    }

    private void start(long id, String name, String text, long dl) {
        // 再要求は置き換え。旧 id は通知せず捨てる（Rust 側は新しい世代へ移っている）。
        request.take();
        stopQuietly();
        startPendingResume = false;
        ownName = name;
        outgoing = text;
        deadline = dl;
        // 許可ダイアログ表示中の再要求は id を追跡し直すだけ（結果は最新 id で処理する）。
        if (permissionPending) {
            request.track(id);
            scheduleCloser(id);
            return;
        }
        // 非搭載判定を許可確認より先に行い、非対応端末でダイアログを出さない。
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O
                || !activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_WIFI_AWARE)
                || activity.getSystemService(Context.WIFI_AWARE_SERVICE) == null) {
            nativeOnLocalPeerFinished(id, CODE_UNSUPPORTED);
            return;
        }
        if (!foreground()) {
            nativeOnLocalPeerFinished(id, CODE_INTERRUPTED);
            return;
        }
        request.track(id);
        // 期限は許可待ちの間も進める。過ぎていれば即時に走る。
        scheduleCloser(id);
        if (ContextCompat.checkSelfPermission(activity, required())
                != PackageManager.PERMISSION_GRANTED) {
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

    // 閉じる時点で未開始（許可待ちのまま等）ならコード 6、開始済みなら 0（利用不可になっていれば 2）。
    // 相手を見て送信を試みた（sends > 0）のに有効な受信が無いなら、送信の成否・再送の尽き方によらず 6（Internal）。
    // 相手を一度も見ていないとき（sends == 0）だけ 0（相手なし）。受信済みは 0。
    private void scheduleCloser(final long id) {
        closer = () -> {
            // 窓満了後に許可ダイアログの結果を待ち続けず、次の start が通常の判定を通るようにする。
            permissionPending = false;
            int code;
            if (!launched) {
                code = CODE_OTHER;
            } else if (!available()) {
                code = CODE_DISABLED;
            } else {
                code = (sends > 0 && !accepted) ? CODE_OTHER : CODE_OK;
            }
            finish(id, code);
        };
        handler.postAtTime(closer, deadline);
    }

    private boolean available() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return false;
        }
        return awareAvailable();
    }

    @RequiresApi(Build.VERSION_CODES.O)
    private boolean awareAvailable() {
        WifiAwareManager m = (WifiAwareManager) activity.getSystemService(Context.WIFI_AWARE_SERVICE);
        return m != null && m.isAvailable();
    }

    private void onPermissionResult(Map<String, Boolean> result) {
        permissionPending = false;
        long id = request.current();
        if (id == RequestTracker.NONE) {
            return;
        }
        // 取消（空 Map）は拒否扱い。必要な許可（FINE / NEARBY）の結果で判定する。
        if (!Boolean.TRUE.equals(result.get(required()))) {
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

    // 許可済み・前景で呼ぶ。API 26 未満では start が弾くのでここへ来ない。
    private void begin(long id) {
        if (SystemClock.uptimeMillis() >= deadline) {
            // 窓を実行できていないので空の成功にしない。
            finish(id, CODE_OTHER);
            return;
        }
        try {
            beginAware(id);
        } catch (SecurityException e) {
            finish(id, CODE_PERMISSION);
        } catch (RuntimeException e) {
            finish(id, CODE_OTHER);
        }
    }

    @RequiresApi(Build.VERSION_CODES.O)
    private void beginAware(final long id) {
        final WifiAwareManager mgr =
                (WifiAwareManager) activity.getSystemService(Context.WIFI_AWARE_SERVICE);
        byte[] name = ownName.getBytes(StandardCharsets.US_ASCII);
        byte[] text = outgoing.getBytes(StandardCharsets.UTF_8);
        if (name.length != NAME_BYTES || text.length > MAX_TEXT_BYTES) {
            finish(id, CODE_OTHER);
            return;
        }
        payload = new byte[NAME_BYTES + text.length];
        System.arraycopy(name, 0, payload, 0, NAME_BYTES);
        System.arraycopy(text, 0, payload, NAME_BYTES, text.length);
        // isAvailable() の確認より先に登録し、確認から登録までの状態変化を落とさない。
        receiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                if (request.current() == id && !available()) {
                    finish(id, CODE_DISABLED);
                }
            }
        };
        ContextCompat.registerReceiver(activity, receiver,
                new IntentFilter(WifiAwareManager.ACTION_WIFI_AWARE_STATE_CHANGED),
                ContextCompat.RECEIVER_NOT_EXPORTED);
        // Wi-Fi オフ等では getCharacteristics() が null を返すため、利用可能性の確認より後に参照する。
        if (!mgr.isAvailable() || mgr.getCharacteristics() == null) {
            finish(id, CODE_DISABLED);
            return;
        }
        if (NAME_BYTES + text.length > mgr.getCharacteristics().getMaxServiceSpecificInfoLength()) {
            finish(id, CODE_UNSUPPORTED);
            return;
        }
        mgr.attach(new AttachCallback() {
            @Override
            public void onAttached(WifiAwareSession session) {
                if (request.current() != id) {
                    session.close();
                    return;
                }
                awareSession = session;
                launched = true;
                publishAndSubscribe(id, session);
            }

            @Override
            public void onAttachFailed() {
                finish(id, CODE_DISABLED);
            }
        }, handler);
    }

    @RequiresApi(Build.VERSION_CODES.O)
    private void publishAndSubscribe(final long id, WifiAwareSession session) {
        try {
            session.publish(new PublishConfig.Builder().setServiceName(SERVICE_NAME)
                    .setPublishType(PublishConfig.PUBLISH_TYPE_UNSOLICITED).build(),
                    new DiscoverySessionCallback() {
                        @Override
                        public void onPublishStarted(PublishDiscoverySession s) {
                            if (request.current() != id) {
                                s.close();
                                return;
                            }
                            publishSession = s;
                        }

                        @Override
                        public void onSessionConfigFailed() {
                            finish(id, CODE_OTHER);
                        }

                        @Override
                        public void onSessionTerminated() {
                            // OS による探索終了。窓の途中なら嘘の成功にせず失敗で確定する。
                            finish(id, CODE_OTHER);
                        }

                        @Override
                        public void onMessageReceived(PeerHandle peer, byte[] message) {
                            received(id, publishSession, peer, message);
                        }

                        @Override
                        public void onMessageSendSucceeded(int messageId) {
                            sent(id, messageId);
                        }

                        @Override
                        public void onMessageSendFailed(int messageId) {
                            resend(id, messageId);
                        }
                    }, handler);
            session.subscribe(new SubscribeConfig.Builder().setServiceName(SERVICE_NAME)
                    .setSubscribeType(SubscribeConfig.SUBSCRIBE_TYPE_PASSIVE).build(),
                    new DiscoverySessionCallback() {
                        @Override
                        public void onSubscribeStarted(SubscribeDiscoverySession s) {
                            if (request.current() != id) {
                                s.close();
                                return;
                            }
                            subscribeSession = s;
                        }

                        @Override
                        public void onSessionConfigFailed() {
                            finish(id, CODE_OTHER);
                        }

                        @Override
                        public void onSessionTerminated() {
                            // OS による探索終了。窓の途中なら嘘の成功にせず失敗で確定する。
                            finish(id, CODE_OTHER);
                        }

                        @Override
                        public void onServiceDiscovered(PeerHandle peer, byte[] info,
                                List<byte[]> filter) {
                            send(id, subscribeSession, peer, false, 0);
                        }

                        @Override
                        public void onMessageReceived(PeerHandle peer, byte[] message) {
                            received(id, subscribeSession, peer, message);
                        }

                        @Override
                        public void onMessageSendSucceeded(int messageId) {
                            sent(id, messageId);
                        }

                        @Override
                        public void onMessageSendFailed(int messageId) {
                            resend(id, messageId);
                        }
                    }, handler);
        } catch (SecurityException e) {
            finish(id, CODE_PERMISSION);
        } catch (RuntimeException e) {
            finish(id, CODE_OTHER);
        }
    }

    @RequiresApi(Build.VERSION_CODES.O)
    private void send(long id, Object session, PeerHandle peer, boolean reply, int retries) {
        if (request.current() != id || session == null || payload == null) {
            return;
        }
        // 再送（retries > 0）は上限を消費しない。発見起点と返信は別枠。
        if (retries == 0) {
            if (reply ? replies >= MAX_REPLIES : sends >= MAX_SENDS) {
                return;
            }
            if (reply) {
                replies++;
            } else {
                sends++;
            }
        }
        int msgId = nextMsgId++;
        inflight.put(msgId, new Outgoing(session, peer, reply, retries));
        try {
            ((DiscoverySession) session).sendMessage(peer, msgId, payload);
        } catch (SecurityException e) {
            // API 呼び出し自体の例外は「相手なし」の成功にせず、失敗として確定する。
            inflight.remove(msgId);
            finish(id, CODE_PERMISSION);
        } catch (RuntimeException e) {
            inflight.remove(msgId);
            finish(id, CODE_OTHER);
        }
    }

    // 非同期の送信失敗。同じ相手へ上限つきで再送する（失敗通知が来ない限り何もしない）。
    @RequiresApi(Build.VERSION_CODES.O)
    private void resend(long id, int messageId) {
        Outgoing o = inflight.remove(messageId);
        if (o != null && o.retries < MAX_RETRIES) {
            send(id, o.session, o.peer, o.reply, o.retries + 1);
        }
    }

    @RequiresApi(Build.VERSION_CODES.O)
    private void received(long id, Object session, PeerHandle peer, byte[] message) {
        if (request.current() != id || message == null || message.length < NAME_BYTES
                || message.length > NAME_BYTES + MAX_TEXT_BYTES) {
            return;
        }
        String name = new String(message, 0, NAME_BYTES, StandardCharsets.US_ASCII);
        byte[] text = Arrays.copyOfRange(message, NAME_BYTES, message.length);
        if (nativeOnLocalPeerMessage(id, name, text)) {
            accepted = true;
            send(id, session, peer, true, 0);
            maybeDone(id);
        }
    }

    private void sent(long id, int messageId) {
        if (request.current() == id) {
            inflight.remove(messageId);
            sendOk++;
            maybeDone(id);
        }
    }

    // 受信済みかつ自分の送信が 1 件以上届いたら早期確定する（届いた保証は守らない範囲）。
    private void maybeDone(long id) {
        if (accepted && sendOk > 0) {
            finish(id, CODE_OK);
        }
    }

    // タイマー・セッション・レシーバーを解放する。何度呼んでも安全。
    private void stopQuietly() {
        if (closer != null) {
            handler.removeCallbacks(closer);
            closer = null;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            closeAware();
        }
        publishSession = null;
        subscribeSession = null;
        awareSession = null;
        if (receiver != null) {
            try {
                activity.unregisterReceiver(receiver);
            } catch (RuntimeException e) {
                // 未登録は無視する。
            }
            receiver = null;
        }
        launched = false;
        accepted = false;
        sends = 0;
        replies = 0;
        sendOk = 0;
        inflight.clear();
        payload = null;
    }

    @RequiresApi(Build.VERSION_CODES.O)
    private void closeAware() {
        // 1 つの close の例外で後続の解放が飛ばないよう、個別に囲む。
        try {
            if (publishSession != null) {
                ((DiscoverySession) publishSession).close();
            }
        } catch (RuntimeException e) {
            // 閉じ済みは無視する。
        }
        try {
            if (subscribeSession != null) {
                ((DiscoverySession) subscribeSession).close();
            }
        } catch (RuntimeException e) {
            // 閉じ済みは無視する。
        }
        try {
            if (awareSession != null) {
                ((WifiAwareSession) awareSession).close();
            }
        } catch (RuntimeException e) {
            // 閉じ済みは無視する。
        }
    }
}
