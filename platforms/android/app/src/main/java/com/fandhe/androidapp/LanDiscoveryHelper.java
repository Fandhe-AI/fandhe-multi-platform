package com.fandhe.androidapp;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.nsd.NsdManager;
import android.net.nsd.NsdServiceInfo;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;

import androidx.activity.ComponentActivity;
import androidx.lifecycle.Lifecycle;

import java.net.Inet4Address;
import java.net.InetAddress;
import java.util.ArrayDeque;
import java.util.List;

// LAN のサービス探索（mDNS / Bonjour、Issue #489、親 #109）の Android 実装ヘルパー。
// MainActivity から生成・委譲される（Rust の `device/android_lan_discovery.rs` が
// `MainActivity.requestLanDiscovery` を JNI で呼び、結果を native 2 本で返す）。
//
// 並行性: 状態を触る経路は main looper 1 本（`requestLanDiscovery` は post、NSD の各コールバックも
// API 33 未満は内部スレッドで届くため main へ post し直す）。ロックは持たない。
// 完了は RequestTracker の first-wins で 1 回だけ通知する。
//
// 期限: 窓の長さ(durationMs)は Rust 側 BROWSE_DURATION_MS をそのまま受け取る。JNI を受けた時点
// （post より前）の uptimeMillis を起点にし、UI キューでの待ちも窓に含める。
//
// ライフサイクル: onPause / onDestroy で保留中の要求をコード 4（中断）で確定し、探索・タイマー・
// 解決キューを解放する。非前景での開始要求も 4。再生成後の新インスタンスは空の tracker から始まり、
// 旧 id の完了は Rust 側 reset_pending_tracking と id 照合で捨てられる。
//
// 結果コードの SSOT は Rust の `decode_code`。ここは値を合わせるだけで分岐しない。
// サービス名・アドレスはログへ出さない。守らない範囲: 窓の間に公開をやめたサービスが残ること、
// 窓内に解決が済まないこと、API 34 未満は解決を取り消せないこと、IP は 1 件のみ。
final class LanDiscoveryHelper {
    private static final int CODE_OK = 0;
    private static final int CODE_PERMISSION = 1;
    private static final int CODE_DISCONNECTED = 2;
    private static final int CODE_UNSUPPORTED = 3;
    private static final int CODE_INTERRUPTED = 4;
    private static final int CODE_OTHER = 6;

    private static final String SERVICE_TYPE = "_http._tcp";
    // ponytail: 応答の洪水対策の上限。超えた分は捨てる。上限に当たる環境が出たら入れ替え方を検討する。
    private static final int MAX_FORWARDED = 256;
    private static final int MAX_RESOLVE_QUEUE = 64;

    private static native void nativeOnLanService(long id, String name, String addr, int port);

    private static native void nativeOnLanFinished(long id, int code);

    private final ComponentActivity activity;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final RequestTracker request = new RequestTracker();

    // 以下は main looper 専用。
    private NsdManager.DiscoveryListener discovery;
    // stopServiceDiscovery 済みで onDiscoveryStopped 待ちの listener。届くまで同 type の探索は
    // 有効扱いのため、この間は discoverServices を呼ばず pendingLaunch に保留する。
    private NsdManager.DiscoveryListener stopping;
    private Runnable pendingLaunch;
    private final ArrayDeque<NsdServiceInfo> resolveQueue = new ArrayDeque<>();
    private NsdManager.ResolveListener resolving;
    private int forwarded;
    private Runnable closer;
    // API 31 以下は Wi-Fi が mDNS 応答を捨てるため探索の間だけ保持する（空リストの嘘の成功を防ぐ）。
    private WifiManager.MulticastLock multicastLock;

    LanDiscoveryHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // JNI スレッドから呼ばれる。
    void request(final long id, int durationMs) {
        final long deadline = SystemClock.uptimeMillis() + durationMs;
        handler.post(() -> start(id, deadline));
    }

    void onPause() {
        interrupt();
    }

    void onDestroy() {
        interrupt();
    }

    private void interrupt() {
        long id = request.take();
        stopQuietly();
        if (id != RequestTracker.NONE) {
            nativeOnLanFinished(id, CODE_INTERRUPTED);
        }
    }

    private void finish(long id, int code) {
        // 旧 id の遅延通知が現行要求の資源を止めないよう、照合が通ったときだけ解放する。
        if (request.complete(id)) {
            stopQuietly();
            nativeOnLanFinished(id, code);
        }
    }

    private void start(final long id, long deadline) {
        // 再要求は置き換え。旧 id は通知せず捨てる（Rust 側は新しい世代へ移っている）。
        request.take();
        stopQuietly();
        if (activity.isFinishing() || activity.isDestroyed()
                || !activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            nativeOnLanFinished(id, CODE_INTERRUPTED);
            return;
        }
        final NsdManager nsd = (NsdManager) activity.getSystemService(Context.NSD_SERVICE);
        if (nsd == null) {
            nativeOnLanFinished(id, CODE_UNSUPPORTED);
            return;
        }
        if (!connected()) {
            nativeOnLanFinished(id, CODE_DISCONNECTED);
            return;
        }
        request.track(id);
        forwarded = 0;
        acquireMulticastLock();
        discovery = new NsdManager.DiscoveryListener() {
            @Override
            public void onDiscoveryStarted(String serviceType) {
            }

            @Override
            public void onServiceFound(final NsdServiceInfo info) {
                handler.post(() -> onFound(id, info));
            }

            @Override
            public void onServiceLost(NsdServiceInfo info) {
            }

            @Override
            public void onDiscoveryStopped(String serviceType) {
                handler.post(() -> stopped(this));
            }

            @Override
            public void onStartDiscoveryFailed(String serviceType, int errorCode) {
                handler.post(() -> finish(id, CODE_OTHER));
            }

            @Override
            public void onStopDiscoveryFailed(String serviceType, int errorCode) {
                handler.post(() -> stopped(this));
            }
        };
        final NsdManager.DiscoveryListener listener = discovery;
        // 期限は保留中も進める。過ぎていれば即時に走る。
        closer = () -> close(id);
        handler.postAtTime(closer, deadline);
        Runnable launch = () -> {
            try {
                nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener);
            } catch (SecurityException e) {
                finish(id, CODE_PERMISSION);
            } catch (RuntimeException e) {
                finish(id, CODE_OTHER);
            }
        };
        if (stopping != null) {
            pendingLaunch = launch;
        } else {
            launch.run();
        }
    }

    // 旧探索の停止が確定した。保留中の開始があれば今走らせる。
    private void stopped(NsdManager.DiscoveryListener who) {
        if (stopping != who) {
            return;
        }
        stopping = null;
        Runnable launch = pendingLaunch;
        pendingLaunch = null;
        if (launch != null) {
            launch.run();
        }
    }

    // 窓が閉じた。未接続なら 0 件の嘘の成功を避けてコード 2。
    private void close(long id) {
        finish(id, connected() ? CODE_OK : CODE_DISCONNECTED);
    }

    private boolean connected() {
        ConnectivityManager cm =
                (ConnectivityManager) activity.getSystemService(Context.CONNECTIVITY_SERVICE);
        return cm != null && cm.getActiveNetwork() != null;
    }

    private void onFound(long id, NsdServiceInfo info) {
        if (request.current() != id || forwarded >= MAX_FORWARDED) {
            return;
        }
        forwarded++;
        String name = info.getServiceName();
        // 名前だけの行を先に送る。解決できたら同じ名前で addr / port を送り、合流は Rust 側が担う。
        nativeOnLanService(id, name, null, 0);
        if (resolveQueue.size() < MAX_RESOLVE_QUEUE) {
            resolveQueue.add(info);
            resolveNext(id);
        }
    }

    // 解決は全 API で 1 件ずつ直列に回す（API 34 未満は同時に 1 件のみ）。
    @SuppressWarnings("deprecation")
    private void resolveNext(final long id) {
        if (resolving != null || resolveQueue.isEmpty()) {
            return;
        }
        NsdManager nsd = (NsdManager) activity.getSystemService(Context.NSD_SERVICE);
        if (nsd == null) {
            return;
        }
        final NsdServiceInfo target = resolveQueue.poll();
        final NsdManager.ResolveListener[] self = new NsdManager.ResolveListener[1];
        self[0] = new NsdManager.ResolveListener() {
            @Override
            public void onResolveFailed(NsdServiceInfo info, int errorCode) {
                handler.post(() -> afterResolve(id, self[0], null));
            }

            @Override
            public void onServiceResolved(final NsdServiceInfo info) {
                handler.post(() -> afterResolve(id, self[0], info));
            }
        };
        resolving = self[0];
        try {
            nsd.resolveService(target, resolving);
        } catch (RuntimeException e) {
            resolving = null;
            resolveNext(id);
        }
    }

    private void afterResolve(long id, NsdManager.ResolveListener who, NsdServiceInfo info) {
        // 窓を閉じた後・置き換え後に遅れて届いたコールバックは捨てる。
        if (resolving != who || request.current() != id) {
            return;
        }
        resolving = null;
        if (info != null) {
            nativeOnLanService(id, info.getServiceName(), pickAddress(info), info.getPort());
        }
        resolveNext(id);
    }

    @SuppressWarnings("deprecation")
    private static String pickAddress(NsdServiceInfo info) {
        InetAddress chosen = null;
        if (Build.VERSION.SDK_INT >= 34) {
            List<InetAddress> all = info.getHostAddresses();
            for (InetAddress a : all) {
                if (a instanceof Inet4Address) {
                    chosen = a;
                    break;
                }
                if (chosen == null) {
                    chosen = a;
                }
            }
        } else {
            chosen = info.getHost();
        }
        return chosen == null ? null : chosen.getHostAddress();
    }

    private void acquireMulticastLock() {
        try {
            WifiManager wifi = (WifiManager) activity.getApplicationContext()
                    .getSystemService(Context.WIFI_SERVICE);
            if (wifi != null) {
                multicastLock = wifi.createMulticastLock("fandhe-lan-discovery");
                multicastLock.setReferenceCounted(false);
                multicastLock.acquire();
            }
        } catch (RuntimeException e) {
            // 取得できない端末（Wi-Fi なし等）でも探索は続ける。
            multicastLock = null;
        }
    }

    // タイマー・探索・解決・マルチキャストロックを解放する。何度呼んでも安全。
    private void stopQuietly() {
        pendingLaunch = null;
        if (closer != null) {
            handler.removeCallbacks(closer);
            closer = null;
        }
        NsdManager nsd = (NsdManager) activity.getSystemService(Context.NSD_SERVICE);
        if (nsd != null) {
            if (discovery != null) {
                try {
                    nsd.stopServiceDiscovery(discovery);
                    // listener は onDiscoveryStopped まで捨てず、次の開始を直列化する。
                    stopping = discovery;
                } catch (RuntimeException e) {
                    // 未登録（探索が有効でない）なら待つものはない。
                }
            }
            if (resolving != null && Build.VERSION.SDK_INT >= 34) {
                try {
                    nsd.stopServiceResolution(resolving);
                } catch (RuntimeException e) {
                    // 取り消せない場合は id 照合で遅延コールバックを捨てる。
                }
            }
        }
        discovery = null;
        resolving = null;
        resolveQueue.clear();
        if (multicastLock != null) {
            try {
                if (multicastLock.isHeld()) {
                    multicastLock.release();
                }
            } catch (RuntimeException e) {
                // 解放済みは無視する。
            }
            multicastLock = null;
        }
    }
}
