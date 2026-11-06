package com.fandhe.platform;

import android.app.Activity;
import android.app.Application;
import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;

import androidx.activity.ComponentActivity;

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
// Rust を載せた Activity（`GameActivity` のサブクラス）の生成・状態保存・破棄だけをサブクラスの
// `onHost*` へ流す。他の Activity（Health Connect の理由画面など）には helper を付けない。
// `onHostCreated` は `Activity.onCreate` の中（`ComponentActivity` の保存状態の復元後・STARTED より前）で
// 呼ばれる。`registerForActivityResult` を呼べるのは STARTED より前だけなので、ランチャーの登録はここで行う。
// 登録キーは必ず明示する（`getActivityResultRegistry().register(key, owner, contract, cb)`）。連番キーだと
// 生成順が変わるだけで再生成後の結果が別のランチャーへ届く。
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
    public void onActivityStarted(Activity activity) {}

    @Override
    public void onActivityResumed(Activity activity) {}

    @Override
    public void onActivityPaused(Activity activity) {}

    @Override
    public void onActivityStopped(Activity activity) {}

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
