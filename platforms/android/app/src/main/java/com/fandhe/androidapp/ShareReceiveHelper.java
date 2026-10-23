package com.fandhe.androidapp;

import android.content.Intent;

// 共有の受け取り（Issue #607、親 #122）。他アプリの共有シートから届いたテキスト 1 件を
// Rust 側の受け口へ渡す。
//
// 呼び出し元: `MainActivity` の `onCreate`（コールドスタート）と `onNewIntent`（起動中）。
// 呼び出し先: `crates/fandhe-egui-proxy/src/device/android_share_receive.rs` の JNI 実体
// （feature `device` 無効時の no-op は `android.rs`）。JNI 署名は本ファイルの宣言・no-op
// スタブ・Rust 実体の 3 か所で `(String) -> void` に一致させる。
//
// 検査（長さ・制御文字・UTF-8）は Rust 側の共通層に一本化し、ここでは文字列を加工しない
// （trim もしない）。受け取るのは `EXTRA_TEXT` だけで、`EXTRA_STREAM`・`ClipData`・件名は
// 読まない（他アプリのファイルへ届く URI 読み取り許可を使わない）。本文・例外メッセージは
// ログへ出さない。状態を持たず UI スレッドから呼ばれる。
// 守らない範囲: プロセス死亡後の再表示（新プロセスの受け口は空）。
// 切り離し時に削除する（feature `device`）。
final class ShareReceiveHelper {
    private ShareReceiveHelper() {}

    private static native void nativeOnShareReceived(String text);

    // `ACTION_SEND` / `ACTION_SEND_MULTIPLE` のときだけ Rust へ渡す。MAIN・VIEW 等は拒否件数に
    // 混ぜず無視する。受け取れない形（複数・text/plain 以外・ストリーム付き・不正な Parcel）は
    // null を渡し、Rust 側が拒否として件数だけ数える。native 呼び出しは 1 回だけにする。
    static void deliver(Intent intent) {
        if (intent == null) {
            return;
        }
        String action = intent.getAction();
        if (Intent.ACTION_SEND_MULTIPLE.equals(action)) {
            nativeOnShareReceived(null);
            return;
        }
        if (!Intent.ACTION_SEND.equals(action)) {
            return;
        }
        String text = null;
        try {
            // 明示インテントは intent-filter を迂回できるため、型もここで見る。
            boolean plain = "text/plain".equals(Intent.normalizeMimeType(intent.getType()));
            if (plain && !intent.hasExtra(Intent.EXTRA_STREAM)) {
                CharSequence cs = intent.getCharSequenceExtra(Intent.EXTRA_TEXT);
                text = cs == null ? null : cs.toString();
            }
        } catch (RuntimeException e) {
            // 不正な Parcel の展開失敗など。内容はログに出さず拒否 1 件にする。
            text = null;
        }
        nativeOnShareReceived(text);
    }
}
