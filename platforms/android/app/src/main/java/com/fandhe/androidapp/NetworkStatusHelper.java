package com.fandhe.androidapp;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;

// ネットワーク接続状態画面（Issue #1284）の Android 実装ヘルパー。
// Rust の `device/android_network_status.rs` が `MainActivity.networkStatusRead` 経由で
// 1 秒間隔に同期で呼ぶ。判定ロジックは Rust 側の `snapshot_from_code`（ホストの
// `cargo test` で検査）に寄せ、ここは OS の事実を int のビットに詰めて返すだけにする。
// コールバック登録・要求 id・保留状態を持たないため、Activity 再生成で破棄すべき要求は無い。
// 定数の値は Rust 側の同名定数（結果コード表）と一致させること。
final class NetworkStatusHelper {
    static final int FAILED = -1;
    static final int OFFLINE = 0;
    static final int PRESENT = 1;
    static final int NOT_METERED = 1 << 1;
    static final int WIFI = 1 << 2;
    static final int CELLULAR = 1 << 3;
    static final int ETHERNET = 1 << 4;

    private NetworkStatusHelper() {}

    // 戻り値: FAILED = 取得失敗、OFFLINE = アクティブなネットワーク無し、
    // それ以外 = PRESENT に各ビットを足した値。接続詳細はログへ出さない。
    static int read(Context context) {
        try {
            ConnectivityManager cm = context.getSystemService(ConnectivityManager.class);
            if (cm == null) {
                return FAILED;
            }
            Network network = cm.getActiveNetwork();
            if (network == null) {
                return OFFLINE;
            }
            NetworkCapabilities caps = cm.getNetworkCapabilities(network);
            if (caps == null) {
                return OFFLINE;
            }
            int code = PRESENT;
            if (caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)) {
                code |= NOT_METERED;
            }
            if (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) {
                code |= WIFI;
            }
            if (caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)) {
                code |= CELLULAR;
            }
            if (caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)) {
                code |= ETHERNET;
            }
            return code;
        } catch (RuntimeException e) {
            return FAILED;
        }
    }
}
