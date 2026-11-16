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
// 結果を `nativeOnBiometricResult` で受ける）。画面の経路は `BiometricAuth` trait を通らない（pull 型 bridge）。公開入口も同じ要求 id の追跡を使う。
//
// code の意味（本ファイルが SSOT、Rust 側 `outcome_from_android_code` の doc と対）:
// 0 = 認証した / 1 = キャンセル（利用者の操作 `ERROR_USER_CANCELED`・`ERROR_NEGATIVE_BUTTON` のみ）/ 2 = 使えない（未登録・非対応等）/ 3 = 失敗（不一致・ロックアウト等）/
// 4 = 内部エラー（提示依頼自体の失敗・helper 無し・FragmentActivity でない）/
// 5 = 中断（OS 起因の取り消し `ERROR_CANCELED`、または再生成を伴わない Activity 破棄。利用者の操作ではない。Rust 側は `Err(Internal)`）。
//
// 要求 id は static（Activity 再生成を跨いで生存する必要がある）。`BiometricPrompt` は `FragmentActivity` の
// `FragmentManager` に紐づくためインスタンスごとに `attach`（onCreate）で作り直す。
//
// 再生成と prompt（androidx.biometric 1.1.0 のソースで確認）:
// - 認証中の prompt は設定変更をまたいで生き残り、`BiometricPrompt` クラス doc は回転時に `cancelAuthentication()` を
//   呼ばないよう求める。結果は Activity の `onCreate` で最後に作った `BiometricPrompt` の callback へ、届ける時点で
//   読んで届く（結果に prompt の識別子は付かない）。
// - そのため再生成では何も取り消さない。新 helper は `attach` で追跡中の id を自分の `promptId` として引き継ぎ、
//   生き残った prompt の結果を `finish` が id 照合付きで届ける。tracker は 1 件しか持たず、`tryTrack` は追跡中なら
//   拒否するので、その prompt が終わるまで別の要求は始まらず、結果を取り違えない。
// - Rust 側は再生成で `reset_pending_tracking()` が待ち手を `Internal` で戻し、引き継いだ prompt の結果は旧 id として捨てる。
// - 再生成を伴わない破棄では prompt の宛先ごと消えるため、`detach` が中断（5）で確定し tracker を空ける。
// 残る限界: prompt（再生成をまたいで生き残ったものを含む）に OS が結果を一度も返さないと、その間の次の要求は内部エラーで断られる
// （固定タイムアウトは足さない。再生成を伴わない破棄では `detach` が解放する）。
//
// UI スレッドで読み書きする。`promptId` は JNI を呼ぶスレッド（`request`）からも書くため volatile。
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
    private static final int BIOMETRIC_RESULT_INTERRUPTED = 5;

    private static final RequestTracker biometricRequest = new RequestTracker();

    private final ComponentActivity activity;
    // `FragmentActivity` でないとき（想定外）は null。要求は内部エラーで確定する。
    private final BiometricPrompt biometricPrompt;
    private boolean resumed;
    // この helper が結果を確定する責任を持つ要求の id。`request` が `tryTrack` に成功した直後に設定する（`present` 前の
    // 破棄でも `detach` が確定できる）。再生成後の新 helper は `attach` で追跡中の id を引き継ぐ。
    // 確定は照合付き `complete` なので二重に届かない。
    private volatile long promptId = RequestTracker.NONE;

    // `biometricPrompt` 生成時に渡す唯一のコールバックインスタンス。`finish` が id を照合して 1 度だけ確定する
    // （first-wins。二重通知防止）。`onAuthenticationFailed`（不一致・プロンプト継続）は最終結果にしない。
    private final BiometricPrompt.AuthenticationCallback biometricCallback =
            new BiometricPrompt.AuthenticationCallback() {
                @Override
                public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult result) {
                    finish(BIOMETRIC_RESULT_AUTHENTICATED);
                }

                @Override
                public void onAuthenticationError(int errorCode, CharSequence errString) {
                    // errString はログへ出さない（画面へ渡す情報も結果コードのみに絞る、security.md A09）。
                    finish(biometricErrorToResultCode(errorCode));
                }

                @Override
                public void onAuthenticationFailed() {
                    // 不一致（プロンプト継続）。最終結果は Succeeded/Error 側で確定するため、ここでは何もしない。
                }
            };

    // この helper の要求が今も待っている要求のときだけ結果を 1 度届ける（first-wins）。
    // callback と `detach` の二重通知は `complete` が false を返して no-op になる。
    private void finish(int code) {
        long id = promptId;
        if (id != RequestTracker.NONE && biometricRequest.complete(id)) {
            nativeOnBiometricResult(id, code);
        }
    }

    // `BiometricPrompt.ERROR_*` → `nativeOnBiometricResult` の code への写像（本ファイルがこの表の SSOT）。
    // 未知のコード（TIMEOUT/UNABLE_TO_PROCESS/NO_SPACE/VENDOR 等）は失敗側へ倒す（偽のキャンセル・偽の成功を作らない）。
    private static int biometricErrorToResultCode(int errorCode) {
        switch (errorCode) {
            case BiometricPrompt.ERROR_USER_CANCELED:
            case BiometricPrompt.ERROR_NEGATIVE_BUTTON:
                return BIOMETRIC_RESULT_CANCELLED;
            // OS 起因の取り消し（センサー不可・ユーザー切替・ロック等）。利用者の操作ではない。
            case BiometricPrompt.ERROR_CANCELED:
                return BIOMETRIC_RESULT_INTERRUPTED;
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
        // 再生成の直前の要求が生き残っていれば、その prompt の結果はこの helper の callback へ来る。id を引き継ぐ。
        long inFlight = biometricRequest.current();
        if (helper.biometricPrompt != null && inFlight != RequestTracker.NONE) {
            helper.promptId = inFlight;
        }
        synchronized (BiometricAuthHelper.class) {
            HELPERS.put(activity, helper);
        }
    }

    // Activity 破棄。再生成（`isChangingConfigurations()`）なら何もしない: prompt は新 Activity へ引き継がれる。
    // 再生成を伴わない破棄では prompt の宛先ごと消えるため、この helper の要求を中断（5）として 1 度だけ確定し、
    // tracker を空ける。照合付き `complete` のため、後続の要求や callback との二重通知にはならない。
    static void detach(Activity activity) {
        BiometricAuthHelper helper;
        synchronized (BiometricAuthHelper.class) {
            helper = HELPERS.remove(activity);
        }
        if (helper != null && !activity.isChangingConfigurations()) {
            helper.finish(BIOMETRIC_RESULT_INTERRUPTED);
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
        // 進行中の認証（再生成で生き残った prompt を含む）があれば新しい要求は拒否する
        // （id を上書きすると旧認証の完了が新要求の成功になる）。
        if (!biometricRequest.tryTrack(requestId)) {
            nativeOnBiometricResult(requestId, BIOMETRIC_RESULT_INTERNAL_ERROR);
            return;
        }
        helper.promptId = requestId;
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
