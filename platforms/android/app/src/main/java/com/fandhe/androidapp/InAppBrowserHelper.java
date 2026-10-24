package com.fandhe.androidapp;

import android.content.ActivityNotFoundException;
import android.net.Uri;

import androidx.activity.ComponentActivity;
import androidx.browser.customtabs.CustomTabsIntent;
import androidx.lifecycle.Lifecycle;

// アプリ内ブラウザ（Issue #703、親 #135）の Android 実装ヘルパー。MainActivity から生成・委譲される。
// Rust の `device/android_in_app_browser.rs` が `MainActivity.requestInAppBrowserOpen` を JNI で呼び、
// 結果を `nativeOnInAppBrowserResult` で受けて `in_app_browser::complete_from_os` へ渡す。
// 表示の直前に `nativeTryBeginPresent` で Rust 側の期限切れ・差し替えと排他にする。
//
// 流れ: Custom Tabs で http / https の URL を開く（非対応ブラウザでは `launchUrl` が既定ブラウザで開く）。
// 閉じた通知の専用 API は無いため、起動後の離脱と復帰（onPause → onResume）を「閉じた」とみなす。
// 期限（10 秒）は Rust 側の単調時計だけが持ち、ここには持たない。
//
// 結果コードの SSOT は KIND_*（Rust 側 `android_in_app_browser.rs` の定数と同じ値）。
// ライフサイクル: 状態はインスタンスフィールド。再生成後の新インスタンスは NONE から始まり、
// Rust 側 `reset_pending_tracking()` と対で旧要求を捨てる。旧要求の遅延結果は id 照合で捨てる。
// 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止（非前景・ブラウザなしを成功に畳まない）。
// 守らない範囲: 開いた後に Rust から閉じること・閲覧内容と読み込み失敗の検知・既定ブラウザの
// UI の差・複数プロセス・プロセス死後の結果。
// ロック順: launch は this のロックを持ったまま nativeTryBeginPresent を呼ぶ。Rust 側は自分のロックの
// 内側から Java を呼ばない（begin() でロックを放してから open を呼ぶ）ため循環しない。
// URL・例外メッセージはログに出さない。
final class InAppBrowserHelper {
    static final int KIND_CLOSED = 0;
    static final int KIND_NO_BROWSER = 1;
    static final int KIND_NO_PRESENTER = 2;
    static final int KIND_INTERRUPTED = 3;
    static final int KIND_INVALID_URL = 4;
    static final int KIND_FAILED = 5;

    private static native boolean nativeTryBeginPresent(long requestId);

    private static native void nativeOnInAppBrowserResult(long requestId, int kind);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    // 起動を OS に渡した要求 / 起動後に onPause を挟んだ要求。どちらも this のロックで守る。
    private long launchedId = RequestTracker.NONE;
    private long leftId = RequestTracker.NONE;

    InAppBrowserHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // JNI スレッドから呼ばれる。
    void open(long id, String url) {
        synchronized (this) {
            request.track(id);
            launchedId = RequestTracker.NONE;
            leftId = RequestTracker.NONE;
        }
        activity.runOnUiThread(() -> launch(id, url));
    }

    private synchronized void launch(long id, String url) {
        // 差し替え済み・確定済みの旧要求は起動しない。
        if (request.current() != id) {
            return;
        }
        if (activity.isFinishing() || activity.isDestroyed()) {
            finish(id, KIND_INTERRUPTED);
            return;
        }
        // 背景で表示を始めない。
        if (!activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            finish(id, KIND_NO_PRESENTER);
            return;
        }
        // 多重防御: http / https 以外・host なし・userinfo 付きは開かない。
        Uri uri = url == null ? null : Uri.parse(url);
        if (uri == null || !isWebUri(uri)) {
            finish(id, KIND_INVALID_URL);
            return;
        }
        // false は Rust 側が期限切れを確定済み、または古い要求。結果は送らず追跡だけ外す。
        if (!nativeTryBeginPresent(id)) {
            request.complete(id);
            return;
        }
        try {
            new CustomTabsIntent.Builder().build().launchUrl(activity, uri);
            launchedId = id;
        } catch (ActivityNotFoundException e) {
            finish(id, KIND_NO_BROWSER);
        } catch (RuntimeException e) {
            finish(id, KIND_FAILED);
        }
    }

    private static boolean isWebUri(Uri uri) {
        String scheme = uri.getScheme();
        boolean web = "http".equalsIgnoreCase(scheme) || "https".equalsIgnoreCase(scheme);
        String host = uri.getHost();
        return web && host != null && !host.isEmpty() && uri.getUserInfo() == null;
    }

    // first-wins: 現在の要求だけ確定して Rust へ返す。
    private void finish(long id, int kind) {
        if (request.complete(id)) {
            launchedId = RequestTracker.NONE;
            leftId = RequestTracker.NONE;
            nativeOnInAppBrowserResult(id, kind);
        }
    }

    synchronized void onPause() {
        if (launchedId != RequestTracker.NONE && launchedId == request.current()) {
            leftId = launchedId;
        }
    }

    // 離脱後の復帰＝閉じた。未起動の onPause / onResume は無視する。
    synchronized void onResume() {
        long id = leftId;
        if (id != RequestTracker.NONE) {
            finish(id, KIND_CLOSED);
        }
    }

    synchronized void onDestroy() {
        long id = request.current();
        if (id != RequestTracker.NONE) {
            finish(id, KIND_INTERRUPTED);
        }
    }
}
