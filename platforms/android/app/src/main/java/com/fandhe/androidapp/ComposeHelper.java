package com.fandhe.androidapp;

import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.net.Uri;

import androidx.activity.ComponentActivity;
import androidx.lifecycle.Lifecycle;

// メール・SMS 作成画面（Issue #1386、親 #1142）の Android 実装ヘルパー。MainActivity から
// 生成・委譲される（Rust の `device/android_compose_mail_sms.rs` が `MainActivity.requestCompose` を
// JNI で呼び、結果を `nativeOnComposeResult` で受けて `compose_mail_sms::outcome_from_android_code`
// で契約の結果へ写像する）。
//
// 公式 intents-common の「Compose an email / SMS」に従い、どちらも `ACTION_SENDTO`。
// メールは `mailto:` + EXTRA_EMAIL/CC/BCC/SUBJECT/TEXT（宛先は `\n` 連結で受け、配列へ戻す）、SMS は `smsto:`（宛先は fromParts で符号化）+ `sms_body`。
// URL は組み立てず値は extra で渡すので、`&` `?` `#` によるパラメータ注入の経路がない。
// `<queries>` と `SEND_SMS` は宣言しない: startActivity は package visibility を要さず
// （ActivityNotFoundException を扱う）、作成画面を開くだけなら送信権限は不要。
//
// 結果は「作成画面へ渡した」(HANDED_OFF) か「受け手がいない」(NO_HANDLER) のみで、送信や
// 下書き保存は分からない。非前景・破棄中は OS に黙って抑止されうるため起動せず BACKGROUND を返す。
// ライフサイクル: `request` はインスタンスフィールド。再生成後の新インスタンスは NONE から始まり、
// 旧インスタンスの結果も Rust 側 `reset_pending_tracking()` と id 照合で捨てられる。
// 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止。守らない範囲: 渡した後の送信可否。
// 宛先・件名・本文・例外メッセージはログに出さない。
final class ComposeHelper {
    static final int KIND_MAIL = 0;
    static final int KIND_SMS = 1;

    // 結果コードの SSOT。Rust 側 `outcome_from_android_code` が同じ値で写像する。
    static final int RESULT_HANDED_OFF = 0;
    static final int RESULT_NO_HANDLER = 1;
    static final int RESULT_BACKGROUND = 2;
    static final int RESULT_PRESENT_FAILED = 3;

    private static native void nativeOnComposeResult(long requestId, int code);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();

    ComposeHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // JNI スレッドから呼ばれる。
    void compose(long id, int kind, String to, String cc, String bcc, String subject,
            String body) {
        request.track(id);
        activity.runOnUiThread(() -> start(id, kind, to, cc, bcc, subject, body));
    }

    // `\n` 連結を配列へ戻す。空文字列は空配列（"".split は [""] を返すため）。
    static String[] split(String joined) {
        return joined.isEmpty() ? new String[0] : joined.split("\n");
    }

    private void start(long id, int kind, String to, String cc, String bcc, String subject,
            String body) {
        // 差し替え済みの旧要求は結果を返さず捨てる。
        if (!request.complete(id)) {
            return;
        }
        if (activity.isFinishing() || activity.isDestroyed()
                || !activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
            nativeOnComposeResult(id, RESULT_BACKGROUND);
            return;
        }
        Intent intent;
        if (kind == KIND_MAIL) {
            intent = new Intent(Intent.ACTION_SENDTO, Uri.parse("mailto:"))
                    .putExtra(Intent.EXTRA_EMAIL, split(to))
                    .putExtra(Intent.EXTRA_SUBJECT, subject)
                    .putExtra(Intent.EXTRA_TEXT, body);
            if (!cc.isEmpty()) {
                intent.putExtra(Intent.EXTRA_CC, split(cc));
            }
            if (!bcc.isEmpty()) {
                intent.putExtra(Intent.EXTRA_BCC, split(bcc));
            }
        } else if (kind == KIND_SMS) {
            intent = new Intent(Intent.ACTION_SENDTO, Uri.fromParts("smsto", to, null))
                    .putExtra("sms_body", body);
        } else {
            nativeOnComposeResult(id, RESULT_PRESENT_FAILED);
            return;
        }
        int code;
        try {
            activity.startActivity(intent);
            code = RESULT_HANDED_OFF;
        } catch (ActivityNotFoundException e) {
            code = RESULT_NO_HANDLER;
        } catch (RuntimeException e) {
            code = RESULT_PRESENT_FAILED;
        }
        nativeOnComposeResult(id, code);
    }
}
