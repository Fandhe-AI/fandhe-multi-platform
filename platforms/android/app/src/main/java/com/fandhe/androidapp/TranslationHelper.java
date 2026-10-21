package com.fandhe.androidapp;

import android.icu.util.ULocale;
import android.os.Build;
import android.os.CancellationSignal;
import android.util.SparseArray;
import android.view.translation.TranslationCapability;
import android.view.translation.TranslationContext;
import android.view.translation.TranslationManager;
import android.view.translation.TranslationRequest;
import android.view.translation.TranslationRequestValue;
import android.view.translation.TranslationResponse;
import android.view.translation.TranslationResponseValue;
import android.view.translation.TranslationSpec;
import android.view.translation.Translator;

import androidx.activity.ComponentActivity;
import androidx.annotation.RequiresApi;

import java.util.Collections;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.RejectedExecutionException;

// 端末内翻訳（Issue #537、親 #115）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_translation.rs` が `MainActivity.requestTranslation` /
// `cancelTranslation` を JNI で呼び、結果を `nativeOnTranslationResult` で受ける）。
//
// 1 回の要求 = `android.view.translation`（API 31 以上）で、翻訳サービスの言語状態を確かめ、
// 端末内に入っている組だけ Translator を作って 1 回訳す。言語パックのダウンロードは始めない。
// 開始と OS コールバックは単一スレッドの executor へ直列に流す（`getOnDeviceTranslationCapabilities`
// は UI スレッド不可のため）。`cancel` は executor が詰まっていても戻る前に解除が済むよう、
// 呼び出しスレッドで即時に処理する。要求 id の照合と Translator の保持・解放は `lock` の内側で行う。
//
// ライフサイクル: `onPause` では破棄しない（Rust 側の期限 30 秒後の `cancel` で必ず止まり、
// 背景での残存に上限があるため）。`onDestroy`（Activity 再生成を含む）では保留中の要求を
// INTERRUPTED で確定して Translator を破棄する。`request` はインスタンスフィールドなので、
// 再生成後の新インスタンスは未追跡から始まり旧結果を拾わない。
//
// API 31 のクラスは `@RequiresApi` を付けた private メソッドの中だけで使い、minSdk 24 の端末で
// 分岐の手前（`request`）から先へ進まないようにする。原文・訳文・OS の例外メッセージはログに出さない。
final class TranslationHelper {
    // 結果コード。SSOT はここで、Rust 側 `android_translation.rs::decode` が同じ値で写像する。
    static final int RESULT_OK = 0;
    static final int RESULT_UNSUPPORTED = 1;
    static final int RESULT_LANGUAGE_UNSUPPORTED = 2;
    static final int RESULT_NOT_INSTALLED = 3;
    static final int RESULT_INTERRUPTED = 4;
    static final int RESULT_FAILED = 5;

    private static native void nativeOnTranslationResult(long requestId, int code, String text);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    private final ExecutorService executor = Executors.newSingleThreadExecutor();
    private final Object lock = new Object();
    // 取り消し済みの最大 id（lock の内側）。cancel が start より先に走っても古い要求を開始しない。
    private long cancelledId = RequestTracker.NONE;
    // 保留中の要求が持つ Translator の破棄処理と取り消し信号（lock の内側）。
    private Runnable destroyer;
    private CancellationSignal signal;
    private volatile boolean destroyed;

    TranslationHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // JNI スレッドから呼ばれる。
    void request(long id, String text, String source, String target) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            nativeOnTranslationResult(id, RESULT_UNSUPPORTED, null);
            return;
        }
        try {
            executor.execute(() -> start(id, text, source, target));
        } catch (RejectedExecutionException e) {
            nativeOnTranslationResult(id, RESULT_INTERRUPTED, null);
        }
    }

    // Rust 側の期限切れから呼ばれる。古い id なら何もしない。戻る前に翻訳と Translator を解放する
    // （executor へ post しない: executor が停滞しても期限後に OS の処理が残らないようにする）。
    void cancel(long id) {
        Runnable d = null;
        CancellationSignal s = null;
        synchronized (lock) {
            cancelledId = Math.max(cancelledId, id);
            if (request.complete(id)) {
                d = destroyer;
                s = signal;
                destroyer = null;
                signal = null;
            }
        }
        release(d, s);
    }

    // MainActivity.onDestroy から UI スレッドで呼ぶ。
    void onDestroy() {
        destroyed = true;
        Runnable d;
        CancellationSignal s;
        long id;
        synchronized (lock) {
            id = request.take();
            d = destroyer;
            s = signal;
            destroyer = null;
            signal = null;
        }
        release(d, s);
        if (id != RequestTracker.NONE) {
            nativeOnTranslationResult(id, RESULT_INTERRUPTED, null);
        }
        executor.shutdownNow();
    }

    private static void release(Runnable destroyer, CancellationSignal signal) {
        try {
            if (signal != null) {
                signal.cancel();
            }
        } catch (RuntimeException ignored) {
            // 取り消しの失敗は握りつぶし、Translator の破棄は必ず行う。
        }
        try {
            if (destroyer != null) {
                destroyer.run();
            }
        } catch (RuntimeException ignored) {
            // 破棄済み等。
        }
    }

    // executor 上で実行する。
    private void start(long id, String text, String source, String target) {
        Runnable stale;
        CancellationSignal stalePending;
        synchronized (lock) {
            if (id <= cancelledId) {
                return;
            }
            if (destroyed) {
                stale = null;
                stalePending = null;
            } else {
                // 差し替えられた直前の要求の資源が残っていれば、ここで解放する。
                stale = destroyer;
                stalePending = signal;
                request.track(id);
                destroyer = null;
                signal = new CancellationSignal();
            }
        }
        release(stale, stalePending);
        if (destroyed) {
            nativeOnTranslationResult(id, RESULT_INTERRUPTED, null);
            return;
        }
        try {
            startOnS(id, text, source, target);
        } catch (RuntimeException e) {
            finish(id, RESULT_FAILED, null);
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private void startOnS(long id, String text, String source, String target) {
        TranslationManager tm = activity.getSystemService(TranslationManager.class);
        if (tm == null) {
            finish(id, RESULT_UNSUPPORTED, null);
            return;
        }
        int state = languageCode(tm, source, target);
        if (state != RESULT_OK) {
            finish(id, state, null);
            return;
        }
        TranslationContext ctx = new TranslationContext.Builder(
                new TranslationSpec(ULocale.forLanguageTag(source), TranslationSpec.DATA_FORMAT_TEXT),
                new TranslationSpec(ULocale.forLanguageTag(target), TranslationSpec.DATA_FORMAT_TEXT))
                .build();
        tm.createOnDeviceTranslator(ctx, executor, translator -> onTranslator(id, text, translator));
    }

    // 翻訳サービスの状態を結果コードに写す。RESULT_OK なら端末内に言語パックがある。
    @RequiresApi(Build.VERSION_CODES.S)
    private static int languageCode(TranslationManager tm, String source, String target) {
        Set<TranslationCapability> caps = tm.getOnDeviceTranslationCapabilities(
                TranslationSpec.DATA_FORMAT_TEXT, TranslationSpec.DATA_FORMAT_TEXT);
        if (caps == null || caps.isEmpty()) {
            return RESULT_UNSUPPORTED; // 翻訳サービスがない端末。
        }
        String src = ULocale.forLanguageTag(source).getLanguage();
        String dst = ULocale.forLanguageTag(target).getLanguage();
        int result = RESULT_LANGUAGE_UNSUPPORTED;
        for (TranslationCapability c : caps) {
            if (!src.equals(c.getSourceSpec().getLocale().getLanguage())
                    || !dst.equals(c.getTargetSpec().getLocale().getLanguage())) {
                continue;
            }
            switch (c.getState()) {
                case TranslationCapability.STATE_ON_DEVICE:
                    return RESULT_OK;
                case TranslationCapability.STATE_AVAILABLE_TO_DOWNLOAD:
                case TranslationCapability.STATE_DOWNLOADING:
                    result = RESULT_NOT_INSTALLED; // ダウンロードは始めない。
                    break;
                default:
                    break;
            }
        }
        return result;
    }

    // Translator 生成のコールバック（executor 上）。
    @RequiresApi(Build.VERSION_CODES.S)
    private void onTranslator(long id, String text, Translator translator) {
        if (translator == null) {
            finish(id, RESULT_FAILED, null);
            return;
        }
        CancellationSignal s;
        boolean current;
        synchronized (lock) {
            current = request.current() == id;
            s = signal;
            if (current) {
                destroyer = translator::destroy;
            }
        }
        if (!current) {
            // 取り消し・差し替え・破棄の後に届いた Translator は即座に解放する。
            translator.destroy();
            return;
        }
        try {
            TranslationRequest req = new TranslationRequest.Builder()
                    .setTranslationRequestValues(
                            Collections.singletonList(TranslationRequestValue.forText(text)))
                    .build();
            translator.translate(req, s, executor, response -> onResponse(id, response));
        } catch (RuntimeException e) {
            finish(id, RESULT_FAILED, null);
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private void onResponse(long id, TranslationResponse response) {
        if (!response.isFinalResponse()) {
            return;
        }
        try {
            int status = response.getTranslationStatus();
            if (status == TranslationResponse.TRANSLATION_STATUS_CONTEXT_UNSUPPORTED) {
                finish(id, RESULT_LANGUAGE_UNSUPPORTED, null);
                return;
            }
            if (status != TranslationResponse.TRANSLATION_STATUS_SUCCESS) {
                finish(id, RESULT_FAILED, null);
                return;
            }
            SparseArray<TranslationResponseValue> values = response.getTranslationResponseValues();
            TranslationResponseValue v = values == null ? null : values.get(0);
            CharSequence out = v == null ? null : v.getText();
            if (v == null || v.getStatusCode() != TranslationResponseValue.STATUS_SUCCESS || out == null) {
                finish(id, RESULT_FAILED, null);
                return;
            }
            finish(id, RESULT_OK, out.toString());
        } catch (RuntimeException e) {
            finish(id, RESULT_FAILED, null);
        }
    }

    // この id が保留中の要求なら 1 回だけ確定し、Translator を解放する。取り出しと解放対象の取得は
    // cancel と同じ lock 内で不可分に行い、native 通知は lock の外で行う（first-wins）。
    private void finish(long id, int code, String text) {
        Runnable d;
        CancellationSignal s;
        synchronized (lock) {
            if (!request.complete(id)) {
                return; // 取り消し・差し替え・破棄済み。
            }
            d = destroyer;
            s = signal;
            destroyer = null;
            signal = null;
        }
        release(d, s);
        nativeOnTranslationResult(id, code, text);
    }
}
