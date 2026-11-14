package com.fandhe.platform;

import android.app.Activity;
import android.os.Looper;
import android.view.View;
import androidx.activity.ComponentActivity;
import androidx.core.graphics.Insets;
import androidx.core.util.Consumer;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.lifecycle.Lifecycle;
import androidx.lifecycle.LifecycleEventObserver;
import java.util.HashMap;
import java.util.concurrent.CopyOnWriteArrayList;

// window insets の変化を複数の機能 crate へ配る共通層の口（Issue #1327）。
//
// `ViewCompat.setOnApplyWindowInsetsListener` は View につき 1 つしか付けられないため、decorView への
// リスナーは本クラスが 1 つだけ付け、(1) Rust の static へ px を送る（`fandhe_platform_android::window_insets_px`）、
// (2) 登録された機能（fullscreen-immersive 等）へ `WindowInsetsCompat` を配る。insets は消費せず
// そのまま返し、GameActivity 自身の insets 処理を妨げない。
//
// 呼び出し元: 機能 crate の `FeatureLifecycleProvider`（`onHostStarted` / `onHostResumed`。decorView を
// 触るため `onHostCreated` では呼ばない）。旧構造の `MainActivity` は `onCreate` の末尾で呼ぶ。
// 値だけが要る機能（safe-area-insets）も、取り付けは最初の登録で起きるため provider から登録する。
// 守らない範囲: 破棄中の Window から届く報告の抑止（受け手が自分の破棄フラグで捨てる）、
// Activity 再生成直後に前の値が 1 回残ること。
public final class HostInsets {
    private HostInsets() {}

    // 実体は fandhe-platform-android の Java_com_fandhe_platform_HostInsets_nativeOnWindowInsetsChanged。
    // 署名（int, int, int, int）→ void は Rust 側と 2 か所で一致させる。
    private static native void nativeOnWindowInsetsChanged(int left, int top, int right, int bottom);

    // Activity ごとの配り先。UI スレッドだけが触る。ON_DESTROY で外す（WeakHashMap は値 → Activity の
    // 強参照で回収されないため使わない）。
    private static final HashMap<Activity, CopyOnWriteArrayList<Consumer<WindowInsetsCompat>>> LISTENERS =
            new HashMap<>();

    /**
     * insets の配り先を足す。UI スレッドから呼ぶ。同じインスタンスの再登録は無視する。
     * 破棄済み（DESTROYED）の Activity には何もしない。
     */
    public static void addListener(ComponentActivity activity, Consumer<WindowInsetsCompat> listener) {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            throw new IllegalStateException("HostInsets.addListener は UI スレッドから呼ぶ");
        }
        if (activity.getLifecycle().getCurrentState() == Lifecycle.State.DESTROYED) {
            return;
        }
        CopyOnWriteArrayList<Consumer<WindowInsetsCompat>> list = LISTENERS.get(activity);
        if (list == null) {
            list = new CopyOnWriteArrayList<>();
            LISTENERS.put(activity, list);
            attach(activity, list);
        }
        if (list.addIfAbsent(listener)) {
            // 後から登録した機能にも現在値を届ける（decor 未 attach なら no-op）。
            ViewCompat.requestApplyInsets(activity.getWindow().getDecorView());
        }
    }

    private static void attach(
            ComponentActivity activity, CopyOnWriteArrayList<Consumer<WindowInsetsCompat>> list) {
        View decor = activity.getWindow().getDecorView();
        ViewCompat.setOnApplyWindowInsetsListener(decor, (view, insets) -> {
            Insets bars = insets.getInsets(
                    WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout());
            nativeOnWindowInsetsChanged(bars.left, bars.top, bars.right, bars.bottom);
            for (Consumer<WindowInsetsCompat> l : list) {
                l.accept(insets);
            }
            return insets;
        });
        activity.getLifecycle().addObserver((LifecycleEventObserver) (source, event) -> {
            if (event == Lifecycle.Event.ON_DESTROY) {
                LISTENERS.remove(activity);
            }
        });
    }
}
