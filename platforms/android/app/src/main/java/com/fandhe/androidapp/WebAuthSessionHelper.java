package com.fandhe.androidapp;

import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.net.Uri;

import androidx.activity.ComponentActivity;
import androidx.browser.customtabs.CustomTabsIntent;
import androidx.lifecycle.Lifecycle;

import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.security.SecureRandom;

// OAuth 用 Web 認証セッション（Issue #534、親 #113）の Android 実装ヘルパー。MainActivity から
// 生成・委譲される。Rust の `device/android_web_auth_session.rs` が `MainActivity.requestWebAuthStart`
// 等を JNI で呼び、結果を `nativeOnWebAuthSessionResult` で受けて `web_auth_session::bridge::complete_from_os`
// へ渡す（コールバック URL の検査は Rust 側 `verify_callback` に一本化）。
//
// 流れ: Custom Tabs で認可ページ（デモは 127.0.0.1）を開く（開けなければ ACTION_VIEW）→ ブラウザが
// `fandhe://web-auth/callback?...` へ遷移 → 既存の intent-filter + singleTask により
// `MainActivity.onNewIntent` へ届き、ここで確定する。url-scheme 受信箱へは流さない
// （code・state を別画面に出さない）。コールバックなしで戻った（onPause 後の onResume）はキャンセル。
// 期限（300 秒）は Rust 側の単調時計だけが持ち、ここには持たない。
//
// コールバックは開始 URL の state と一致する現在の要求にだけ確定する（不一致は破棄）。
//
// 結果コードの SSOT は KIND_*（Rust 側の写像と同じ値）。
// ライフサイクル: 状態はインスタンスフィールド。再生成後の新インスタンスは NONE から始まり、
// Rust 側 `reset_pending_tracking()` と対で旧要求を捨てる。旧要求の遅延結果は id 照合で捨てる。
// 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止（非前景・ブラウザなしを成功に畳まない）。
// 守らない範囲: Custom Tab を閉じられないこと（取り消し・期限切れ後もタブが残る）・既定ブラウザの
// UI の差・複数プロセス・プロセス死後のコールバック・他アプリによる `fandhe` scheme の横取り
// （PKCE で緩和、RFC 8252 §8.1）・トークン交換。
// URL・state・code・例外メッセージはログに出さない。
final class WebAuthSessionHelper {
    static final int KIND_URL = 0;
    static final int KIND_CANCELLED = 1;
    static final int KIND_NO_BROWSER = 2;
    static final int KIND_FAILED = 3;

    private static native void nativeOnWebAuthSessionResult(long requestId, int kind, String url);

    private static final SecureRandom RANDOM = new SecureRandom();

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    // 起動を OS に渡した要求 / 起動後に onPause を挟んだ要求。どちらも this のロックで守る。
    private long launchedId = RequestTracker.NONE;
    private long leftId = RequestTracker.NONE;
    // 現在の要求の開始 URL に含まれる state。コールバックの所属照合に使う（this のロックで守る）。
    private String currentState = null;

    WebAuthSessionHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // JNI スレッドから呼ばれる。
    void start(long id, String url) {
        synchronized (this) {
            request.track(id);
            currentState = Uri.parse(url).getQueryParameter("state");
            launchedId = RequestTracker.NONE;
            leftId = RequestTracker.NONE;
        }
        activity.runOnUiThread(() -> launch(id, url));
    }

    // JNI スレッドから呼ばれる。launch と同じロックで排他にし、未開始なら起動させない。
    // 開いた Custom Tab は閉じられない（守らない範囲）。
    void cancel(long id) {
        synchronized (this) {
            if (request.complete(id)) {
                launchedId = RequestTracker.NONE;
                leftId = RequestTracker.NONE;
            }
        }
    }

    private synchronized void launch(long id, String url) {
        // 取り消し済み・差し替え済みの旧要求は起動しない。
        if (request.current() != id) {
            return;
        }
        if (activity.isFinishing() || activity.isDestroyed()
                || !activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            finish(id, KIND_FAILED);
            return;
        }
        // 多重防御: 開くのはデモ認可ページ（http://127.0.0.1）だけ。任意 URL を開く入口にしない。
        Uri uri = Uri.parse(url);
        if (!"http".equals(uri.getScheme()) || !"127.0.0.1".equals(uri.getHost())) {
            finish(id, KIND_FAILED);
            return;
        }
        try {
            try {
                new CustomTabsIntent.Builder().build().launchUrl(activity, uri);
            } catch (ActivityNotFoundException e) {
                activity.startActivity(new Intent(Intent.ACTION_VIEW, uri)
                        .addCategory(Intent.CATEGORY_BROWSABLE));
            }
            launchedId = id;
        } catch (ActivityNotFoundException e) {
            finish(id, KIND_NO_BROWSER);
        } catch (RuntimeException e) {
            finish(id, KIND_FAILED);
        }
    }

    // first-wins: 現在の要求だけ確定して Rust へ返す。
    private void finish(long id, int kind) {
        if (request.complete(id)) {
            launchedId = RequestTracker.NONE;
            leftId = RequestTracker.NONE;
            nativeOnWebAuthSessionResult(id, kind, null);
        }
    }

    synchronized void onPause() {
        if (launchedId != RequestTracker.NONE && launchedId == request.current()) {
            leftId = launchedId;
        }
    }

    // コールバックなしでアプリへ戻った＝キャンセル。onNewIntent は onResume より先に来るため、
    // コールバックが先に確定していれば leftId は消えていて何もしない。
    synchronized void onResume() {
        long id = leftId;
        if (id != RequestTracker.NONE) {
            finish(id, KIND_CANCELLED);
        }
    }

    // コールバック intent なら true（呼び出し側は url-scheme へ流さない）。
    synchronized boolean onNewIntent(Intent intent) {
        if (!isCallback(intent)) {
            return false;
        }
        // 旧要求のタブから届いたコールバックは、現在の追跡情報を消さずに捨てる（state で所属を照合）。
        Uri data = intent.getData();
        String cbState = data == null ? null : data.getQueryParameter("state");
        if (currentState == null || !currentState.equals(cbState)) {
            // 旧コールバックでの前面復帰に続く onResume が、現在の要求をキャンセル扱いにしないよう離脱記録を消す。
            // launchedId / currentState は維持し、現在の要求は自分のコールバックを待ち続ける。
            leftId = RequestTracker.NONE;
            return true;
        }
        long id = request.take();
        launchedId = RequestTracker.NONE;
        leftId = RequestTracker.NONE;
        currentState = null;
        if (id != RequestTracker.NONE) {
            nativeOnWebAuthSessionResult(id, KIND_URL, intent.getDataString());
        }
        return true;
    }

    // パスまでは見ない（パス違いは Rust の verify_callback が失敗にする。url-scheme へ漏らさない側に広く取る）。
    static boolean isCallback(Intent intent) {
        if (intent == null || !Intent.ACTION_VIEW.equals(intent.getAction())) {
            return false;
        }
        Uri data = intent.getData();
        return data != null && "fandhe".equalsIgnoreCase(data.getScheme())
                && "web-auth".equalsIgnoreCase(data.getAuthority());
    }

    // 範囲外は null（Rust 側が Internal にする）。
    static byte[] random(int len) {
        if (len <= 0 || len > 64) {
            return null;
        }
        byte[] b = new byte[len];
        RANDOM.nextBytes(b);
        return b;
    }

    static byte[] sha256(byte[] data) {
        try {
            return MessageDigest.getInstance("SHA-256").digest(data);
        } catch (NoSuchAlgorithmException e) {
            return null;
        }
    }
}
