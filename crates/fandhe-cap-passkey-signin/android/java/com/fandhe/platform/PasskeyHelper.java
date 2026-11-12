package com.fandhe.platform;

import android.os.CancellationSignal;
import android.os.Handler;
import android.os.Looper;

import androidx.activity.ComponentActivity;
import androidx.core.content.ContextCompat;
import androidx.credentials.CreateCredentialRequest;
import androidx.credentials.CreateCredentialResponse;
import androidx.credentials.CreatePublicKeyCredentialRequest;
import androidx.credentials.CreatePublicKeyCredentialResponse;
import androidx.credentials.Credential;
import androidx.credentials.CredentialManager;
import androidx.credentials.CredentialManagerCallback;
import androidx.credentials.GetCredentialRequest;
import androidx.credentials.GetCredentialResponse;
import androidx.credentials.GetPublicKeyCredentialOption;
import androidx.credentials.PublicKeyCredential;
import androidx.credentials.exceptions.CreateCredentialCancellationException;
import androidx.credentials.exceptions.CreateCredentialException;
import androidx.credentials.exceptions.CreateCredentialNoCreateOptionException;
import androidx.credentials.exceptions.CreateCredentialProviderConfigurationException;
import androidx.credentials.exceptions.CreateCredentialUnsupportedException;
import androidx.credentials.exceptions.GetCredentialCancellationException;
import androidx.credentials.exceptions.GetCredentialException;
import androidx.credentials.exceptions.GetCredentialProviderConfigurationException;
import androidx.credentials.exceptions.GetCredentialUnsupportedException;
import androidx.credentials.exceptions.NoCredentialException;
import androidx.credentials.exceptions.domerrors.AbortError;
import androidx.credentials.exceptions.domerrors.ConstraintError;
import androidx.credentials.exceptions.domerrors.DomError;
import androidx.credentials.exceptions.domerrors.InvalidStateError;
import androidx.credentials.exceptions.domerrors.NotAllowedError;
import androidx.credentials.exceptions.domerrors.NotSupportedError;
import androidx.credentials.exceptions.domerrors.SecurityError;
import androidx.credentials.exceptions.publickeycredential.CreatePublicKeyCredentialDomException;
import androidx.credentials.exceptions.publickeycredential.GetPublicKeyCredentialDomException;
import androidx.lifecycle.Lifecycle;

import java.lang.ref.WeakReference;
import java.security.SecureRandom;

// パスキー（WebAuthn）の登録・サインイン要求（Issue #1280、親 #92 の b-android）の Android 実装ヘルパー。
// 範囲は Credential Manager でパスキーの登録とサインイン要求を出し、結果の種別と本文を Rust へ返すこと。
// `PasskeyLifecycle` が Activity ごとに生成する。Rust の `fandhe-cap-passkey-signin/src/android.rs` が同期 JNI の static メソッド `random(int)`
// で乱数を得て、static メソッド `request(activity, id, register, requestJson)` を JNI で呼び、結果を
// `nativeOnPasskeyResult(id, kind, payload)` で受けて `passkey::bridge::deliver` へ渡す。
//
// 要求 JSON の組み立てと応答の照合（challenge・type・UP/UV/AT・署名の有無）は Rust だけで行い、ここへ
// 定数を複製しない。ここは OS の応答を (kind, payload) に分けるだけ。KIND_* は `android_passkey.rs` の
// 定数と同じ値（SSOT はこの Java 側）。DOM 例外名は `getDomError()` を instanceof で判定して作る
// （クラス名の文字列化は R8 で壊れるため使わない）。Rust 側の写像表は `outcome_from_dom_error`（SSOT）。
//
// ライフサイクル: Activity は WeakReference で持つ。onPause / onStop では取り消さない（Credential Manager の UI が
// 自身を pause させる）。onDestroy で CancellationSignal を取り消し、追跡を外して保留中の要求は KIND_INTERRUPTED で確定する。再生成後の新
// インスタンスは NONE から始まり、Rust 側 reset_pending_tracking() と対で旧要求を捨てる。OS の UI を出す直前に
// 要求の世代と Activity の状態（RESUMED・isFinishing・isDestroyed）を見直す。
// 守る範囲: 1 プロセス内の要求と完了の順序・嘘の成功の禁止。守らない範囲: プロセス死をまたぐ結果・
// Credential Manager のコールバックが返らない場合・複数プロセス。期限は掛けない（利用者操作の UI）。
// challenge・user.id・応答 JSON・例外メッセージはログに出さない。
// ロック順: this のロック内で完結し、他のロックを取らない。Rust 側は自分のロックの内側から Java を呼ばない。
final class PasskeyHelper {
    static final int KIND_RESPONSE = 0;
    static final int KIND_NOT_ALLOWED = 1;
    static final int KIND_DOM_ERROR = 2;
    static final int KIND_UNSUPPORTED = 3;
    static final int KIND_FAILED = 4;
    // OS の応答前に Activity が破棄された（待っている呼び出しを残さない）。Rust 側は Internal へ写す。
    static final int KIND_INTERRUPTED = 5;

    private static final SecureRandom RANDOM = new SecureRandom();

    private static native void nativeOnPasskeyResult(long requestId, int kind, String payload);

    private final WeakReference<ComponentActivity> activityRef;
    private final RequestTracker request = new RequestTracker();
    private final Handler uiHandler = new Handler(Looper.getMainLooper());

    // 以下は this のロックで守る。
    private CancellationSignal signal;
    private boolean destroyed;

    // Activity ごとの helper。`PasskeyLifecycle`（`FeatureLifecycleProvider`）が生成時に付け、破棄時に外して OS の UI を取り消す。
    private static final java.util.Map<android.app.Activity, PasskeyHelper> HELPERS =
            new java.util.WeakHashMap<>();

    static synchronized void attach(ComponentActivity activity) {
        HELPERS.put(activity, new PasskeyHelper(activity));
    }

    static void detach(android.app.Activity activity) {
        PasskeyHelper helper;
        synchronized (PasskeyHelper.class) {
            helper = HELPERS.remove(activity);
        }
        if (helper != null) {
            helper.onDestroy();
        }
    }

    // JNI スレッドから呼ばれる入口。helper が無いときは失敗で確定する（結果が来ないまま待たせない）。
    static void request(android.app.Activity activity, long id, boolean register, String requestJson) {
        PasskeyHelper helper;
        synchronized (PasskeyHelper.class) {
            helper = HELPERS.get(activity);
        }
        if (helper == null) {
            nativeOnPasskeyResult(id, KIND_FAILED, null);
            return;
        }
        helper.request(id, register, requestJson);
    }

    private PasskeyHelper(ComponentActivity activity) {
        this.activityRef = new WeakReference<>(activity);
    }

    // challenge と user.id の素材。fandhe-device は乱数を作らない契約のため OS 側の CSPRNG で作る。
    static byte[] random(int len) {
        if (len <= 0 || len > 256) {
            return null;
        }
        byte[] b = new byte[len];
        RANDOM.nextBytes(b);
        return b;
    }

    // JNI スレッドから呼ばれる（static 入口 `request(Activity, ...)` 経由）。
    private void request(long id, boolean register, String requestJson) {
        synchronized (this) {
            request.track(id);
        }
        uiHandler.post(() -> start(id, register, requestJson));
    }

    private synchronized void start(long id, boolean register, String requestJson) {
        if (request.current() != id || destroyed) {
            return;
        }
        ComponentActivity activity = activityRef.get();
        // 背景で OS の UI を出さない。
        if (activity == null || activity.isFinishing() || activity.isDestroyed()
                || !activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            finish(id, KIND_FAILED, null);
            return;
        }
        try {
            CredentialManager manager = CredentialManager.create(activity);
            CancellationSignal s = new CancellationSignal();
            signal = s;
            if (register) {
                manager.createCredentialAsync(activity,
                        (CreateCredentialRequest) new CreatePublicKeyCredentialRequest(requestJson), s,
                        ContextCompat.getMainExecutor(activity), new CreateCallback(id));
            } else {
                manager.getCredentialAsync(activity,
                        new GetCredentialRequest.Builder()
                                .addCredentialOption(new GetPublicKeyCredentialOption(requestJson))
                                .build(),
                        s, ContextCompat.getMainExecutor(activity), new GetCallback(id));
            }
        } catch (RuntimeException e) {
            finish(id, KIND_FAILED, null);
        }
    }

    private final class CreateCallback
            implements CredentialManagerCallback<CreateCredentialResponse, CreateCredentialException> {
        private final long id;

        CreateCallback(long id) {
            this.id = id;
        }

        @Override
        public void onResult(CreateCredentialResponse response) {
            if (response instanceof CreatePublicKeyCredentialResponse) {
                done(id, KIND_RESPONSE,
                        ((CreatePublicKeyCredentialResponse) response).getRegistrationResponseJson());
            } else {
                done(id, KIND_FAILED, null);
            }
        }

        @Override
        public void onError(CreateCredentialException e) {
            if (e instanceof CreateCredentialCancellationException) {
                done(id, KIND_NOT_ALLOWED, null);
            } else if (e instanceof CreatePublicKeyCredentialDomException) {
                done(id, KIND_DOM_ERROR, domName(((CreatePublicKeyCredentialDomException) e).getDomError()));
            } else if (e instanceof CreateCredentialProviderConfigurationException
                    || e instanceof CreateCredentialUnsupportedException
                    || e instanceof CreateCredentialNoCreateOptionException) {
                done(id, KIND_UNSUPPORTED, null);
            } else {
                done(id, KIND_FAILED, null);
            }
        }
    }

    private final class GetCallback
            implements CredentialManagerCallback<GetCredentialResponse, GetCredentialException> {
        private final long id;

        GetCallback(long id) {
            this.id = id;
        }

        @Override
        public void onResult(GetCredentialResponse response) {
            Credential credential = response.getCredential();
            if (credential instanceof PublicKeyCredential) {
                done(id, KIND_RESPONSE, ((PublicKeyCredential) credential).getAuthenticationResponseJson());
            } else {
                done(id, KIND_FAILED, null);
            }
        }

        @Override
        public void onError(GetCredentialException e) {
            if (e instanceof GetCredentialCancellationException || e instanceof NoCredentialException) {
                done(id, KIND_NOT_ALLOWED, null);
            } else if (e instanceof GetPublicKeyCredentialDomException) {
                done(id, KIND_DOM_ERROR, domName(((GetPublicKeyCredentialDomException) e).getDomError()));
            } else if (e instanceof GetCredentialProviderConfigurationException
                    || e instanceof GetCredentialUnsupportedException) {
                done(id, KIND_UNSUPPORTED, null);
            } else {
                done(id, KIND_FAILED, null);
            }
        }
    }

    // DOM 例外名。Rust の `outcome_from_dom_error` が受ける名前だけを返し、それ以外は空（Rust 側で失敗になる）。
    private static String domName(DomError e) {
        if (e instanceof NotAllowedError) return "NotAllowedError";
        if (e instanceof AbortError) return "AbortError";
        if (e instanceof NotSupportedError) return "NotSupportedError";
        if (e instanceof SecurityError) return "SecurityError";
        if (e instanceof ConstraintError) return "ConstraintError";
        if (e instanceof InvalidStateError) return "InvalidStateError";
        return "";
    }

    // コールバック（メインスレッド）。再開後に現在の要求かを照合し直す。
    private synchronized void done(long id, int kind, String payload) {
        if (request.current() != id || destroyed) {
            return;
        }
        signal = null;
        finish(id, kind, payload);
    }

    // first-wins: complete に成功したときだけ Rust へ送る。
    private void finish(long id, int kind, String payload) {
        if (!request.complete(id)) {
            return;
        }
        nativeOnPasskeyResult(id, kind, payload);
    }

    // Activity 破棄。OS の UI を取り消し、追跡を外す。保留中の要求があれば KIND_INTERRUPTED で確定する
    // （待っている呼び出しを残さない）。`take` は first-wins のため、結果を届け済みの要求には二重に届けない。
    private synchronized void onDestroy() {
        destroyed = true;
        long id = request.take();
        if (signal != null) {
            signal.cancel();
            signal = null;
        }
        if (id != RequestTracker.NONE) {
            nativeOnPasskeyResult(id, KIND_INTERRUPTED, null);
        }
    }
}
