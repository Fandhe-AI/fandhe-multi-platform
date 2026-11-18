package com.fandhe.platform;

import android.content.Intent;
import android.net.Uri;

// カスタム URL scheme 受信（Issue #1218）の Android 実装ヘルパー。
//
// 呼び出し元: `UrlSchemeLifecycle`（非公開 provider。起動時と `onNewIntent` を共通層の `deliverIntents` で配る、#1325）。
// 利用者の `MainActivity` には何も書かせない。
// 呼び出し先: `crates/fandhe-cap-url-scheme/src/android.rs` の JNI 実体。
// JNI 署名はこの宣言と Rust 実体の 2 か所で `(String) -> void` に一致させる
// （本 crate を選んだアプリにだけ入るので no-op スタブは要らない、#1194）。
//
// `ACTION_VIEW` のときだけ URL を Rust へ渡す（MAIN/LAUNCHER 起動は拒否件数に混ぜない）。他機能の経路は渡さない:
//   - https の App Links は deep-link の受信箱へ（`DeepLinkLifecycle`）
//   - `fandhe://web-auth` のコールバックは web-auth-session が確定する（code・state を本受信箱へ流さない、Issue #534）
// 上の 2 つの判定は他機能の Java に依存せず、同じ条件をここに持つ（機能 crate 同士を Java で結ばない。
// web-auth の条件は `WebAuthSessionHelper.isCallback`、https の条件は `DeepLinkHelper.deliver` と一致させる）。
// null の data はそのまま渡し、Rust 側が拒否として数える。検査（scheme・長さ・制御文字）は Rust に一本化し、
// ここでは文字列を加工しない。URL はトークンを含みうるためログへ出さない。状態を持たない。
final class UrlSchemeHelper {
    private UrlSchemeHelper() {}

    private static native void nativeOnUrlSchemeReceived(String url);

    static void deliver(Intent intent) {
        if (intent == null || !Intent.ACTION_VIEW.equals(intent.getAction())) {
            return;
        }
        Uri data = intent.getData();
        if (data != null) {
            String scheme = data.getScheme();
            if ("https".equalsIgnoreCase(scheme)) {
                return;
            }
            if ("fandhe".equalsIgnoreCase(scheme) && "web-auth".equalsIgnoreCase(data.getAuthority())) {
                return;
            }
        }
        nativeOnUrlSchemeReceived(intent.getDataString());
    }
}
