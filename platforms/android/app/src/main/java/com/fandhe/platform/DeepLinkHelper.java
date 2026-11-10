package com.fandhe.platform;

import android.content.Intent;
import android.net.Uri;

// ディープリンク（App Links、Issue #1279、親 #153）の Android 実装ヘルパー。
// `MainActivity.deliverUrlIntent` が ACTION_VIEW のたびに呼び、scheme が https のものだけを
// Rust の deep-link 受信箱へ渡す（それ以外は呼び出し側が url-scheme 受信箱へ渡す）。
// Rust 側の実体は `crates/fandhe-egui-proxy/src/device/android_deep_link.rs`
// （feature `device` 無効時の no-op は `android.rs`）。JNI 署名はこの宣言・no-op・実体の 3 か所で一致させる。
//
// 検査（https・許可ホストの完全一致・長さ・制御文字）は Rust に一本化しており、ここでは
// 文字列を加工しない。二重配送の防ぎ方は url-scheme と同じで、呼び出し側が
// 再生成・履歴起動のときは呼ばない。URL はトークンを含みうるためログへ出さない。
final class DeepLinkHelper {
    private DeepLinkHelper() {}

    // https の intent なら Rust へ渡して true、それ以外は何もせず false。
    static boolean deliver(Intent intent) {
        Uri data = intent.getData();
        if (data == null || !"https".equalsIgnoreCase(data.getScheme())) {
            return false;
        }
        nativeOnDeepLinkReceived(intent.getDataString());
        return true;
    }

    private static native void nativeOnDeepLinkReceived(String url);
}
