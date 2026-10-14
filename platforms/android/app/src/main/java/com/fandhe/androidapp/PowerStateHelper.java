package com.fandhe.androidapp;

import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.os.BatteryManager;
import android.os.Build;
import android.os.PowerManager;

// 電源状態画面（Issue #1276）の Android 実装ヘルパー。MainActivity.powerStateRead() から
// 委譲される。Rust の `device/android_power_state.rs` が egui スレッドから JNI（`()J`）で
// 1 秒に 1 回呼ぶ同期 getter で、Java -> Rust の native コールバックは無い（no-op スタブ不要）。
//
// 戻り値は 5 項目を 1 つの long に詰めた値（各 8bit、不明は 255）。Rust 側 `unpack` と対で決める:
//   bit 0-7   電池の有無   0 = 無し, 1 = あり, 255 = 不明
//   bit 8-15  残量 %       0..100, 255 = 不明
//   bit 16-23 充電状態     BatteryManager.EXTRA_STATUS の生値 (1..5), 255 = 不明
//   bit 24-31 省電力       0 / 1, 255 = 不明
//   bit 32-39 熱           PowerManager.THERMAL_STATUS_* の生値 (0..6), 255 = 不明（API 28 以下）
// 失敗（RuntimeException）は -1。成功値へ畳まない。
//
// registerReceiver(null, ...) は sticky intent を返すだけで receiver を登録しないため解除不要。
// 3 つの API はいずれも binder 呼び出しでスレッド制約・Looper 要件が無く、android_main
// スレッドから呼べる。権限は不要（BATTERY_STATS は宣言しない）。Activity は握らず
// ApplicationContext だけを使うので、Activity 再生成で保留する要求も状態も無い。
// 守る範囲: 1 回の呼び出し内で同じ時点の値を返すこと。守らない範囲: 呼び出し間隔内の変化の反映。
final class PowerStateHelper {
    private static final long UNKNOWN = 255L;

    private PowerStateHelper() {}

    static long read(Context context) {
        try {
            long present = UNKNOWN;
            long percent = UNKNOWN;
            long status = UNKNOWN;
            Intent i = context.registerReceiver(null, new IntentFilter(Intent.ACTION_BATTERY_CHANGED));
            if (i != null) {
                if (i.hasExtra(BatteryManager.EXTRA_PRESENT)) {
                    present = i.getBooleanExtra(BatteryManager.EXTRA_PRESENT, false) ? 1L : 0L;
                }
                int level = i.getIntExtra(BatteryManager.EXTRA_LEVEL, -1);
                int scale = i.getIntExtra(BatteryManager.EXTRA_SCALE, -1);
                // level > scale だと percent が 100 を超え、8bit フィールドから溢れて
                // 隣の充電状態へ混入する。範囲外は不明のまま返す。
                if (level >= 0 && scale > 0 && level <= scale) {
                    percent = (long) level * 100L / scale;
                }
                int st = i.getIntExtra(BatteryManager.EXTRA_STATUS, -1);
                if (st >= 0 && st < 255) {
                    status = st;
                }
            }
            long lowPower = UNKNOWN;
            long thermal = UNKNOWN;
            PowerManager pm = context.getSystemService(PowerManager.class);
            if (pm != null) {
                lowPower = pm.isPowerSaveMode() ? 1L : 0L;
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    thermal = pm.getCurrentThermalStatus();
                }
            }
            return present | (percent << 8) | (status << 16) | (lowPower << 24) | (thermal << 32);
        } catch (RuntimeException e) {
            return -1L;
        }
    }
}
