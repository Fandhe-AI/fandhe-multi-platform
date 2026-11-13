package com.fandhe.platform;

import android.app.Activity;
import android.hardware.input.InputManager;
import android.os.Build;
import android.view.ActionMode;
import android.view.InputDevice;
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

import java.util.List;

// ゲームコントローラ入力（Issue #935 / #1303）の Java 側。Rust 側
// （`crates/fandhe-cap-gamepad/src/android.rs`）の JNI エクスポート 5 本へ生値を渡すだけで、
// ソース判定の最終判断・デッドゾーン・値域正規化・keyCode の写像・状態遷移は Rust 側の状態機械が担う
// （`platforms/android/app/src/` に Java のテスト基盤が無く、ロジックはホストの `cargo test` で検証するため）。
//
// 呼び出し元は `GamepadLifecycle` だけ。Rust から Java は呼ばない（static native による一方向）。
final class GamepadHelper {
    private GamepadHelper() {}

    // `reportPresence()` が `InputDevice.getDeviceIds()` を再走査した結果を
    // begin → (見つかった deviceId ごとに) scanId → end の 3 段で通知する。1 台の物理コントローラが
    // ボタン用・ジョイスティック用の 2 つの deviceId に分かれて見える機種（split pad、Joy-Con 等）は
    // Android 上で別々の InputDevice として扱われ、公開 API に再結合の手段が無いため束ねない
    // （Rust 側の状態機械は走査で見つかった先頭 1 台のみを追跡する、PR #939）。
    private static native void nativeOnGamepadPresenceScanBegin();
    private static native void nativeOnGamepadPresenceScanId(int deviceId);
    private static native void nativeOnGamepadPresenceScanEnd();
    // `dispatchGenericMotionEvent` の最新サンプル（x/y/z/rz はスティック軸、leftTrigger/rightTrigger は
    // トリガ軸、hatX/hatY は十字キー軸、flat* は各軸の `MotionRange#getFlat()`）。
    private static native void nativeOnGamepadMotion(
            int deviceId,
            int source,
            float x,
            float y,
            float z,
            float rz,
            float leftTrigger,
            float rightTrigger,
            float hatX,
            float hatY,
            float flatX,
            float flatY,
            float flatZ,
            float flatRz);
    // `dispatchKeyEvent` の DOWN/UP（keyCode は `KeyEvent.KEYCODE_BUTTON_*` / `KEYCODE_DPAD_*`）。
    private static native void nativeOnGamepadKey(int deviceId, int source, int keyCode, boolean down);

    // 接続監視。Android には軸の現在値を問い合わせる API が無い（イベント駆動のみ）ため、
    // `InputDevice.getDeviceIds()` を都度再走査して「先頭のゲームパッドの deviceId、または無し」を通知する
    // （レベルトリガ）。listener は static 1 つで、同じ listener の重複登録を OS は無視する。
    private static InputManager inputManager;

    private static final InputManager.InputDeviceListener DEVICE_LISTENER =
            new InputManager.InputDeviceListener() {
                @Override
                public void onInputDeviceAdded(int deviceId) {
                    reportPresence();
                }

                @Override
                public void onInputDeviceRemoved(int deviceId) {
                    reportPresence();
                }

                @Override
                public void onInputDeviceChanged(int deviceId) {
                    reportPresence();
                }
            };

    // Activity の Window.Callback を包む。`getDecorView()` は呼ばない（`requestWindowFeature` への副作用を避ける）。
    // 二重に包まない（同じ Window へ複数回呼ばれても 1 段だけ）。
    static void wrap(Activity activity) {
        Window window = activity.getWindow();
        Window.Callback current = window.getCallback();
        if (current == null || current instanceof ObservingCallback) {
            return;
        }
        window.setCallback(new ObservingCallback(current));
    }

    static void onResumed(Activity activity) {
        inputManager = activity.getSystemService(InputManager.class);
        if (inputManager != null) {
            inputManager.registerInputDeviceListener(DEVICE_LISTENER, null);
            reportPresence();
        }
    }

    // バックグラウンド中の key up・切断の取りこぼしで「接続中」が張り付かないよう、登録解除に加えて
    // 明示的に「無し」（begin → end を続けて送る）を通知する。
    static void onPaused() {
        if (inputManager != null) {
            inputManager.unregisterInputDeviceListener(DEVICE_LISTENER);
            nativeOnGamepadPresenceScanBegin();
            nativeOnGamepadPresenceScanEnd();
        }
    }

    // 接続中のゲームパッド／ジョイスティック（`isVirtual()` を除く）を漏れなく Rust 側へ通知する。
    private static void reportPresence() {
        InputManager manager = inputManager;
        if (manager == null) {
            return;
        }
        nativeOnGamepadPresenceScanBegin();
        for (int deviceId : manager.getInputDeviceIds()) {
            InputDevice device = manager.getInputDevice(deviceId);
            if (device == null || device.isVirtual()) {
                continue;
            }
            int sources = device.getSources();
            boolean isGamepad = (sources & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD;
            boolean isJoystick = (sources & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK;
            if (isGamepad || isJoystick) {
                nativeOnGamepadPresenceScanId(deviceId);
            }
        }
        nativeOnGamepadPresenceScanEnd();
    }

    // ゲームコントローラのスティック・トリガ・十字キー軸の中継。最新サンプルのみを渡す（履歴サンプルは読まない）。
    private static void observeMotion(MotionEvent event) {
        int source = event.getSource();
        boolean isJoystick = (source & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK;
        if (isJoystick && event.getAction() == MotionEvent.ACTION_MOVE) {
            InputDevice device = event.getDevice();
            nativeOnGamepadMotion(
                    event.getDeviceId(),
                    source,
                    event.getAxisValue(MotionEvent.AXIS_X),
                    event.getAxisValue(MotionEvent.AXIS_Y),
                    event.getAxisValue(MotionEvent.AXIS_Z),
                    event.getAxisValue(MotionEvent.AXIS_RZ),
                    Math.max(
                            event.getAxisValue(MotionEvent.AXIS_LTRIGGER),
                            event.getAxisValue(MotionEvent.AXIS_BRAKE)),
                    Math.max(
                            event.getAxisValue(MotionEvent.AXIS_RTRIGGER),
                            event.getAxisValue(MotionEvent.AXIS_GAS)),
                    event.getAxisValue(MotionEvent.AXIS_HAT_X),
                    event.getAxisValue(MotionEvent.AXIS_HAT_Y),
                    axisFlat(device, source, MotionEvent.AXIS_X),
                    axisFlat(device, source, MotionEvent.AXIS_Y),
                    axisFlat(device, source, MotionEvent.AXIS_Z),
                    axisFlat(device, source, MotionEvent.AXIS_RZ));
        }
    }

    // `InputDevice.getMotionRange(axis, source)` のデッドゾーン幅（`range` が無ければ 0。
    // Rust 側 `apply_flat` が NaN・負値と同様に安全側へ倒す）。
    private static float axisFlat(InputDevice device, int source, int axis) {
        if (device == null) {
            return 0f;
        }
        InputDevice.MotionRange range = device.getMotionRange(axis, source);
        return range == null ? 0f : range.getFlat();
    }

    // ゲームコントローラのボタン押下/解放の中継。ソース前段フィルタ（GAMEPAD / JOYSTICK のみ）は、
    // 物理キーボードの打鍵（パスワード等）をゲームパッド経路へ誤って流さないための防御。
    // HAT 軸を持たず十字キーを SOURCE_DPAD（KEYBOARD ビット無し）の KeyEvent でのみ通知する機種があるため、
    // SOURCE_DPAD かつ SOURCE_KEYBOARD を含まない場合も通す（TV リモコンは SOURCE_KEYBOARD | SOURCE_DPAD で来る
    // ため誤って混ざらない、PR #939）。
    private static void observeKey(KeyEvent event) {
        int source = event.getSource();
        boolean isGamepad = (source & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD;
        boolean isJoystick = (source & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK;
        boolean isDpadOnly =
                (source & InputDevice.SOURCE_DPAD) == InputDevice.SOURCE_DPAD
                        && (source & InputDevice.SOURCE_KEYBOARD) != InputDevice.SOURCE_KEYBOARD;
        if (isGamepad || isJoystick || isDpadOnly) {
            int action = event.getAction();
            if (action == KeyEvent.ACTION_DOWN || action == KeyEvent.ACTION_UP) {
                nativeOnGamepadKey(
                        event.getDeviceId(), source, event.getKeyCode(), action == KeyEvent.ACTION_DOWN);
            }
        }
    }

    // 元の Window.Callback（通常は Activity、または AppCompat の包み）へ全メソッドを委譲し、
    // `dispatchKeyEvent` / `dispatchGenericMotionEvent` だけ通知してから元の戻り値をそのまま返す。
    // 【契約】イベントは一切消費しない: TV フォーカス（#280）のリモコン十字キー（`KEYCODE_DPAD_*`）を含む
    // 既存の入力経路（GameActivity → winit → `egui::Key::Arrow*`）を変えない。
    // API 24 以降の default メソッドも明示的に委譲し、元の実装を飛ばさない。
    private static final class ObservingCallback implements Window.Callback {
        private final Window.Callback wrapped;

        ObservingCallback(Window.Callback wrapped) {
            this.wrapped = wrapped;
        }

        @Override
        public boolean dispatchKeyEvent(KeyEvent event) {
            observeKey(event);
            return wrapped.dispatchKeyEvent(event);
        }

        @Override
        public boolean dispatchGenericMotionEvent(MotionEvent event) {
            observeMotion(event);
            return wrapped.dispatchGenericMotionEvent(event);
        }

        @Override
        public boolean dispatchKeyShortcutEvent(KeyEvent event) {
            return wrapped.dispatchKeyShortcutEvent(event);
        }

        @Override
        public boolean dispatchTouchEvent(MotionEvent event) {
            return wrapped.dispatchTouchEvent(event);
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
