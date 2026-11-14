package com.fandhe.platform;

import android.app.Activity;
import android.hardware.input.InputManager;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import androidx.core.util.Consumer;

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

    // 観測者は static final の同一インスタンスにする（`HostInput` は同じインスタンスの再登録を無視するため、
    // Activity 再生成後の新しい Window へ登録し直しても二重に呼ばれない）。
    private static final Consumer<KeyEvent> KEY_OBSERVER = GamepadHelper::observeKey;
    private static final Consumer<MotionEvent> MOTION_OBSERVER = GamepadHelper::observeMotion;

    // 共通層 `HostInput`（#1327）へキー・汎用モーションの観測者を登録する。イベントは消費されない。
    static void wrap(Activity activity) {
        HostInput.addKeyObserver(activity, KEY_OBSERVER);
        HostInput.addGenericMotionObserver(activity, MOTION_OBSERVER);
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
}
