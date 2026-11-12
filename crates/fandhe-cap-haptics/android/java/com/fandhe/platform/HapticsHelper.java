package com.fandhe.platform;

import android.content.Context;
import android.os.Build;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.os.VibratorManager;

// 触覚フィードバック画面（Issue #1092、#1187）の Android 実装ヘルパー。利用者の `MainActivity` には何も書かせない。
// Rust の `src/android.rs` が egui スレッドから static の `tap(Context)` を JNI で同期に呼ぶ。`Vibrator.vibrate` に
// スレッド制約は無く、Java -> Rust の native コールバックも持たないため no-op スタブは不要。状態を持たない
// ので Activity のライフサイクルへ参加する必要もない。
//
// 戻り値（Rust 側 `decode_haptics_result` と対で決める契約）:
//   0 = 要求を渡した（サイレント・システム触覚オフでは無反応で、検知できない）
//   1 = 振動子なし（`Unsupported` へ写す）
//   2 = 失敗（`SecurityException` 等。Java 例外を JNI 側へ残さず `Internal` へ写す）
// 15ms は Web 実装（`navigator.vibrate(15)`）に揃えた軽い単発。
final class HapticsHelper {
    static final int RESULT_OK = 0;
    static final int RESULT_UNSUPPORTED = 1;
    static final int RESULT_FAILED = 2;

    private HapticsHelper() {}

    @SuppressWarnings("deprecation")
    static int tap(Context context) {
        try {
            Vibrator vibrator;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                VibratorManager manager = context.getSystemService(VibratorManager.class);
                vibrator = manager == null ? null : manager.getDefaultVibrator();
            } else {
                vibrator = (Vibrator) context.getSystemService(Context.VIBRATOR_SERVICE);
            }
            if (vibrator == null || !vibrator.hasVibrator()) {
                return RESULT_UNSUPPORTED;
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                vibrator.vibrate(
                        VibrationEffect.createOneShot(15, VibrationEffect.DEFAULT_AMPLITUDE));
            } else {
                vibrator.vibrate(15);
            }
            return RESULT_OK;
        } catch (RuntimeException e) {
            return RESULT_FAILED;
        }
    }
}
