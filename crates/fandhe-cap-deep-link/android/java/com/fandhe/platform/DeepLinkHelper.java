package com.fandhe.platform;

import android.content.Intent;
import android.net.Uri;

// ディープリンク（App Links、Issue #1279、親 #153）の Android 実装ヘルパー。
// `DeepLinkLifecycle` が共通層の `deliverIntents` で起動時と `onNewIntent` を配り、ACTION_VIEW のうち scheme が https の
// ものだけを Rust の deep-link 受信箱へ渡す（それ以外は url-scheme 側が受信箱へ渡す。`UrlSchemeHelper` は https を除く）。
// Rust 側の実体は `crates/fandhe-cap-deep-link/src/android.rs`
// （feature `device` 無効時の no-op は `crates/fandhe-gallery/src/lib.rs`）。JNI 署名はこの宣言・no-op・実体の 3 か所で一致させる。
//
// 検査（https・許可ホストの完全一致・長さ・制御文字）は Rust に一本化しており、ここでは
// 文字列を加工しない。二重配送の防ぎ方は url-scheme と同じで、呼び出し側が
// 再生成・履歴起動のときは呼ばない。URL はトークンを含みうるためログへ出さない。
final class DeepLinkHelper {
    private DeepLinkHelper() {}

    // ACTION_VIEW かつ https の intent なら Rust へ渡す。それ以外は何もしない。
    static void deliver(Intent intent) {
        if (intent == null || !Intent.ACTION_VIEW.equals(intent.getAction())) {
            return;
        }
        Uri data = intent.getData();
        if (data == null || !"https".equalsIgnoreCase(data.getScheme())) {
            return;
        }
        nativeOnDeepLinkReceived(intent.getDataString());
    }

    private static native void nativeOnDeepLinkReceived(String url);
}
