package com.fandhe.platform;

import android.app.Activity;

import androidx.activity.ComponentActivity;
import androidx.biometric.BiometricManager;
import androidx.biometric.BiometricPrompt;
import androidx.core.content.ContextCompat;
import androidx.fragment.app.FragmentActivity;

import java.util.Map;
import java.util.WeakHashMap;

// 生体認証（Issue #908）の Android 実装ヘルパー。`BiometricAuthLifecycle` が Activity ごとに生成する
// （Rust の `fandhe-cap-biometric-auth/src/android.rs` が static メソッド `request(Activity, long, String)` を JNI で呼び、
// 結果を `nativeOnBiometricResult` で受ける）。`BiometricAuth` trait は通らない（pull 型 bridge）。
//
// code の意味（本ファイルが SSOT、Rust 側 `outcome_from_android_code` の doc と対）:
// 0 = 認証した / 1 = キャンセル / 2 = 使えない（未登録・非対応等）/ 3 = 失敗（不一致・ロックアウト等）/
// 4 = 内部エラー（提示依頼自体の失敗・helper 無し・FragmentActivity でない）。
//
// 要求 id は static（`shareRequest` と同じ理由。Activity 再生成を跨いで生存する必要がある）。`BiometricPrompt` は
// `FragmentActivity` の `FragmentManager` に紐づくためインスタンスごとに `attach`（onCreate）で作り直す。
// 公式ガイダンス上、内部 Fragment 経由で旧インスタンスへの提示結果が新インスタンスの callback へ再接続される契約のため、
// 要求 id が static であれば正しく反映される。プロセス死では `RequestTracker.NONE` に戻るため死後の再配送は無視される。
// UI スレッドのみで読み書きするため追加の同期は不要。
//
// 前面判定: `BiometricPrompt.authenticate` は `onSaveInstanceState` 済み（state-save 後）に呼ぶと例外もコールバックも
// 無しで打ち切られる（codex-review P1 指摘、PR #917）。`resumed` は `onHostResumed` で true、`onHostPaused` で false にする
// （`Activity.onPause` は `onSaveInstanceState` より必ず先に走る）。従来の `MainActivity.activityResumed` と同じ判定で、
// 届く位置だけが `super.onResume()` / `super.onPause()` の内側になる点が違う。
final class BiometricAuthHelper {
    private static native void nativeOnBiometricResult(long requestId, int code);

    private static final int BIOMETRIC_RESULT_AUTHENTICATED = 0;
    private static final int BIOMETRIC_RESULT_CANCELLED = 1;
    private static final int BIOMETRIC_RESULT_UNAVAILABLE = 2;
    private static final int BIOMETRIC_RESULT_FAILED = 3;
    private static final int BIOMETRIC_RESULT_INTERNAL_ERROR = 4;

    private static final RequestTracker biometricRequest = new RequestTracker();

    private final ComponentActivity activity;
    // `FragmentActivity` でないとき（想定外）は null。要求は内部エラーで確定する。
    private final BiometricPrompt biometricPrompt;
    private boolean resumed;

    // `biometricPrompt` 生成時に渡す唯一のコールバックインスタンス。先頭で `biometricRequest` を読んで
    // `RequestTracker.NONE` なら無視する（first-wins。二重通知防止）。`onAuthenticationFailed`（不一致・プロンプト継続）は
    // 最終結果にしない。
    private final BiometricPrompt.AuthenticationCallback biometricCallback =
            new BiometricPrompt.AuthenticationCallback() {
                @Override
                public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult result) {
                    long id = biometricRequest.take();
                    if (id == RequestTracker.NONE) {
                        return;
                    }
                    nativeOnBiometricResult(id, BIOMETRIC_RESULT_AUTHENTICATED);
                }

                @Override
                public void onAuthenticationError(int errorCode, CharSequence errString) {
                    long id = biometricRequest.take();
                    if (id == RequestTracker.NONE) {
                        return;
                    }
                    // errString はログへ出さない（画面へ渡す情報も結果コードのみに絞る、security.md A09）。
                    nativeOnBiometricResult(id, biometricErrorToResultCode(errorCode));
                }

                @Override
                public void onAuthenticationFailed() {
                    // 不一致（プロンプト継続）。最終結果は Succeeded/Error 側で確定するため、ここでは何もしない。
                }
            };

    // `BiometricPrompt.ERROR_*` → `nativeOnBiometricResult` の code への写像（本ファイルがこの表の SSOT）。
    // 未知のコード（TIMEOUT/UNABLE_TO_PROCESS/NO_SPACE/VENDOR 等）は失敗側へ倒す（偽のキャンセル・偽の成功を作らない）。
    private static int biometricErrorToResultCode(int errorCode) {
        switch (errorCode) {
            case BiometricPrompt.ERROR_USER_CANCELED:
            case BiometricPrompt.ERROR_NEGATIVE_BUTTON:
            case BiometricPrompt.ERROR_CANCELED:
                return BIOMETRIC_RESULT_CANCELLED;
            case BiometricPrompt.ERROR_LOCKOUT:
            case BiometricPrompt.ERROR_LOCKOUT_PERMANENT:
                return BIOMETRIC_RESULT_FAILED;
            case BiometricPrompt.ERROR_NO_BIOMETRICS:
            case BiometricPrompt.ERROR_HW_NOT_PRESENT:
            case BiometricPrompt.ERROR_HW_UNAVAILABLE:
            case BiometricPrompt.ERROR_SECURITY_UPDATE_REQUIRED:
            case BiometricPrompt.ERROR_NO_DEVICE_CREDENTIAL:
                return BIOMETRIC_RESULT_UNAVAILABLE;
            default:
                return BIOMETRIC_RESULT_FAILED;
        }
    }

    // Activity ごとの helper。`BiometricAuthLifecycle`（`FeatureLifecycleProvider`）が生成時に付け、破棄時に外す。
    private static final Map<Activity, BiometricAuthHelper> HELPERS = new WeakHashMap<>();

    static void attach(ComponentActivity activity) {
        BiometricAuthHelper helper = new BiometricAuthHelper(activity);
        // Rust 側 `android_main` は `bridge::reset()` で保留中の要求を無条件に捨て、新しい画面状態（`Idle`）を作り直す
        // （codex-review 指摘、PR #917）。`biometricRequest` は static のため Activity 再生成をまたいで残り、
        // `BiometricPrompt` も `FragmentManager` 経由で旧認証操作へ再接続され得るため、放置すると新しい prompt の上に
        // 「宛先を失った」認証が動き続けてしまう。ここで明示的に打ち切り、対になる Rust 側の破棄と揃える。
        // `cancelAuthentication()` は `onAuthenticationError`（`ERROR_CANCELED`）を呼ぶため `biometricRequest` は通常の
        // コールバック経路で `NONE` に戻る（結果は Rust 側が既に破棄済みのため無視される）。
        if (helper.biometricPrompt != null && biometricRequest.current() != RequestTracker.NONE) {
            helper.biometricPrompt.cancelAuthentication();
            // 新しい helper の prompt では cancel が no-op になり得る。id を残すと以降の tryTrack が失敗し続けるため確実に解放する。
            biometricRequest.take();
        }
        synchronized (BiometricAuthHelper.class) {
            HELPERS.put(activity, helper);
        }
    }

    static void detach(Activity activity) {
        synchronized (BiometricAuthHelper.class) {
            HELPERS.remove(activity);
        }
    }

    static void setResumed(Activity activity, boolean resumed) {
        BiometricAuthHelper helper = of(activity);
        if (helper != null) {
            helper.resumed = resumed;
        }
    }

    private static synchronized BiometricAuthHelper of(Activity activity) {
        return HELPERS.get(activity);
    }

    private BiometricAuthHelper(ComponentActivity activity) {
        this.activity = activity;
        this.biometricPrompt = activity instanceof FragmentActivity
                ? new BiometricPrompt((FragmentActivity) activity, ContextCompat.getMainExecutor(activity), biometricCallback)
                : null;
    }

    // JNI から呼ばれる入口（即時 return・結果は非同期通知）。`reason` は Rust 側 `validate_reason`（空・NUL 拒否）を通した
    // 非空文字列の契約。表示にのみ使い、ログへ出さない（security.md A03/A09）。helper が無いときは失敗で確定する。
    static void request(Activity activity, long requestId, String reason) {
        BiometricAuthHelper helper = of(activity);
        if (helper == null) {
            nativeOnBiometricResult(requestId, BIOMETRIC_RESULT_INTERNAL_ERROR);
            return;
        }
        // 進行中の認証があれば新しい要求は拒否する（id を上書きすると旧認証の完了が新要求の成功になる）。
        if (!biometricRequest.tryTrack(requestId)) {
            nativeOnBiometricResult(requestId, BIOMETRIC_RESULT_INTERNAL_ERROR);
            return;
        }
        activity.runOnUiThread(() -> helper.present(requestId, reason));
    }

    private void present(long requestId, String reason) {
        // `isFinishing()`/`isDestroyed()` は破棄処理中しか検出できず、`onSaveInstanceState` 済み（state-save 後）の
        // resumed でない状態はすり抜ける。`resumed` は `onPause` で false に落ちるため、ここで見れば state-save 後の呼び出しを
        // 確実に弾ける。
        if (biometricPrompt == null || activity.isFinishing() || activity.isDestroyed() || !resumed) {
            biometricRequest.complete(requestId);
            nativeOnBiometricResult(requestId, BIOMETRIC_RESULT_INTERNAL_ERROR);
            return;
        }
        if (BiometricManager.from(activity).canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG)
                != BiometricManager.BIOMETRIC_SUCCESS) {
            // 未登録・非対応ハードウェア・OS 側要件未達等（可否判定の失敗を「認証した」にしない）。
            biometricRequest.complete(requestId);
            nativeOnBiometricResult(requestId, BIOMETRIC_RESULT_UNAVAILABLE);
            return;
        }
        try {
            BiometricPrompt.PromptInfo promptInfo = new BiometricPrompt.PromptInfo.Builder()
                    .setTitle(reason)
                    .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
                    // `DEVICE_CREDENTIAL` を許可しないため negative button は必須（OS 標準文字列を使い新規リソースは足さない）。
                    .setNegativeButtonText(activity.getString(android.R.string.cancel))
                    .build();
            biometricPrompt.authenticate(promptInfo);
        } catch (RuntimeException e) {
            // 提示依頼自体が例外を送出した場合。コールバックは二度と来ないため、ここで直接内部エラーを通知する。
            biometricRequest.complete(requestId);
            nativeOnBiometricResult(requestId, BIOMETRIC_RESULT_INTERNAL_ERROR);
        }
    }
}
