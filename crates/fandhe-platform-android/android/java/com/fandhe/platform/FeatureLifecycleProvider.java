package com.fandhe.platform;

import android.app.Activity;
import android.app.Application;
import android.content.ContentProvider;
import android.content.ContentValues;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;

import androidx.activity.ComponentActivity;
import androidx.core.util.Consumer;

import com.google.androidgamesdk.GameActivity;

// 機能 crate が Activity のライフサイクルへ参加するための共通の土台（Issue #1257、TASK-21.3）。
//
// 目的: 機能の追加・削除を依存の追加・削除だけで済ませる。これまで機能の helper は利用者の
// `MainActivity` のフィールド初期化子・`onCreate`・`onSaveInstanceState`・`onDestroy` から手で配線され、
// 機能を選ぶたびに利用者の Activity を書き換える必要があった。機能 crate はこのクラスを継承した
// 非公開の ContentProvider を 1 つ宣言し（`[[package.metadata.fandhe.android.components]]`）、
// Activity への参加をその中で完結させる。provider はアプリのプロセス開始時（`Application.onCreate` の前）に
// OS が生成するので、利用者のコードは何も呼ばない。
//
// 動き: provider の `onCreate` で `Application.registerActivityLifecycleCallbacks` に自分を登録し、
// Rust を載せた Activity（`GameActivity` のサブクラス）の生成・状態保存・前面への復帰・前面離脱・破棄だけをサブクラスの
// `onHost*` へ流す。他の Activity（Health Connect の理由画面など）には helper を付けない。
// `onHostCreated` は `Activity.onCreate` の中（`ComponentActivity` の保存状態の復元後・STARTED より前）で
// 呼ばれる。`registerForActivityResult` を呼べるのは STARTED より前だけなので、ランチャーの登録はここで行う。
// 登録キーは必ず明示する（`getActivityResultRegistry().register(key, owner, contract, cb)`）。連番キーだと
// 生成順が変わるだけで再生成後の結果が別のランチャーへ届く。
//
// Intent の配送（Issue #1325）: 起動時の Intent と起動中の `onNewIntent` を機能が受けたいときは、
// `onHostCreated` から `deliverIntents` を呼ぶ。利用者の `MainActivity` に配線を書かせない。
//
// provider としては何も公開しない（データを返さず、`exported=false`）。呼び出し元は OS だけ。
// ponytail: 機能 1 つにつき provider 1 つ。機能が数十に増えて起動時間が問題になれば、
// androidx.startup 型の単一 provider とメタデータ列挙へ寄せる。
public abstract class FeatureLifecycleProvider extends ContentProvider
        implements Application.ActivityLifecycleCallbacks {

    /** Activity の生成時（`Activity.onCreate` 内）。ランチャーの登録と状態の復元をここで行う。 */
    protected void onHostCreated(ComponentActivity activity, Bundle savedInstanceState) {}

    /** Activity の状態保存時。 */
    protected void onHostSaveInstanceState(ComponentActivity activity, Bundle outState) {}

    /** Activity の破棄時。helper の表から外し、保持している資源を解放する。 */
    protected void onHostDestroyed(ComponentActivity activity) {}

    /**
     * Activity が前面へ戻った時（`Activity.onResume()` の中、つまり `super.onResume()` の中）。
     * 従来の `MainActivity.onResume` は `super.onResume()` の後で helper を呼んでいた。super に対する
     * 前後だけが違い、同じ `onResume` の内側で呼ばれる。既定は何もしない。
     */
    protected void onHostResumed(ComponentActivity activity) {}

    /**
     * Activity が見えるようになった時（`Activity.onStart()` の中、つまり `super.onStart()` の中）。
     * 従来の `MainActivity.onStart` は `super.onStart()` の後で helper を呼んでいた。super に対する前後だけが違い、
     * 同じ `onStart` の内側で呼ばれる。既定は何もしない。
     */
    protected void onHostStarted(ComponentActivity activity) {}

    /**
     * Activity が見えなくなった時（`Activity.onStop()` の中、つまり `super.onStop()` の中）。
     * 従来の `MainActivity.onStop` は `super.onStop()` の前に helper を呼んでいた。同じ `onStop` の内側で呼ばれるため、
     * `isChangingConfigurations()` の判定など Activity の状態を読む処理は変わらずここで行える。既定は何もしない。
     */
    protected void onHostStopped(ComponentActivity activity) {}

    /**
     * Activity が前面を離れる時（`Activity.onPause()` の中、つまり `super.onPause()` の中）。
     * 従来の `MainActivity.onPause` は `super.onPause()` の後で helper を呼んでいた。同じ `onPause` の内側で
     * 呼ばれるため、権限ダイアログの応答との到着順（契約表 1）はこの通知を受ける側で実際の状態を見て判断する。
     * 既定は何もしない。
     */
    protected void onHostPaused(ComponentActivity activity) {}

    /**
     * 起動時の Intent と起動中の `onNewIntent` を `sink` へ配る。`onHostCreated` の中から呼ぶ（Issue #1325）。
     *
     * <ul>
     *   <li>起動時: `savedInstanceState == null` かつ `FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY` 無しのときだけ 1 回配る。
     *       再生成と履歴（Recents）からの起動は元の Intent が再送されるため配らない（二重配送の防止）。</li>
     *   <li>起動中: `ComponentActivity.addOnNewIntentListener` で受ける。`super.onNewIntent` の中で届き、
     *       利用者の `onNewIntent` 本体より先。リスナーは Activity に付き、破棄で一緒に消える（再生成で重複しない）。</li>
     *   <li>同じ Intent が登録した全機能へ届く。消費して後続を止める口は無い。各 `sink` が action・scheme・host で
     *       自分の分だけ拾う（拾う条件は機能どうしで互いに素にする）。</li>
     *   <li>Intent は他アプリが送れるので信頼しない。検査は `sink` と Rust 側で行い、内容はログへ出さない。
     *       `setIntent` は呼ばない。</li>
     * </ul>
     *
     * 起動中の分だけ要る機能は本メソッドを使わず `activity.addOnNewIntentListener` を直接使う。
     */
    protected static void deliverIntents(ComponentActivity activity, Bundle savedInstanceState,
            Consumer<Intent> sink) {
        activity.addOnNewIntentListener(sink);
        Intent intent = activity.getIntent();
        if (savedInstanceState == null
                && (intent.getFlags() & Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) == 0) {
            sink.accept(intent);
        }
    }

    @Override
    public boolean onCreate() {
        android.content.Context context = getContext();
        Application app = context == null ? null : (Application) context.getApplicationContext();
        if (app == null) {
            return false;
        }
        app.registerActivityLifecycleCallbacks(this);
        return true;
    }

    // Rust を載せた Activity だけが対象。
    private static boolean isHost(Activity activity) {
        return activity instanceof GameActivity && activity instanceof ComponentActivity;
    }

    @Override
    public void onActivityCreated(Activity activity, Bundle savedInstanceState) {
        if (isHost(activity)) {
            onHostCreated((ComponentActivity) activity, savedInstanceState);
        }
    }

    @Override
    public void onActivitySaveInstanceState(Activity activity, Bundle outState) {
        if (isHost(activity)) {
            onHostSaveInstanceState((ComponentActivity) activity, outState);
        }
    }

    @Override
    public void onActivityDestroyed(Activity activity) {
        if (isHost(activity)) {
            onHostDestroyed((ComponentActivity) activity);
        }
    }

    @Override
    public void onActivityStarted(Activity activity) {
        if (isHost(activity)) {
            onHostStarted((ComponentActivity) activity);
        }
    }

    @Override
    public void onActivityResumed(Activity activity) {
        if (isHost(activity)) {
            onHostResumed((ComponentActivity) activity);
        }
    }

    @Override
    public void onActivityPaused(Activity activity) {
        if (isHost(activity)) {
            onHostPaused((ComponentActivity) activity);
        }
    }

    @Override
    public void onActivityStopped(Activity activity) {
        if (isHost(activity)) {
            onHostStopped((ComponentActivity) activity);
        }
    }

    // 以下は ContentProvider の必須実装。データは持たず、何も返さない。

    @Override
    public Cursor query(Uri uri, String[] projection, String selection, String[] selectionArgs,
            String sortOrder) {
        return null;
    }

    @Override
    public String getType(Uri uri) {
        return null;
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        return null;
    }

    @Override
    public int delete(Uri uri, String selection, String[] selectionArgs) {
        return 0;
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection, String[] selectionArgs) {
        return 0;
    }
}
