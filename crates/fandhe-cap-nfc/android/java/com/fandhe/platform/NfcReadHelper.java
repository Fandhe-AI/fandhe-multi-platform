package com.fandhe.platform;

import android.app.Activity;
import android.nfc.NdefMessage;
import android.nfc.NfcAdapter;
import android.nfc.Tag;
import android.nfc.tech.Ndef;
import android.os.Handler;
import android.os.Looper;

import androidx.activity.ComponentActivity;
import androidx.lifecycle.Lifecycle;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// NFC タグ読み取り（Issue #1045、親 #157）の Android 実装ヘルパー。
// Rust の `src/android.rs` が static の `start(Activity, long, int)` / `cancel(Activity, long)` を JNI で呼び、
// 結果を `nativeOnNfcReadResult` で受けて契約（`contract`）の結果へ写像する。
// Activity ごとの helper は `HELPERS` に持ち、`NfcReadLifecycle`（`FeatureLifecycleProvider` 継承）が
// `attach` / `paused` / `detach` で知らせる。helper が無い（生成前・破棄後）`start` は CODE_FAILED で確定し、
// 結果が来ないまま待たせない。
//
// 流れ: 要求のたびに `getDefaultAdapter`（null は UNSUPPORTED）と `isEnabled`（偽は DISABLED）を OS へ問い、
// `enableReaderMode`（前面の間だけ有効。intent の配送は通らない）で最初にかざした 1 枚の
// `getCachedNdefMessage`（検出時に読んだ値。ブロックする I/O の `getNdefMessage` は使わない）を返す。
// UID（`Tag.getId`）は取得しない。
//
// スレッド: 状態の更新は main looper 1 本。`onTagDiscovered` の呼ばれるスレッドは公式に記載が無いため、
// どのスレッドでも安全なよう結果を main へ post してから確定する。
// 結果の確定は必ず `disableReaderMode` を済ませてから `finish` で 1 回だけ行う（`RequestTracker.complete`
// が id 照合の first-wins）。解除が例外を投げたら取れた結果は捨てて FAILED にし、未解放を static に記録して
// onPause / 次の要求の前に再試行する（解除できるまで次の要求は FAILED で断る）。
// ライフサイクル: `onHostPaused`（`Activity.onPause()` 内、`super.onPause()` の中で届く）で解除して INTERRUPTED で確定する
// （OS は resume で reader mode を付け直すため
// 明示的に解除する。NFC は normal 権限で許可ダイアログが無く、pause を正常な許可の中断と取り違えない）。
// `onDestroy` も INTERRUPTED。再生成後の新インスタンスは空の tracker から始まり旧結果は捨てられる。
// 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止・reader mode の解除。守らない範囲: かざしたが届く前の
// 離脱、2 枚同時にかざしたときの選択、他アプリ・OS が先にタグを処理した場合。
// タグの中身・例外の文言・id はログに出さない。
final class NfcReadHelper {
    // 結果コード。Rust 側 `android.rs::decode` が同じ値で写像する（SSOT は本クラス）。
    static final int CODE_OK = 0;
    static final int CODE_EMPTY = 1;
    static final int CODE_UNSUPPORTED = 2;
    static final int CODE_DISABLED = 3;
    static final int CODE_PERMISSION_DENIED = 4;
    static final int CODE_UNREADABLE = 5;
    static final int CODE_TOO_LARGE = 6;
    static final int CODE_INTERRUPTED = 7;
    static final int CODE_FAILED = 8;
    static final int CODE_CANCELLED = 9;

    private static final int READER_FLAGS = NfcAdapter.FLAG_READER_NFC_A
            | NfcAdapter.FLAG_READER_NFC_B
            | NfcAdapter.FLAG_READER_NFC_F
            | NfcAdapter.FLAG_READER_NFC_V;

    private static native void nativeOnNfcReadResult(long requestId, int code, byte[] ndef);

    private static final class Req {
        final long id;
        final int maxBytes;
        NfcAdapter adapter;

        Req(long id, int maxBytes) {
            this.id = id;
            this.maxBytes = maxBytes;
        }
    }

    // Activity ごとの helper。再生成後の新しい Activity は新しい helper を持ち、旧要求は持ち越さない。
    private static final Map<Activity, NfcReadHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private final ComponentActivity activity;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final RequestTracker request = new RequestTracker();
    private Req cur;
    private boolean destroyed;

    // 解除に失敗した reader mode の記録。要求より長く生きるよう static に置く（Activity 再生成後の
    // 新インスタンスも見られる）。解除が確認できるまで次の要求を受け付けない。main スレッドのみ触る。
    private static NfcAdapter unreleasedAdapter;
    private static android.app.Activity unreleasedActivity;

    private NfcReadHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // Activity の生成時（`NfcReadLifecycle` から、UI スレッド）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new NfcReadHelper(activity));
    }

    // Activity の `onPause`（同上）。reader mode を解除して保留中の要求を INTERRUPTED で確定する。
    static void paused(Activity activity) {
        NfcReadHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.onPause();
        }
    }

    // Activity の破棄時（同上）。
    static void detach(Activity activity) {
        NfcReadHelper helper = HELPERS.remove(activity);
        if (helper != null) {
            helper.onDestroy();
        }
    }

    // Rust のワーカーから JNI で呼ばれる入口。main へ post するだけ。helper が無ければ CODE_FAILED で確定する。
    static void start(Activity activity, long id, int maxBytes) {
        NfcReadHelper helper = HELPERS.get(activity);
        if (helper == null) {
            nativeOnNfcReadResult(id, CODE_FAILED, null);
            return;
        }
        helper.request.track(id);
        helper.handler.post(() -> helper.begin(new Req(id, maxBytes)));
    }

    // 期限切れで Rust のワーカーが同期で呼ぶ。main へ post するだけ。helper が無ければ何もしない
    // （`start` が既に確定している）。
    static void cancel(Activity activity, long id) {
        NfcReadHelper helper = HELPERS.get(activity);
        if (helper == null) {
            return;
        }
        helper.handler.post(() -> {
            Req r = helper.cur;
            if (r != null && r.id == id) {
                helper.end(r, CODE_CANCELLED, null);
            }
        });
    }

    private void onPause() {
        Req r = cur;
        if (r != null) {
            end(r, CODE_INTERRUPTED, null);
        }
        retryUnreleased();
    }

    private void onDestroy() {
        destroyed = true;
        onPause();
    }

    // 完了は必ずここを通す（first-wins。差し替え済み・破棄済みの旧要求は黙って捨てる）。
    private void finish(long id, int code, byte[] ndef) {
        if (request.complete(id)) {
            nativeOnNfcReadResult(id, code, ndef);
        }
    }

    // main スレッド。reader mode を解除し終えてから確定する。解除の失敗は握りつぶさず FAILED にする。
    private void end(Req r, int code, byte[] ndef) {
        if (cur == r) {
            cur = null;
        }
        if (r.adapter != null) {
            if (!release(r.adapter, activity)) {
                unreleasedAdapter = r.adapter;
                unreleasedActivity = activity;
                code = CODE_FAILED;
                ndef = null;
            }
            r.adapter = null;
        }
        finish(r.id, code, ndef);
    }

    private static boolean release(NfcAdapter adapter, android.app.Activity act) {
        try {
            adapter.disableReaderMode(act);
            return true;
        } catch (RuntimeException e) {
            return false;
        }
    }

    // 未解放の記録があれば解除を再試行する。確認できたら記録を消して true、まだなら false。
    private static boolean retryUnreleased() {
        if (unreleasedAdapter == null) {
            return true;
        }
        if (release(unreleasedAdapter, unreleasedActivity)) {
            unreleasedAdapter = null;
            unreleasedActivity = null;
            return true;
        }
        return false;
    }

    private void begin(Req r) {
        if (destroyed || !activity.getLifecycle().getCurrentState()
                .isAtLeast(Lifecycle.State.RESUMED)) {
            finish(r.id, CODE_INTERRUPTED, null);
            return;
        }
        Req prev = cur;
        if (prev != null) {
            // 旧要求の結果は tracker の id 不一致で捨てられる。
            end(prev, CODE_INTERRUPTED, null);
        }
        if (!retryUnreleased()) {
            finish(r.id, CODE_FAILED, null);
            return;
        }
        try {
            NfcAdapter adapter = NfcAdapter.getDefaultAdapter(activity);
            if (adapter == null) {
                finish(r.id, CODE_UNSUPPORTED, null);
                return;
            }
            if (!adapter.isEnabled()) {
                finish(r.id, CODE_DISABLED, null);
                return;
            }
            cur = r;
            adapter.enableReaderMode(activity, tag -> onTag(r, tag), READER_FLAGS, null);
            r.adapter = adapter;
        } catch (SecurityException e) {
            end(r, CODE_PERMISSION_DENIED, null);
        } catch (UnsupportedOperationException e) {
            end(r, CODE_UNSUPPORTED, null);
        } catch (IllegalStateException e) {
            end(r, CODE_INTERRUPTED, null);
        } catch (RuntimeException e) {
            end(r, CODE_FAILED, null);
        }
    }

    // どのスレッドから呼ばれても良いよう、読んだ結果を main へ渡して確定する。
    private void onTag(Req r, Tag tag) {
        int code;
        byte[] bytes = null;
        try {
            Ndef ndef = Ndef.get(tag);
            if (ndef == null) {
                code = CODE_UNREADABLE;
            } else {
                NdefMessage msg = ndef.getCachedNdefMessage();
                if (msg == null) {
                    // OS の値が得られなかった失敗。空 NDEF の判定は共通パーサー（0 レコード）に委ねる。
                    code = CODE_FAILED;
                } else {
                    byte[] raw = msg.toByteArray();
                    if (raw.length > r.maxBytes) {
                        code = CODE_TOO_LARGE;
                    } else {
                        code = CODE_OK;
                        bytes = raw;
                    }
                }
            }
        } catch (RuntimeException e) {
            code = CODE_FAILED;
        }
        final int c = code;
        final byte[] b = bytes;
        handler.post(() -> {
            // 2 枚目以降・旧要求は捨てる。
            if (cur == r) {
                end(r, c, b);
            }
        });
    }
}
