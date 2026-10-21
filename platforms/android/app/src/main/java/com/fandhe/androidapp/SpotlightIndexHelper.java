package com.fandhe.androidapp;

import android.app.appsearch.AppSearchBatchResult;
import android.app.appsearch.AppSearchManager;
import android.app.appsearch.AppSearchResult;
import android.app.appsearch.AppSearchSchema;
import android.app.appsearch.AppSearchSession;
import android.app.appsearch.BatchResultCallback;
import android.app.appsearch.GenericDocument;
import android.app.appsearch.PutDocumentsRequest;
import android.app.appsearch.SearchSpec;
import android.app.appsearch.SetSchemaRequest;
import android.content.Context;
import android.os.Build;
import android.os.SystemClock;

import androidx.annotation.NonNull;
import androidx.annotation.RequiresApi;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Executor;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicBoolean;

// OS の検索への登録（Issue #562、親 #117 の b-android）の Android 実装ヘルパー。
// Rust の `device/android_spotlight_index.rs` がドライバのワーカースレッドから
// `MainActivity.spotlightIndex` / `spotlightRemoveAll` 経由で同期 JNI で呼ぶ。
// Java から Rust への native コールバックは無く、JNI 署名の照合先は Java の 1 か所だけ。
// AppSearch の platform storage（framework の `android.app.appsearch`、API 31 以上）に、
// 固定メモ 1 件の登録と自ドメイン（namespace）の全削除だけを行う。Android に受信経路は無い。
//
// コード表の SSOT はここで、Rust 側が同じ値で解釈する。
//   0=成功 / 1=非対応（API 31 未満・サービス無し） / 2=期限切れ / 3=失敗
//   成功は OS が成功を返したときだけ。部分失敗・例外・未知の状態はすべて 3。
//
// 期限: Rust のドライバだけが期限を持つ。ここでは `remainingMs` を超えて待たない（`Future.get`）。
// 期限切れで先に戻っても、後から届いた終端で session を閉じる。
// 順序: 単一スレッドの EXECUTOR はコールバック単位の直列化で、要求全体の順序は保証しない。
// そこで (1) 期限切れで戻るときは `cancelled` を立て、まだ発送していない OS 書き込み
// （setSchema 後の put・remove）を行わずに終端させる、(2) 次の要求は前の要求の終端
// （`inFlight`）を期限内で待ってから始める。発送済みの書き込みは取り消せない
// （AppSearch に取り消し API は無い）ので、その完了を待つことで後続の更新が巻き戻されないようにする。
// ライフサイクル: 要求ごとの状態を持たず `applicationContext` だけを使う。Activity が破棄されても
// `remainingMs` 以内に戻り、古い結果は Rust のドライバが要求 id の不一致で捨てる。
//
// 守る範囲: 失敗を成功に畳まないこと・全終端での session 解放・待ちの上限・
// 1 プロセス内での要求どうしの順序（前の要求の OS 書き込みが後の要求を追い越さない）。
// 守らない範囲: ランチャー・システム検索に出るか（OEM 次第）・反映の時間差・
// 期限後に OS 側で済んでいた場合との食い違い。
// id・題名・本文・例外メッセージはログに出さない。他パッケージへの公開は設定しない。
final class SpotlightIndexHelper {
    private static final int OK = 0, UNSUPPORTED = 1, TIMEOUT = 2, FAILED = 3;
    private static final String TYPE = "FandheMemo";
    private static final Executor EXECUTOR = Executors.newSingleThreadExecutor();
    private static final Object LOCK = new Object();
    /** 直近に始めた要求の終端。次の要求はこれが完了してから始める（LOCK の内側で読み書き）。 */
    private static CompletableFuture<Integer> inFlight;

    private SpotlightIndexHelper() {}

    static int index(Context ctx, String domain, String id, String title, String text, long remainingMs) {
        if (remainingMs <= 0) return TIMEOUT;
        if (ctx == null || domain == null || id == null || title == null || text == null) return FAILED;
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return UNSUPPORTED;
        return indexS(ctx, domain, id, title, text, remainingMs);
    }

    static int removeAll(Context ctx, String domain, long remainingMs) {
        if (remainingMs <= 0) return TIMEOUT;
        if (ctx == null || domain == null) return FAILED;
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return UNSUPPORTED;
        return removeAllS(ctx, domain, remainingMs);
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private static int indexS(
            Context ctx, String domain, String id, String title, String text, long remainingMs) {
        return run(ctx, domain, remainingMs, (session, f, cancelled) -> {
            AppSearchSchema schema = new AppSearchSchema.Builder(TYPE)
                    .addProperty(stringProp("title"))
                    .addProperty(stringProp("text"))
                    .build();
            // システム UI への表示は既定でも許されるが、この機能の目的なので明示する。
            SetSchemaRequest req = new SetSchemaRequest.Builder()
                    .addSchemas(schema)
                    .setSchemaTypeDisplayedBySystem(TYPE, true)
                    .build();
            session.setSchema(req,
                    EXECUTOR, EXECUTOR, schemaResult -> {
                        // 非同期コールバック内の例外も失敗で確定し session を閉じる（run の try は届かない）。
                        try {
                            if (!schemaResult.isSuccess()) {
                                finish(session, f, FAILED);
                                return;
                            }
                            // 期限切れ後は未発送の書き込みを行わない（後続の全削除を巻き戻さないため）。
                            if (cancelled.get()) {
                                finish(session, f, TIMEOUT);
                                return;
                            }
                            GenericDocument doc = new GenericDocument.Builder<>(domain, id, TYPE)
                                    .setPropertyString("title", title)
                                    .setPropertyString("text", text)
                                    .build();
                            session.put(new PutDocumentsRequest.Builder().addGenericDocuments(doc).build(),
                                    EXECUTOR, new BatchResultCallback<String, Void>() {
                                        @Override
                                        public void onResult(@NonNull AppSearchBatchResult<String, Void> r) {
                                            finish(session, f, r.isSuccess() ? OK : FAILED);
                                        }

                                        @Override
                                        public void onSystemError(@NonNull Throwable t) {
                                            finish(session, f, FAILED);
                                        }
                                    });
                        } catch (RuntimeException e) {
                            finish(session, f, FAILED);
                        }
                    });
        });
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private static int removeAllS(Context ctx, String domain, long remainingMs) {
        return run(ctx, domain, remainingMs, (session, f, cancelled) -> {
            // クエリは定数（空 = 全件）。namespace で自ドメインに絞る。入力から組み立てない。
            SearchSpec spec = new SearchSpec.Builder()
                    .addFilterNamespaces(domain)
                    .setTermMatch(SearchSpec.TERM_MATCH_EXACT_ONLY)
                    .build();
            session.remove("", spec, EXECUTOR, r -> {
                // 消すものが無い（NOT_FOUND）だけは成功。それ以外の失敗は成功に畳まない。
                boolean ok;
                try {
                    ok = r.isSuccess() || r.getResultCode() == AppSearchResult.RESULT_NOT_FOUND;
                } catch (RuntimeException e) {
                    ok = false;
                }
                finish(session, f, ok ? OK : FAILED);
            });
        });
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private static AppSearchSchema.StringPropertyConfig stringProp(String name) {
        return new AppSearchSchema.StringPropertyConfig.Builder(name)
                .setCardinality(AppSearchSchema.PropertyConfig.CARDINALITY_OPTIONAL)
                .setIndexingType(AppSearchSchema.StringPropertyConfig.INDEXING_TYPE_PREFIXES)
                .setTokenizerType(AppSearchSchema.StringPropertyConfig.TOKENIZER_TYPE_PLAIN)
                .build();
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private interface Op {
        /** `cancelled` が立っていたら、まだ発送していない OS 書き込みを行わず `finish(TIMEOUT)` する。 */
        void start(AppSearchSession session, CompletableFuture<Integer> f, AtomicBoolean cancelled);
    }

    /**
     * 前の要求の終端を待ってから session を開いて `op` を走らせ、`remainingMs` を上限に結果コードを待つ。
     * 期限は呼び出しの開始を起点に単調時計で測る。
     */
    @RequiresApi(Build.VERSION_CODES.S)
    private static int run(Context ctx, String domain, long remainingMs, Op op) {
        final long deadline = SystemClock.elapsedRealtime() + remainingMs;
        AppSearchManager mgr = ctx.getSystemService(AppSearchManager.class);
        if (mgr == null) return UNSUPPORTED;

        // 前の要求（期限切れで戻ったが OS 書き込みが未終端のもの）を追い越さない。
        CompletableFuture<Integer> prev;
        synchronized (LOCK) {
            prev = inFlight;
        }
        if (prev != null && !prev.isDone()) {
            try {
                prev.get(remainingMs, TimeUnit.MILLISECONDS);
            } catch (TimeoutException e) {
                return TIMEOUT;
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return FAILED;
            } catch (Exception ignored) {
                // 終端には達している。結果は前の要求のものなので見ない。
            }
        }
        long left = deadline - SystemClock.elapsedRealtime();
        if (left <= 0) return TIMEOUT;

        CompletableFuture<Integer> f = new CompletableFuture<>();
        AtomicBoolean cancelled = new AtomicBoolean(false);
        synchronized (LOCK) {
            inFlight = f;
        }
        try {
            mgr.createSearchSession(new AppSearchManager.SearchContext.Builder(domain).build(),
                    EXECUTOR, r -> {
                        if (!r.isSuccess()) {
                            f.complete(FAILED);
                            return;
                        }
                        AppSearchSession session;
                        try {
                            session = r.getResultValue();
                        } catch (RuntimeException e) {
                            f.complete(FAILED);
                            return;
                        }
                        if (cancelled.get()) {
                            finish(session, f, TIMEOUT);
                            return;
                        }
                        try {
                            op.start(session, f, cancelled);
                        } catch (RuntimeException e) {
                            finish(session, f, FAILED);
                        }
                    });
            return f.get(left, TimeUnit.MILLISECONDS);
        } catch (TimeoutException e) {
            // 未発送の書き込みを止める。発送済みの分は f の終端まで次の要求が待つ。
            cancelled.set(true);
            return TIMEOUT;
        } catch (InterruptedException e) {
            cancelled.set(true);
            Thread.currentThread().interrupt();
            return FAILED;
        } catch (Exception e) {
            // createSearchSession 自体の失敗。終端を確定して次の要求を待たせない。
            f.complete(FAILED);
            return FAILED;
        }
    }

    /** 全終端はここを通る。session を閉じてから結果を確定する。 */
    @RequiresApi(Build.VERSION_CODES.S)
    private static void finish(AppSearchSession session, CompletableFuture<Integer> f, int code) {
        try {
            session.close();
        } catch (RuntimeException ignored) {
            // 閉じるのに失敗しても結果は変えない。
        }
        f.complete(code);
    }
}
