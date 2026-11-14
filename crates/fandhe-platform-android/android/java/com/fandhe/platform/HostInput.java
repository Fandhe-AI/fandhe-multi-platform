package com.fandhe.platform;

import android.app.Activity;
import android.os.Build;
import android.view.ActionMode;
import android.view.KeyEvent;
import android.view.KeyboardShortcutGroup;
import android.view.Menu;
import android.view.MenuItem;
import android.view.MotionEvent;
import android.view.SearchEvent;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;
import android.view.accessibility.AccessibilityEvent;
import androidx.core.util.Consumer;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

// Activity の入力（タッチ・キー・汎用モーション）を、Activity より先に観測する共通層の口（Issue #1327）。
//
// Window の `Window.Callback` を 1 段だけ包み、観測者へ通知してから元の callback の戻り値をそのまま返す。
// 【契約】イベントは消費しない（消費の口は作らない）: GameActivity → winit → egui の既存の入力経路、
// TV フォーカスのリモコン十字キーを変えない。観測した入力はログへ出さず保存しない
// （キー入力には物理キーボードの打鍵が含まれる）。
//
// 呼び出し元: 機能 crate の `FeatureLifecycleProvider.onHostCreated`（gamepad のキー・モーション、
// 移設後の pen-stylus のタッチ）。旧構造の `MainActivity` は `onCreate` で pen-stylus を登録する。
// `getDecorView()` は呼ばないので `onHostCreated` から呼べる。観測リストは包みのインスタンスが持ち、
// Window と一緒に消える（static も解除 API も持たない）。UI スレッドから呼ぶ。
// 守らない範囲: 起動後に `getWindow().setCallback()` で差し替えられた場合に届かなくなること。
public final class HostInput {
    private HostInput() {}

    /** タッチ入力の観測者を足す。同じインスタンスの再登録は無視する。 */
    public static void addTouchObserver(Activity activity, Consumer<MotionEvent> observer) {
        ObservingCallback cb = install(activity);
        if (cb != null) {
            cb.touch.addIfAbsent(observer);
        }
    }

    /** キー入力の観測者を足す。同じインスタンスの再登録は無視する。 */
    public static void addKeyObserver(Activity activity, Consumer<KeyEvent> observer) {
        ObservingCallback cb = install(activity);
        if (cb != null) {
            cb.key.addIfAbsent(observer);
        }
    }

    /** 汎用モーション（ジョイスティック等）の観測者を足す。同じインスタンスの再登録は無視する。 */
    public static void addGenericMotionObserver(Activity activity, Consumer<MotionEvent> observer) {
        ObservingCallback cb = install(activity);
        if (cb != null) {
            cb.genericMotion.addIfAbsent(observer);
        }
    }

    // 既に包んであればそれを返し、違えば 1 段包む。callback が null なら何もしない。
    private static ObservingCallback install(Activity activity) {
        Window window = activity.getWindow();
        Window.Callback current = window.getCallback();
        if (current == null) {
            return null;
        }
        if (current instanceof ObservingCallback) {
            return (ObservingCallback) current;
        }
        ObservingCallback wrapper = new ObservingCallback(current);
        window.setCallback(wrapper);
        return wrapper;
    }

    // 元の Window.Callback へ全メソッドを委譲し、dispatch 系 3 つだけ通知してから元の戻り値をそのまま返す。
    // API 24 以降の default メソッドも明示的に委譲し、元の実装を飛ばさない。
    private static final class ObservingCallback implements Window.Callback {
        private final Window.Callback wrapped;
        final CopyOnWriteArrayList<Consumer<MotionEvent>> touch = new CopyOnWriteArrayList<>();
        final CopyOnWriteArrayList<Consumer<KeyEvent>> key = new CopyOnWriteArrayList<>();
        final CopyOnWriteArrayList<Consumer<MotionEvent>> genericMotion = new CopyOnWriteArrayList<>();

        ObservingCallback(Window.Callback wrapped) {
            this.wrapped = wrapped;
        }

        @Override
        public boolean dispatchKeyEvent(KeyEvent event) {
            for (Consumer<KeyEvent> o : key) {
                o.accept(event);
            }
            return wrapped.dispatchKeyEvent(event);
        }

        @Override
        public boolean dispatchGenericMotionEvent(MotionEvent event) {
            for (Consumer<MotionEvent> o : genericMotion) {
                o.accept(event);
            }
            return wrapped.dispatchGenericMotionEvent(event);
        }

        @Override
        public boolean dispatchTouchEvent(MotionEvent event) {
            for (Consumer<MotionEvent> o : touch) {
                o.accept(event);
            }
            return wrapped.dispatchTouchEvent(event);
        }

        @Override
        public boolean dispatchKeyShortcutEvent(KeyEvent event) {
            return wrapped.dispatchKeyShortcutEvent(event);
        }

        @Override
        public boolean dispatchTrackballEvent(MotionEvent event) {
            return wrapped.dispatchTrackballEvent(event);
        }

        @Override
        public boolean dispatchPopulateAccessibilityEvent(AccessibilityEvent event) {
            return wrapped.dispatchPopulateAccessibilityEvent(event);
        }

        @Override
        public View onCreatePanelView(int featureId) {
            return wrapped.onCreatePanelView(featureId);
        }

        @Override
        public boolean onCreatePanelMenu(int featureId, Menu menu) {
            return wrapped.onCreatePanelMenu(featureId, menu);
        }

        @Override
        public boolean onPreparePanel(int featureId, View view, Menu menu) {
            return wrapped.onPreparePanel(featureId, view, menu);
        }

        @Override
        public boolean onMenuOpened(int featureId, Menu menu) {
            return wrapped.onMenuOpened(featureId, menu);
        }

        @Override
        public boolean onMenuItemSelected(int featureId, MenuItem item) {
            return wrapped.onMenuItemSelected(featureId, item);
        }

        @Override
        public void onWindowAttributesChanged(WindowManager.LayoutParams attrs) {
            wrapped.onWindowAttributesChanged(attrs);
        }

        @Override
        public void onContentChanged() {
            wrapped.onContentChanged();
        }

        @Override
        public void onWindowFocusChanged(boolean hasFocus) {
            wrapped.onWindowFocusChanged(hasFocus);
        }

        @Override
        public void onAttachedToWindow() {
            wrapped.onAttachedToWindow();
        }

        @Override
        public void onDetachedFromWindow() {
            wrapped.onDetachedFromWindow();
        }

        @Override
        public void onPanelClosed(int featureId, Menu menu) {
            wrapped.onPanelClosed(featureId, menu);
        }

        @Override
        public boolean onSearchRequested() {
            return wrapped.onSearchRequested();
        }

        @Override
        public boolean onSearchRequested(SearchEvent searchEvent) {
            return wrapped.onSearchRequested(searchEvent);
        }

        @Override
        public ActionMode onWindowStartingActionMode(ActionMode.Callback callback) {
            return wrapped.onWindowStartingActionMode(callback);
        }

        @Override
        public ActionMode onWindowStartingActionMode(ActionMode.Callback callback, int type) {
            return wrapped.onWindowStartingActionMode(callback, type);
        }

        @Override
        public void onActionModeStarted(ActionMode mode) {
            wrapped.onActionModeStarted(mode);
        }

        @Override
        public void onActionModeFinished(ActionMode mode) {
            wrapped.onActionModeFinished(mode);
        }

        @Override
        public void onProvideKeyboardShortcuts(
                List<KeyboardShortcutGroup> data, Menu menu, int deviceId) {
            wrapped.onProvideKeyboardShortcuts(data, menu, deviceId);
        }

        @Override
        public void onPointerCaptureChanged(boolean hasCapture) {
            // API 26 で追加された default メソッド。minSdk 24 の lint（NewApi）に合わせて分岐する。
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                wrapped.onPointerCaptureChanged(hasCapture);
            }
        }
    }
}
