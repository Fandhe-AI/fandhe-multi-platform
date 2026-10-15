package com.fandhe.androidapp;

import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.Intent;
import android.net.Uri;

import androidx.activity.ComponentActivity;
import androidx.core.content.FileProvider;
import androidx.lifecycle.Lifecycle;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;

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
//
// 添付（Issue #32、メールのみ・1 件）: URL では渡せないため `ACTION_SEND` + `EXTRA_STREAM`
// （`FileProvider` の content URI）にし、`setSelector(ACTION_SENDTO mailto:)` でメールアプリに絞る。
// 一時ファイルは `getCacheDir()/compose/<検証済みファイル名>` だけ（FileProvider の paths もここに限る）。
// 書き込み前に `compose/` を空にするので残るのは最新 1 件。書き込み失敗と startActivity の例外では
// その場で消し、HANDED_OFF では受け手が後から読むため残す（次回の添付要求と OS の cacheDir 掃除で消える）。
// 既知の天井: 受け手がコピーする前に再度添付要求をすると前の添付が読めなくなる（最大 1 件・1 MiB）。
// 一部のメールアプリは selector 経由の EXTRA_STREAM を無視しうる。読み取り権限は Intent 単位の
// FLAG_GRANT_READ_URI_PERMISSION だけで渡す。
// 宛先・件名・本文・添付・例外メッセージはログに出さない。
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
            String body, byte[] attData, String attMime, String attName) {
        request.track(id);
        activity.runOnUiThread(
                () -> start(id, kind, to, cc, bcc, subject, body, attData, attMime, attName));
    }

    // `\n` 連結を配列へ戻す。空文字列は空配列（"".split は [""] を返すため）。
    static String[] split(String joined) {
        return joined.isEmpty() ? new String[0] : joined.split("\n");
    }

    // `compose/` の既存ファイルを全部消してから 1 件書く。`name` は Rust 側で `[A-Za-z0-9._-]` に
    // 検証済みだが、正規化後も `compose/` 直下にあることを確かめる（多重防御）。失敗は null。
    private File writeAttachment(byte[] data, String name) {
        File dir = new File(activity.getCacheDir(), "compose");
        if (!dir.isDirectory() && !dir.mkdirs()) {
            return null;
        }
        File[] old = dir.listFiles();
        if (old != null) {
            for (File f : old) {
                f.delete();
            }
        }
        File f = new File(dir, name);
        try {
            if (!dir.getCanonicalFile().equals(f.getCanonicalFile().getParentFile())) {
                return null;
            }
            // ponytail: UI スレッドでの書き込みは最大 1 MiB。体感できる遅さが出たら作業スレッドへ移す。
            try (FileOutputStream out = new FileOutputStream(f)) {
                out.write(data);
            }
        } catch (IOException e) {
            f.delete();
            return null;
        }
        return f;
    }

    private void start(long id, int kind, String to, String cc, String bcc, String subject,
            String body, byte[] attData, String attMime, String attName) {
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
        File attFile = null;
        if (kind == KIND_MAIL && attData.length > 0) {
            attFile = writeAttachment(attData, attName);
            if (attFile == null) {
                nativeOnComposeResult(id, RESULT_PRESENT_FAILED);
                return;
            }
            Uri uri;
            try {
                uri = FileProvider.getUriForFile(activity,
                        activity.getPackageName() + ".compose", attFile);
            } catch (IllegalArgumentException e) {
                attFile.delete();
                nativeOnComposeResult(id, RESULT_PRESENT_FAILED);
                return;
            }
            intent = new Intent(Intent.ACTION_SEND)
                    .setType(attMime)
                    .putExtra(Intent.EXTRA_EMAIL, split(to))
                    .putExtra(Intent.EXTRA_SUBJECT, subject)
                    .putExtra(Intent.EXTRA_TEXT, body)
                    .putExtra(Intent.EXTRA_STREAM, uri)
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
            intent.setClipData(ClipData.newRawUri("", uri));
            if (!cc.isEmpty()) {
                intent.putExtra(Intent.EXTRA_CC, split(cc));
            }
            if (!bcc.isEmpty()) {
                intent.putExtra(Intent.EXTRA_BCC, split(bcc));
            }
            intent.setSelector(new Intent(Intent.ACTION_SENDTO, Uri.parse("mailto:")));
        } else if (kind == KIND_MAIL) {
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
        if (attFile != null && code != RESULT_HANDED_OFF) {
            attFile.delete();
        }
        nativeOnComposeResult(id, code);
    }
}
