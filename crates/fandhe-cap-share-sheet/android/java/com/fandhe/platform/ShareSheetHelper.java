package com.fandhe.platform;

import android.app.Activity;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// 共有シート（Issue #899、親 #896、#1302 で MainActivity から移動）の Android 実装ヘルパー。範囲は
// `Intent.createChooser` を出し、選択完了を `IntentSender` の broadcast で検出して Rust へ返すこと。
// 利用者の `MainActivity` には何も書かせない。Rust の `src/android.rs` が static の
// `share(Activity, id, text, url)` を JNI で呼び、結果を `nativeOnShareResult` で受けて
// `driver::bridge::deliver` へ渡す。Activity への参加は `ShareSheetLifecycle`（非公開 ContentProvider、
// `FeatureLifecycleProvider` 継承）が行い、生成時に `attach`（ランチャー登録）、破棄時に `detach` を呼ぶ。
// Activity ごとの helper は `HELPERS` に持つ（Rust のワーカーと UI スレッドの両方から触るので同期化し、
// 破棄時に必ず外す）。表に無い Activity への要求は false を返し、Rust が失敗として確定する。
//
// 結果コードの SSOT は Rust 側 `driver::outcome_from_code`（iOS / macOS / Android 共通の表）。
// Android で使うのは 0=共有した（選択 broadcast 受信）／3=内部エラー（`launch()` 例外等）／
// 4=結果不明（chooser は閉じたが選択 broadcast が届かなかった）。1（キャンセル）・2（共有先なし）は使わない。
//
// chooser は選択結果を返さない（`StartActivityForResult` の結果は常に `RESULT_CANCELED`）ため、
// ランチャーの結果は「chooser が閉じた」ことしか意味しない。選択完了は broadcast が別途届ける。
// broadcast は chooser の finish より先に送られるが非同期のため、ランチャーの結果が先に届いたときは
// `SHARE_CHOSEN_GRACE_MS` だけ遅らせて broadcast の遅延到着を待ってから「不明」を確定する。
//
// 状態の持ち方（Activity 再生成を跨ぐため static）: 要求 id は `shareRequest`（`RequestTracker`）、
// 受信器は Application Context へ登録する `shareChosenReceiver`。Activity 単位の receiver は再生成の
// `onDestroy` で消えてしまうため。要求ごとに一意な action にして、遅延した旧要求の broadcast が
// 新しい要求の受信器へ届かないようにする（PR #903）。自パッケージ限定（`setPackage`）で外部からの偽装を防ぐ。
// 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止・受信器の解除。守らない範囲: chooser 表示中の
// プロセス死・複数プロセス・共有先アプリが選択 broadcast を返さない場合（結果不明で返す）。
// 共有テキスト・URL・例外メッセージはログに出さない（security.md A09）。
final class ShareSheetHelper {
    private static final long SHARE_CHOSEN_GRACE_MS = 500L;

    // 選択完了 broadcast の action プレフィックス。要求 ID ごとに一意にする（上記コメント参照）。
    private static final String SHARE_CHOSEN_ACTION_PREFIX = "com.fandhe.platform.SHARE_CHOSEN.";

    private static final String KEY_CHOOSER = "fandhe.share-sheet.chooser";

    // Activity ごとの helper（上記コメント参照）。
    private static final Map<Activity, ShareSheetHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private static final RequestTracker shareRequest = new RequestTracker();

    // UI スレッドでのみ読み書きする。
    private static BroadcastReceiver shareChosenReceiver;

    private static native void nativeOnShareResult(long requestId, int code);

    private final ComponentActivity activity;
    // ランチャーの登録は STARTED より前に呼ぶ契約のため、`attach`（`Activity.onCreate` 内）で生成する。
    private final ActivityResultLauncher<Intent> launcher;

    private ShareSheetHelper(ComponentActivity activity) {
        this.activity = activity;
        this.launcher = activity.getActivityResultRegistry().register(
                KEY_CHOOSER, activity, new ActivityResultContracts.StartActivityForResult(),
                result -> onChooserClosed());
    }

    // Activity の生成時（`ShareSheetLifecycle` から）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new ShareSheetHelper(activity));
    }

    // Activity の破棄時（`ShareSheetLifecycle` から）。受信器は再生成を跨いで生きるため、ここでは外さない。
    static void detach(Activity activity) {
        HELPERS.remove(activity);
    }

    // JNI スレッド（Rust のワーカー / UI）から呼ばれる入口。helper が無い（生成前・破棄後・対象外の
    // Activity）なら false を返し、Rust が失敗として確定する（結果の native は呼ばれない）。
    // `url` は null なら URL 無し。
    static boolean share(Activity activity, long requestId, String text, String url) {
        ShareSheetHelper helper = HELPERS.get(activity);
        if (helper == null) {
            return false;
        }
        helper.begin(requestId, text, url);
        return true;
    }

    private static String shareChosenAction(long requestId) {
        return SHARE_CHOSEN_ACTION_PREFIX + requestId;
    }

    private void begin(long requestId, String text, String url) {
        shareRequest.track(requestId);
        activity.runOnUiThread(() -> {
            try {
                registerShareChosenReceiver(requestId);

                Intent send = new Intent(Intent.ACTION_SEND);
                send.setType("text/plain");
                send.putExtra(Intent.EXTRA_TEXT, url == null ? text : text + "\n" + url);

                // `IntentSender`（API 22+、minSdk 24 で利用可）で選択完了 broadcast を受け取る。
                // `setPackage(自パッケージ)` で自プロセス以外からの偽装 broadcast を受理しない
                // （security.md A01/A05）。API 31+ は system が extras を書き込むため `FLAG_MUTABLE` が必須
                // （API < 31 では未知ビットとして無視される）。
                Intent chosenIntent =
                        new Intent(shareChosenAction(requestId)).setPackage(activity.getPackageName());
                int flags = PendingIntent.FLAG_UPDATE_CURRENT
                        | (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ? PendingIntent.FLAG_MUTABLE : 0);
                PendingIntent pi = PendingIntent.getBroadcast(activity, 0, chosenIntent, flags);

                Intent chooser = Intent.createChooser(send, null, pi.getIntentSender());
                launcher.launch(chooser);
            } catch (RuntimeException e) {
                // `launch()` 自体が例外を送出した場合。chooser は開かれておらずコールバックも来ないため、
                // ここで直接 code 3（内部エラー）を通知する。
                shareRequest.take();
                unregisterShareChosenReceiver();
                nativeOnShareResult(requestId, 3);
            }
        });
    }

    // chooser が閉じたとき。broadcast が先に届いて消費済みなら何もしない。未消費なら猶予後に「不明」を確定する。
    private void onChooserClosed() {
        long id = shareRequest.current();
        if (id == RequestTracker.NONE) {
            return;
        }
        new Handler(Looper.getMainLooper()).postDelayed(() -> {
            if (!shareRequest.complete(id)) {
                // broadcast が遅延到着で先に消費済み（空振り）。
                return;
            }
            unregisterShareChosenReceiver();
            nativeOnShareResult(id, 4);
        }, SHARE_CHOSEN_GRACE_MS);
    }

    // 選択完了 broadcast の受信器を Application Context へ登録する（要求ごと）。`RECEIVER_NOT_EXPORTED` で
    // 他アプリからの直接送信を遮断する。
    private void registerShareChosenReceiver(long requestId) {
        unregisterShareChosenReceiver();
        shareChosenReceiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                // `requestId` はこの受信器を登録した時点の要求。一致するときだけ処理する（二重チェック）。
                if (!shareRequest.complete(requestId)) {
                    return;
                }
                unregisterShareChosenReceiver();
                nativeOnShareResult(requestId, 0);
            }
        };
        ContextCompat.registerReceiver(
                activity.getApplicationContext(),
                shareChosenReceiver,
                new IntentFilter(shareChosenAction(requestId)),
                ContextCompat.RECEIVER_NOT_EXPORTED);
    }

    // どの決着経路からも呼ぶ後始末（二重登録・リーク防止）。未登録（null）なら何もしない。
    private void unregisterShareChosenReceiver() {
        if (shareChosenReceiver == null) {
            return;
        }
        try {
            activity.getApplicationContext().unregisterReceiver(shareChosenReceiver);
        } catch (IllegalArgumentException e) {
            // 未登録状態での unregister（二重呼び出し等）。無害なため握りつぶす。
        }
        shareChosenReceiver = null;
    }
}
