package com.fandhe.platform;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.os.Handler;
import android.os.Looper;

// クリップボード（Issue #859、Phase 11 型サンプル「クリップボード」）の Android 実装ヘルパー。
// 利用者の `MainActivity` には何も書かせない。Rust の `src/android.rs` が egui スレッドから static の
// `request(Context, long, int, String)` を JNI で呼び、結果を `nativeOnClipboardResult` で受けて
// 契約（`contract`）の結果へ写像する。
//
// `android_main` は GameActivity 経路では Java の UI スレッドとは別のネイティブスレッドで走るため、
// `ClipboardManager` の操作を main Looper へ post し、結果を `nativeOnClipboardResult` で返す（Rust 側は RequestSlot で待つ）。
// `ClipboardManager` を UI スレッドで扱うのは Android の作法（`getPrimaryClip` は Android 10 以降アプリのフォーカス状態に依存し、
// OEM 実装によっては Looper を持たないスレッドからの取得が失敗する）。
//
// 状態も保留中の要求も持たない（要求 id は Rust 側が照合し、Activity 再生成後に届いた旧結果は id 不一致で捨てられる）。
// Context は `getSystemService` の取得にだけ使い、保持しない（post した Runnable が実行されるまでの間だけ参照する）。
//
// クリップボードの内容はそのまま native 側へ渡すだけで、ログ出力・保存は一切行わない
// （クリップボードは他アプリのパスワード等が載り得る機密経路のため痕跡を残さない）。
final class ClipboardHelper {
    // 要求の種別。Rust 側 `android.rs` の `CLIPBOARD_OP_READ` / `CLIPBOARD_OP_WRITE` と対（SSOT は本クラス）。
    static final int OP_READ = 0;
    static final int OP_WRITE = 1;

    // 状態コード。Rust 側 `decode_clipboard_result` が同じ値で写像する。
    // 0=成功／1=クリップボードにテキストが無い／2=内部エラー。
    static final int STATUS_OK = 0;
    static final int STATUS_NO_TEXT = 1;
    static final int STATUS_FAILED = 2;

    // requestId: `request` が受け取った値をそのまま返す（Rust 側が ID 一致を検査し、
    // タイムアウト後の遅延到着を新要求の結果と取り違えないため）。
    // text: 読み出し結果（書き込み要求・テキスト不在では null）。
    private static native void nativeOnClipboardResult(long requestId, String text, int status);

    private static final Handler MAIN = new Handler(Looper.getMainLooper());

    private ClipboardHelper() {}

    // Rust のワーカーから JNI で呼ばれる入口。main Looper へ post するだけ。
    static void request(Context activity, long requestId, int op, String text) {
        MAIN.post(() -> run(activity, requestId, op, text));
    }

    private static void run(Context activity, long requestId, int op, String text) {
        ClipboardManager clipboard =
                (ClipboardManager) activity.getSystemService(Context.CLIPBOARD_SERVICE);
        if (clipboard == null) {
            nativeOnClipboardResult(requestId, null, STATUS_FAILED);
            return;
        }
        try {
            if (op == OP_WRITE) {
                clipboard.setPrimaryClip(ClipData.newPlainText(null, text));
                nativeOnClipboardResult(requestId, null, STATUS_OK);
                return;
            }
            if (op != OP_READ) {
                // 未知の op（Rust 側との契約違反）。内容を推測せず内部エラーにする。
                nativeOnClipboardResult(requestId, null, STATUS_FAILED);
                return;
            }
            ClipData clip = clipboard.getPrimaryClip();
            if (clip == null || clip.getItemCount() == 0) {
                nativeOnClipboardResult(requestId, null, STATUS_NO_TEXT);
                return;
            }
            // `getText()` でテキストそのものだけを取る。`coerceToText` は Intent を `intent:` URI へ、
            // Uri を文字列へ強制変換するため、契約の「非テキスト内容は Ok(None)」と食い違う
            // （呼び出し側がテキストか否かを区別できなくなる。codex-review 指摘 P1、PR #860）。
            // テキストが無ければ STATUS_NO_TEXT。
            CharSequence value = clip.getItemAt(0).getText();
            if (value == null || value.length() == 0) {
                nativeOnClipboardResult(requestId, null, STATUS_NO_TEXT);
                return;
            }
            nativeOnClipboardResult(requestId, value.toString(), STATUS_OK);
        } catch (RuntimeException e) {
            // SecurityException（フォーカス無しでの読み取り拒否）等を取りこぼさず Rust 側へ内部エラーとして返す
            // （例外メッセージはクリップボード内容を含み得るためログ出力しない）。
            nativeOnClipboardResult(requestId, null, STATUS_FAILED);
        }
    }
}
