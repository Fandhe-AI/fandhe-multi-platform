package com.fandhe.androidapp;

import android.content.Context;
import android.os.Build;
import android.os.Handler;
import android.os.HandlerThread;
import android.view.textservice.SentenceSuggestionsInfo;
import android.view.textservice.SpellCheckerSession;
import android.view.textservice.SuggestionsInfo;
import android.view.textservice.TextInfo;
import android.view.textservice.TextServicesManager;

import androidx.activity.ComponentActivity;
import androidx.annotation.RequiresApi;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

// スペルチェック（Issue #580、親 #119）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_spell_check.rs` が `MainActivity.requestSpellCheck` / `cancelSpellCheck` を
// JNI で呼び、結果を `nativeOnSpellCheckResult` で受ける）。
//
// 1 回の要求 = `TextServicesManager.newSpellCheckerSession` で session を作り、
// `getSentenceSuggestions`（`getSuggestions` は非推奨）で短い文を 1 回検査して、全経路で session を
// `cancel()`・`close()` する。session は Looper のあるスレッドで作る必要があるため、UI スレッドを
// 止めないよう専用の HandlerThread（最初の要求で起動）に開始を直列に流す。コールバックの到着スレッドには
// 依存せず、状態は `lock` の内側だけで扱う。`cancel` / `onDestroy` は呼び出しスレッドで即時に処理し、
// 戻る前に session の解放を済ませる。要求 id の照合は `RequestTracker`（first-wins）で行う。
//
// ライフサイクル: `onPause` では破棄しない（権限が要らず検査は短く、Rust 側の期限 10 秒後の `cancel` で
// 必ず止まるため）。`onDestroy`（Activity 再生成を含む）では保留中の要求を INTERRUPTED で確定し、
// session を閉じて HandlerThread を止める。`request` はインスタンスフィールドなので、再生成後の
// 新インスタンスは未追跡から始まり旧結果を拾わない。
//
// API の分岐: 31 未満は `isSpellCheckerEnabled` が使えないため、session が null なら「無効」と「辞書なし」を
// 区別せず UNSUPPORTED にする。31 以上は無効なら UNSUPPORTED、有効なのに session が null なら
// LANGUAGE_UNAVAILABLE。API 31 のメソッドは `@RequiresApi` を付けた private メソッドの中だけで呼ぶ。
// 入力文・誤り語・候補・OS の例外メッセージはログに出さない。
final class SpellCheckHelper {
    // 結果コード。SSOT はここで、Rust 側 `android_spell_check.rs::decode` が同じ値で写像する。
    static final int RESULT_OK = 0;
    static final int RESULT_UNSUPPORTED = 1;
    static final int RESULT_LANGUAGE_UNAVAILABLE = 2;
    static final int RESULT_INTERRUPTED = 3;
    static final int RESULT_FAILED = 4;

    // ranges: 誤り 1 件につき (UTF-16 開始位置, UTF-16 長さ, 候補数) の 3 値を平らに並べたもの。
    // suggestions: 全候補を誤りの順に平らに並べたもの。
    private static native void nativeOnSpellCheckResult(
            long requestId, int code, int[] ranges, String[] suggestions, String language);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    private final Object lock = new Object();
    // 取り消し済みの最大 id（lock の内側）。cancel が start より先に走っても古い要求を開始しない。
    private long cancelledId = RequestTracker.NONE;
    // 開始済みの最大 id（lock の内側）。JNI スレッドからの投入が前後して古い要求が後から start に
    // 届いても、新しい要求の session と追跡を上書きしない（id は Rust の SLOT が単調増加で振る）。
    private long startedId = RequestTracker.NONE;
    // 保留中の要求が持つ session（lock の内側）。
    private SpellCheckerSession session;
    private HandlerThread thread;
    private Handler handler;
    private volatile boolean destroyed;

    SpellCheckHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // JNI スレッドから呼ばれる。
    void request(long id, String text, String language, int limit) {
        Handler h;
        synchronized (lock) {
            if (destroyed) {
                h = null;
            } else {
                if (handler == null) {
                    thread = new HandlerThread("fandhe-spell-check");
                    thread.start();
                    handler = new Handler(thread.getLooper());
                }
                h = handler;
            }
        }
        if (h == null || !h.post(() -> start(id, text, language, limit))) {
            nativeOnSpellCheckResult(id, RESULT_INTERRUPTED, null, null, null);
        }
    }

    // Rust 側の期限切れから呼ばれる。古い id なら何もしない。呼び出しスレッドで session を閉じてから戻る。
    void cancel(long id) {
        SpellCheckerSession s = null;
        synchronized (lock) {
            cancelledId = Math.max(cancelledId, id);
            if (request.complete(id)) {
                s = session;
                session = null;
            }
        }
        close(s);
    }

    // MainActivity.onDestroy から UI スレッドで呼ぶ。
    void onDestroy() {
        destroyed = true;
        SpellCheckerSession s;
        long id;
        HandlerThread t;
        synchronized (lock) {
            id = request.take();
            s = session;
            session = null;
            t = thread;
            thread = null;
            handler = null;
        }
        close(s);
        if (id != RequestTracker.NONE) {
            nativeOnSpellCheckResult(id, RESULT_INTERRUPTED, null, null, null);
        }
        if (t != null) {
            t.quitSafely();
        }
    }

    private static void close(SpellCheckerSession s) {
        if (s == null) {
            return;
        }
        try {
            s.cancel();
        } catch (RuntimeException ignored) {
            // 取り消しの失敗は握りつぶし、close は必ず行う。
        }
        try {
            s.close();
        } catch (RuntimeException ignored) {
            // 閉じ済み等。
        }
    }

    // HandlerThread 上で実行する。
    private void start(long id, String text, String language, int limit) {
        SpellCheckerSession stale;
        synchronized (lock) {
            if (id <= cancelledId || id <= startedId) {
                return;
            }
            if (destroyed) {
                stale = null;
            } else {
                // 差し替えられた直前の要求の session が残っていれば、ここで解放する。
                stale = session;
                session = null;
                startedId = id;
                request.track(id);
            }
        }
        close(stale);
        if (destroyed) {
            nativeOnSpellCheckResult(id, RESULT_INTERRUPTED, null, null, null);
            return;
        }
        try {
            TextServicesManager tsm = (TextServicesManager)
                    activity.getSystemService(Context.TEXT_SERVICES_MANAGER_SERVICE);
            if (tsm == null) {
                finish(id, RESULT_UNSUPPORTED, null, null, null);
                return;
            }
            boolean api31 = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S;
            if (api31 && !isUsableOnS(tsm)) {
                finish(id, RESULT_UNSUPPORTED, null, null, null);
                return;
            }
            // 利用者の言語設定ではなく、要求した言語の subtype を使わせる。
            Listener listener = new Listener(id, language);
            SpellCheckerSession s = tsm.newSpellCheckerSession(
                    null, Locale.forLanguageTag(language), listener, false);
            if (s == null) {
                finish(id, api31 ? RESULT_LANGUAGE_UNAVAILABLE : RESULT_UNSUPPORTED, null, null, null);
                return;
            }
            boolean current;
            synchronized (lock) {
                current = request.current() == id;
                if (current) {
                    session = s;
                }
            }
            if (!current) {
                // 取り消し・差し替え・破棄の後に作られた session は即座に解放する。
                close(s);
                return;
            }
            listener.actualLanguage = resolveActualLanguage(s, language);
            s.getSentenceSuggestions(new TextInfo[] {new TextInfo(text)}, limit);
        } catch (RuntimeException e) {
            finish(id, RESULT_FAILED, null, null, null);
        }
    }

    // スペルチェッカーが有効で、現在のチェッカーが選ばれているか（API 31 以上）。
    @RequiresApi(Build.VERSION_CODES.S)
    private static boolean isUsableOnS(TextServicesManager tsm) {
        return tsm.isSpellCheckerEnabled() && tsm.getCurrentSpellCheckerInfo() != null;
    }

    private final class Listener implements SpellCheckerSession.SpellCheckerSessionListener {
        private final long id;
        // 実際に検査へ使う言語（session 生成後、検査開始前に設定する）。
        volatile String actualLanguage;

        Listener(long id, String requested) {
            this.id = id;
            this.actualLanguage = requested;
        }

        @Override
        public void onGetSuggestions(SuggestionsInfo[] results) {
            // 単語単位の API は使わない。
        }

        @Override
        public void onGetSentenceSuggestions(SentenceSuggestionsInfo[] results) {
            try {
                // 渡した TextInfo は 1 件なので、空配列は「検査されなかった」を表す（IME がスペルチェックを
                // 抑止中のとき SDK は空配列を返す）。誤り 0 件の成功にしない。
                if (results == null || results.length == 0) {
                    finish(id, RESULT_FAILED, null, null, null);
                    return;
                }
                List<Integer> ranges = new ArrayList<>();
                List<String> words = new ArrayList<>();
                // 欠けた要素（null）は検査結果が届いていないので、捨てずに失敗で確定する。
                // 誤り 0 件の成功にするのは、全要素を読めたときだけ。
                for (SentenceSuggestionsInfo info : results) {
                    if (info == null || !collect(info, ranges, words)) {
                        finish(id, RESULT_FAILED, null, null, null);
                        return;
                    }
                }
                int[] r = new int[ranges.size()];
                for (int i = 0; i < r.length; i++) {
                    r[i] = ranges.get(i);
                }
                finish(id, RESULT_OK, r, words.toArray(new String[0]), actualLanguage);
            } catch (RuntimeException e) {
                finish(id, RESULT_FAILED, null, null, null);
            }
        }
    }

    // session が実際に使う subtype の言語タグを推定する。OS は同じ主言語の別 subtype を選びうるため、
    // 要求をそのまま返さない。完全一致 → 同じ主言語の先頭の順（OS の選択規則と同じ）。subtype が
    // 列挙されない（言語非依存のチェッカー）・取得できない場合は要求した言語を返す。
    private static String resolveActualLanguage(SpellCheckerSession s, String requested) {
        try {
            android.view.textservice.SpellCheckerInfo info = s.getSpellChecker();
            if (info == null) {
                return requested;
            }
            String want = normalizeTag(requested);
            String primary = want.split("-")[0];
            String sameLanguage = null;
            for (int i = 0; i < info.getSubtypeCount(); i++) {
                android.view.textservice.SpellCheckerSubtype st = info.getSubtypeAt(i);
                String tag = st.getLanguageTag();
                if (tag == null || tag.isEmpty()) {
                    tag = st.getLocale() == null ? "" : st.getLocale().replace('_', '-');
                }
                if (tag.isEmpty()) {
                    continue;
                }
                String n = normalizeTag(tag);
                if (n.equals(want)) {
                    return tag;
                }
                if (sameLanguage == null && n.split("-")[0].equals(primary)) {
                    sameLanguage = tag;
                }
            }
            return sameLanguage != null ? sameLanguage : requested;
        } catch (RuntimeException e) {
            return requested;
        }
    }

    private static String normalizeTag(String tag) {
        return tag.replace('_', '-').toLowerCase(Locale.ROOT);
    }

    // 綴りの誤りだけを取り出す（辞書にある語・文法の誤りだけのものは除く）。語ごとの結果が欠けて
    // いたら false を返し、呼び出し側が失敗で確定する（欠けた語を「誤りなし」に畳まない）。
    private static boolean collect(SentenceSuggestionsInfo info, List<Integer> ranges, List<String> words) {
        // 候補数の上限。Rust 側が MAX_SUGGESTIONS で整形するため、ここでは過大な配列を避ける目安のみ。
        final int cap = 16;
        for (int i = 0; i < info.getSuggestionsCount(); i++) {
            SuggestionsInfo si = info.getSuggestionsInfoAt(i);
            if (si == null) {
                return false;
            }
            int attrs = si.getSuggestionsAttributes();
            boolean typo = (attrs & SuggestionsInfo.RESULT_ATTR_LOOKS_LIKE_TYPO) != 0;
            boolean known = (attrs & SuggestionsInfo.RESULT_ATTR_IN_THE_DICTIONARY) != 0;
            if (!typo || known) {
                continue;
            }
            int count = Math.min(Math.max(si.getSuggestionsCount(), 0), cap);
            int added = 0;
            for (int j = 0; j < count; j++) {
                String w = si.getSuggestionAt(j);
                if (w != null) {
                    words.add(w);
                    added++;
                }
            }
            ranges.add(info.getOffsetAt(i));
            ranges.add(info.getLengthAt(i));
            ranges.add(added);
        }
        return true;
    }

    // この id が保留中の要求なら 1 回だけ確定し、session を解放する。取り出しと解放対象の取得は
    // cancel と同じ lock 内で不可分に行い、native 通知は lock の外で行う（first-wins）。
    private void finish(long id, int code, int[] ranges, String[] suggestions, String language) {
        SpellCheckerSession s;
        synchronized (lock) {
            if (!request.complete(id)) {
                return; // 取り消し・差し替え・破棄済み。
            }
            s = session;
            session = null;
        }
        close(s);
        nativeOnSpellCheckResult(id, code, ranges, suggestions, language);
    }
}
