package com.fandhe.androidapp;

import android.app.Activity;
import android.os.Build;
import android.view.Display;
import android.view.WindowManager;

/**
 * 画面のリフレッシュレート・HDR・広色域の読み取り（親 #177、Issue #872）。
 *
 * <p>呼び出し元: Rust 側 {@code device/android_display_refresh_hdr.rs} が egui スレッドから、
 * ボタン押下ごとに {@code MainActivity.displayRefreshHdrRead()}（JNI シグネチャ {@code ()[F}）
 * 経由で 1 回だけ同期で呼ぶ。Java → Rust の native コールバックは無いため no-op スタブは不要。
 * {@code Display} の getter と {@code Configuration} の参照にスレッド制約は無い。
 * Activity は引数で受けるだけで保持せず、要求 id・保留状態・リスナーも持たない
 * （Activity 再生成で破棄すべきものは無い）。個体識別子は読まない。
 *
 * <p>戻り値 {@code float[4] {currentHz, maxHz, hdr, wideGamut}}（Rust の {@code from_android} と対）:
 * <ul>
 *   <li>currentHz: {@code getRefreshRate()} の生値（検証は Rust 側）</li>
 *   <li>maxHz: {@code getSupportedModes()} の最大。モードが 0 件なら -1</li>
 *   <li>hdr: 1 = 対応、0 = 非対応、-1 = 取れない</li>
 *   <li>wideGamut: 1 / 0。API 24・25 は取れないので -1</li>
 * </ul>
 * 失敗（Activity 破棄済み・Display 取得不可・例外）は null。
 */
final class DisplayRefreshHdrHelper {
    private DisplayRefreshHdrHelper() {}

    @SuppressWarnings("deprecation")
    static float[] read(Activity a) {
        if (a == null || a.isFinishing() || a.isDestroyed()) {
            return null;
        }
        try {
            WindowManager wm = a.getWindowManager();
            if (wm == null) {
                return null;
            }
            // Activity.getDisplay() は API 30+ のため使わない（minSdk 24）。
            Display d = wm.getDefaultDisplay();
            if (d == null) {
                return null;
            }
            float max = -1f;
            for (Display.Mode m : d.getSupportedModes()) {
                max = Math.max(max, m.getRefreshRate());
            }
            float hdr;
            float wide = -1f;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                hdr = d.isHdr() ? 1f : 0f;
                wide = a.getResources().getConfiguration().isScreenWideColorGamut() ? 1f : 0f;
            } else {
                Display.HdrCapabilities caps = d.getHdrCapabilities();
                if (caps == null) {
                    hdr = -1f;
                } else {
                    hdr = caps.getSupportedHdrTypes().length > 0 ? 1f : 0f;
                }
            }
            return new float[] {d.getRefreshRate(), max, hdr, wide};
        } catch (RuntimeException e) {
            return null;
        }
    }
}
