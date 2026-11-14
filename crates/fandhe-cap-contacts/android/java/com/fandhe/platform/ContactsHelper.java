package com.fandhe.platform;

import android.app.Activity;
import android.database.Cursor;
import android.net.Uri;
import android.provider.ContactsContract;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;

import java.util.Map;
import java.util.WeakHashMap;

// 連絡先選択（Issue #1222）の Android 実装ヘルパー。`ContactsLifecycle` が Activity ごとに生成する
// （Rust の `fandhe-cap-contacts/src/android.rs` が static メソッド `request(Activity, long)` を JNI で呼び、
// 結果を `nativeOnContactPickerResult` で受ける）。
//
// `PickContact` は `ACTION_PICK`（`Contacts.CONTENT_TYPE`）で連絡先アプリを開く。選んだ 1 件の URI 読み取り権限だけが
// 委譲されるため `READ_CONTACTS` は不要。
//
// 結果コード（`nativeOnContactPickerResult` の `code`）: 0 = 選択（name が有効）/ 1 = キャンセル /
// 3 = 失敗（`launch()` 例外・連絡先アプリ無し・読み取り失敗・名前なし・helper 無しを一律ここへ畳む。インテント経由に
// 権限拒否は無く 2 は送らない）。name は選ばれた連絡先の表示名のみで、ログには出さない。
//
// 要求 id と「連絡先アプリを開いている」フラグは static: Activity 再生成をまたいで残る。契約は「保留中の要求は
// 復元せず破棄」で、Rust 側（`android_main`）が再生成のたびに受け皿を捨てるため、再生成をまたいで届いた結果は
// Rust 側の id 照合で捨てられる。`contactPickerOpen` は旧ピッカーの結果が新インスタンスへ届くまで true のまま残し、
// 新しい起動を断る（断らないと、再生成後に始めた要求 B の id を旧要求 A の結果が消費して B の成功に見える）。
// 読み書きは UI スレッドだけで行う。
final class ContactsHelper {
    private static native void nativeOnContactPickerResult(long requestId, int code, String name);

    private static final int CODE_PICKED = 0;
    private static final int CODE_CANCELLED = 1;
    private static final int CODE_FAILED = 3;

    private static final RequestTracker contactPickRequest = new RequestTracker();
    private static boolean contactPickerOpen = false;

    private final ComponentActivity activity;
    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、`attach`（`onHostCreated`、onCreate 内）で生成する。
    // キーは従来の MainActivity の文字列のまま（再生成をまたいで保留中の結果を同じ登録へ配送するため）。
    private final ActivityResultLauncher<Void> pickContact;

    // Activity ごとの helper。`ContactsLifecycle`（`FeatureLifecycleProvider`）が生成時に付け、破棄時に外す。
    private static final Map<Activity, ContactsHelper> HELPERS = new WeakHashMap<>();

    static void attach(ComponentActivity activity) {
        // 再生成: 保留中の要求 id を捨てる。旧ピッカーの結果は新インスタンスへ届き得るが、id が NONE なので捨てられる。
        contactPickRequest.take();
        ContactsHelper helper = new ContactsHelper(activity);
        synchronized (ContactsHelper.class) {
            HELPERS.put(activity, helper);
        }
    }

    static void detach(Activity activity) {
        synchronized (ContactsHelper.class) {
            HELPERS.remove(activity);
        }
    }

    private static synchronized ContactsHelper of(Activity activity) {
        return HELPERS.get(activity);
    }

    private ContactsHelper(ComponentActivity activity) {
        this.activity = activity;
        this.pickContact = activity.getActivityResultRegistry().register(
                "fandhe:MainActivity:6", activity, new ActivityResultContracts.PickContact(), uri -> {
                    contactPickerOpen = false;
                    long requestId = contactPickRequest.take();
                    if (requestId == RequestTracker.NONE) {
                        return;
                    }
                    if (uri == null) {
                        nativeOnContactPickerResult(requestId, CODE_CANCELLED, null);
                        return;
                    }
                    // ContentResolver の読み取りを UI スレッドでやらない。
                    new Thread(() -> deliverPickedContact(requestId, uri), "fandhe-contact-pick").start();
                });
    }

    // JNI から呼ばれる入口。即時 return・結果は `nativeOnContactPickerResult`。helper が無い（生成前・破棄後）
    // ときは失敗で確定する（結果が来ないまま待たせない）。
    static void request(Activity activity, long requestId) {
        ContactsHelper helper = of(activity);
        if (helper == null) {
            nativeOnContactPickerResult(requestId, CODE_FAILED, null);
            return;
        }
        activity.runOnUiThread(() -> helper.launch(requestId));
    }

    private void launch(long requestId) {
        // 再生成前に開いた連絡先アプリの結果がまだ届いていない。その結果がこの要求の id を
        // 消費しないよう、起動せずに失敗で返す（`contactPickerOpen` 参照）。
        if (contactPickerOpen) {
            nativeOnContactPickerResult(requestId, CODE_FAILED, null);
            return;
        }
        contactPickRequest.track(requestId);
        contactPickerOpen = true;
        try {
            pickContact.launch(null);
        } catch (RuntimeException e) {
            // 連絡先アプリ無し（ActivityNotFoundException）等。コールバックが来ない経路なので
            // ここで一度だけ通知する。
            contactPickerOpen = false;
            contactPickRequest.take();
            nativeOnContactPickerResult(requestId, CODE_FAILED, null);
        }
    }

    // 選ばれた連絡先の表示名だけを読んで Rust へ渡す（ワーカースレッド）。projection は
    // DISPLAY_NAME の固定 1 列、selection は使わない。Uri・名前・例外メッセージはログへ出さない（A09）。
    private void deliverPickedContact(long requestId, Uri uri) {
        String name = null;
        try (Cursor c = activity.getContentResolver().query(
                uri, new String[] {ContactsContract.Contacts.DISPLAY_NAME}, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                name = c.getString(0);
            }
        } catch (RuntimeException e) {
            name = null;
        }
        if (name == null) {
            nativeOnContactPickerResult(requestId, CODE_FAILED, null);
        } else {
            nativeOnContactPickerResult(requestId, CODE_PICKED, name);
        }
    }
}
