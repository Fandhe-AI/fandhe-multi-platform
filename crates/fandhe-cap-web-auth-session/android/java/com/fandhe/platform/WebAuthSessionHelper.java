package com.fandhe.platform;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.net.Uri;

import androidx.activity.ComponentActivity;
import androidx.browser.customtabs.CustomTabsIntent;
import androidx.lifecycle.Lifecycle;

import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.security.SecureRandom;
import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// OAuth 用 Web 認証セッション（Issue #534、親 #113）の Android 実装ヘルパー。`WebAuthSessionLifecycle`
// （非公開 provider、Issue #1333）が Activity ごとに生成して `HELPERS` に持つ。Rust の
// `crates/fandhe-cap-web-auth-session/src/android.rs` が static の `requestStart` 等を JNI で呼び（利用者の
// `MainActivity` に受け口は要らない）、結果を `nativeOnWebAuthSessionResult` で受けて `web_auth_session::bridge::complete_from_os`
// へ渡す（コールバック URL の検査は Rust 側 `verify_callback` に一本化）。
//
// 流れ: Custom Tabs で認可ページ（デモは 127.0.0.1）を開く（開けなければ ACTION_VIEW）→ ブラウザが
// `fandhe://web-auth/callback?...` へ遷移 → 既存の intent-filter + singleTask により
// `onNewIntent` へ届き、`attach` が足した `addOnNewIntentListener`（起動中のコールバックだけ要るため共通層の
// `deliverIntents` は使わない）で確定する。url-scheme 受信箱へは流さない（code・state を別画面に出さない。
// `UrlSchemeHelper.deliver` が同じ条件で除外する）。コールバックなしで戻った（onPause 後の onResume）はキャンセル。
// 従来の `MainActivity.onNewIntent` は `setIntent` を呼ばずに返していたため、code を含む intent は残らない（本経路も保持しない）。
// 期限（300 秒）は Rust 側の単調時計だけが持ち、ここには持たない。
//
// コールバックは開始 URL の state と一致する現在の要求にだけ確定する（不一致は破棄）。
//
// 結果コードの SSOT は KIND_*（Rust 側の写像と同じ値）。
// ライフサイクル（引き継ぎ型）: 要求の状態（追跡 id・離脱記録・state）は Activity ではなくプロセスの static に持つ。
// 認証中に Activity が再生成（回転等）されても、コールバックは新しい Activity の `onNewIntent` へ届く
// （MainActivity は singleTask。公式文書 Tasks and the back stack: 既存インスタンスがあれば `onNewIntent()` へ routing、
// 構成変更は同じ Activity の破棄と再生成）。新しい helper が同じ状態を見るので、再生成をまたいで結果を 1 度だけ確定できる。
// Rust 側の保留（`bridge::ACTIVE`・`SLOT`）も残したままにするので、呼び出し側の掃除は要らない。
// 結果が一度も返らない場合（Activity が再生成されずに終了した・プロセス死）は、Rust 側の期限（300 秒）が失敗として確定する。
// 旧要求の遅延結果は id 照合で捨てる。
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

    private static final Map<Activity, WebAuthSessionHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    // 要求の状態は static（引き継ぎ型）。Activity ごとの helper はこれを共有し、LOCK で守る。
    private static final Object LOCK = new Object();
    private static final RequestTracker request = new RequestTracker();
    // 起動を OS に渡した要求 / 起動後に onPause を挟んだ要求。
    private static long launchedId = RequestTracker.NONE;
    private static long leftId = RequestTracker.NONE;
    // 現在の要求の開始 URL に含まれる state。コールバックの所属照合に使う。
    private static String currentState = null;

    private final ComponentActivity activity;

    private WebAuthSessionHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // Activity の生成時（`WebAuthSessionLifecycle.onHostCreated`）。要求の状態は static なので、再生成後の新 helper も
    // 認証中の要求を引き継いで、`onNewIntent` のコールバックを確定できる。
    static void attach(ComponentActivity activity) {
        WebAuthSessionHelper helper = new WebAuthSessionHelper(activity);
        HELPERS.put(activity, helper);
        activity.addOnNewIntentListener(helper::onNewIntent);
    }

    // Activity の破棄時。以後の要求は拒否される（要求が無ければ何もしない）。
    static void detach(Activity activity) {
        HELPERS.remove(activity);
    }

    // Rust から呼ぶ入口（JNI スレッド）。helper が無い（生成前・破棄後）要求は受理せず false（失敗で確定させる）。
    static boolean requestStart(Activity activity, long id, String url) {
        WebAuthSessionHelper helper = HELPERS.get(activity);
        if (helper == null) {
            return false;
        }
        helper.start(id, url);
        return true;
    }

    static void requestCancel(Activity activity, long id) {
        WebAuthSessionHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.cancel(id);
        }
    }

    // `WebAuthSessionLifecycle` から（UI スレッド）。`Activity.onPause()` / `onResume()` の中（`super` の中）で届く。
    // 従来の `MainActivity` は `super` の後で呼んでいたため、`super` に対する前後だけが違う。
    static void paused(Activity activity) {
        WebAuthSessionHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.onPause();
        }
    }

    static void resumed(Activity activity) {
        WebAuthSessionHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.onResume();
        }
    }

    // JNI スレッドから呼ばれる。
    void start(long id, String url) {
        synchronized (LOCK) {
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
        synchronized (LOCK) {
            if (request.complete(id)) {
                launchedId = RequestTracker.NONE;
                leftId = RequestTracker.NONE;
            }
        }
    }

    private void launch(long id, String url) {
        synchronized (LOCK) {
            launchLocked(id, url);
        }
    }

    private void launchLocked(long id, String url) {
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

    // first-wins: 現在の要求だけ確定して Rust へ返す（LOCK を持って呼ぶ）。
    private void finish(long id, int kind) {
        if (request.complete(id)) {
            launchedId = RequestTracker.NONE;
            leftId = RequestTracker.NONE;
            nativeOnWebAuthSessionResult(id, kind, null);
        }
    }

    void onPause() {
        synchronized (LOCK) {
            if (launchedId != RequestTracker.NONE && launchedId == request.current()) {
                leftId = launchedId;
            }
        }
    }

    // コールバックなしでアプリへ戻った＝キャンセル。onNewIntent は onResume より先に来るため、
    // コールバックが先に確定していれば leftId は消えていて何もしない。
    void onResume() {
        synchronized (LOCK) {
            long id = leftId;
            if (id != RequestTracker.NONE) {
                finish(id, KIND_CANCELLED);
            }
        }
    }

    // コールバック intent なら true（呼び出し側は url-scheme へ流さない）。
    boolean onNewIntent(Intent intent) {
        synchronized (LOCK) {
            return onNewIntentLocked(intent);
        }
    }

    private boolean onNewIntentLocked(Intent intent) {
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
