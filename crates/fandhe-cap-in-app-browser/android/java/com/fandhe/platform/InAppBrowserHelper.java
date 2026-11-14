package com.fandhe.platform;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.net.Uri;

import androidx.activity.ComponentActivity;
import androidx.browser.customtabs.CustomTabsIntent;
import androidx.lifecycle.Lifecycle;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// アプリ内ブラウザ（Issue #703、親 #135）の Android 実装ヘルパー。利用者の Activity には何も書かせない。
// Rust の `fandhe-cap-in-app-browser/src/android.rs` が static の `open(Activity, long, String)` を JNI で呼び、
// 結果を `nativeOnInAppBrowserResult` で受けて `complete_from_os` へ渡す。Activity の生成・破棄・Resumed / Paused は
// `InAppBrowserLifecycle`（非公開 ContentProvider、`FeatureLifecycleProvider` 継承）が知らせる。Activity ごとの helper は
// `HELPERS` に持つ。helper が無い（生成前・破棄後）要求は成功にも待ちにもせず、`nativeOnInAppBrowserResult` へ
// KIND_INTERRUPTED を返して確定する。
// 表示の直前に `nativeTryBeginPresent` で Rust 側の期限切れ・差し替えと排他にする。
//
// 流れ: Custom Tabs で http / https の URL を開く（非対応ブラウザでは `launchUrl` が既定ブラウザで開く）。
// 閉じた通知の専用 API は無いため、起動後の離脱と復帰（onPause → onResume）を「閉じた」とみなす。
// 期限（10 秒）は Rust 側の単調時計だけが持ち、ここには持たない。
//
// 結果コードの SSOT は KIND_*（Rust 側 `android.rs` の定数と同じ値）。
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

    // Activity ごとの helper。Rust のワーカーと UI スレッドの両方から触るので同期化し、Activity が GC されれば項目も消える。
    private static final Map<Activity, InAppBrowserHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    // 起動を OS に渡した要求 / 起動後に onPause を挟んだ要求。どちらも this のロックで守る。
    private long launchedId = RequestTracker.NONE;
    private long leftId = RequestTracker.NONE;

    private InAppBrowserHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // Activity の生成時（`InAppBrowserLifecycle` から、UI スレッド）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new InAppBrowserHelper(activity));
    }

    // Activity の破棄時（同上）。保留中の要求を中断で確定し、以後の要求を受け付けない。
    static void detach(Activity activity) {
        InAppBrowserHelper helper = HELPERS.remove(activity);
        if (helper != null) {
            helper.onDestroy();
        }
    }

    // Rust から呼ぶ入口（JNI スレッド）。helper が無い要求は中断で確定し、結果が来ないまま待たせない。
    static void open(Activity activity, long id, String url) {
        InAppBrowserHelper helper = HELPERS.get(activity);
        if (helper == null) {
            nativeOnInAppBrowserResult(id, KIND_INTERRUPTED);
            return;
        }
        helper.open(id, url);
    }

    // `InAppBrowserLifecycle` から（UI スレッド）。helper が無ければ何もしない。
    static void resumed(Activity activity) {
        InAppBrowserHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.onResume();
        }
    }

    static void paused(Activity activity) {
        InAppBrowserHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.onPause();
        }
    }

    // JNI スレッドから呼ばれる。
    private void open(long id, String url) {
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

    private synchronized void onPause() {
        if (launchedId != RequestTracker.NONE && launchedId == request.current()) {
            leftId = launchedId;
        }
    }

    // 離脱後の復帰＝閉じた。未起動の onPause / onResume は無視する。
    private synchronized void onResume() {
        long id = leftId;
        if (id != RequestTracker.NONE) {
            finish(id, KIND_CLOSED);
        }
    }

    private synchronized void onDestroy() {
        long id = request.current();
        if (id != RequestTracker.NONE) {
            finish(id, KIND_INTERRUPTED);
        }
    }
}
