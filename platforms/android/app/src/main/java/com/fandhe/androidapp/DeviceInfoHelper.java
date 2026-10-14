package com.fandhe.androidapp;

import android.app.ActivityManager;
import android.content.Context;
import android.os.Build;
import android.os.Environment;
import android.os.StatFs;

// 端末・OS 情報画面（Issue #1306、親 #1131）の Android 実装ヘルパー。MainActivity.deviceInfoRead()
// から委譲される。Rust の `device/android_device_info.rs` が egui スレッドからボタン押下ごとに
// JNI（`()Ljava/lang/String;`）で 1 回呼ぶ同期 getter で、Java -> Rust の native コールバックは
// 無い（no-op スタブ不要）。
//
// 戻り値は 4 項目を U+001F（Unit Separator）でつないだ文字列。Rust 側 `from_android` と対で決める:
//   1 OS 版        Build.VERSION.RELEASE。不明は空文字
//   2 機種         Build.MANUFACTURER + " " + Build.MODEL。片方だけでも可。不明は空文字
//   3 メモリ総量   ActivityManager.MemoryInfo.totalMem（バイト、10 進）。不明は空文字
//                  カーネル予約分が引かれるため、広告上の RAM 容量より小さい
//   4 空き容量     StatFs(データ領域).getAvailableBytes()（バイト、10 進）。不明は空文字、0 は有効値
// 文字列項目からは U+001F を除いてから詰める。失敗（RuntimeException）は null。
//
// 権限は不要（READ_PHONE_STATE 等は宣言しない）。個体識別子（ANDROID_ID・シリアル・広告 ID・
// MAC・端末名）は読まない。3 つの API はスレッド制約が無く android_main スレッドから呼べる。
// Activity は握らず ApplicationContext だけを使い、要求 id・保留状態も持たないので、Activity
// 再生成で破棄すべきものは無い。
// 守る範囲: 不明・0 バイト・失敗の区別。守らない範囲: 読み取り後の変化の反映（再読込まで）。
final class DeviceInfoHelper {
    private static final char SEP = '\u001F';

    private DeviceInfoHelper() {}

    static String read(Context context) {
        try {
            String version = text(Build.VERSION.RELEASE);
            String maker = text(Build.MANUFACTURER);
            String model = text(Build.MODEL);
            String device = maker.isEmpty() ? model : (model.isEmpty() ? maker : maker + " " + model);

            String memory = "";
            ActivityManager am = (ActivityManager) context.getSystemService(Context.ACTIVITY_SERVICE);
            if (am != null) {
                ActivityManager.MemoryInfo mi = new ActivityManager.MemoryInfo();
                am.getMemoryInfo(mi);
                if (mi.totalMem > 0) {
                    memory = Long.toString(mi.totalMem);
                }
            }

            String free = "";
            try {
                free = Long.toString(new StatFs(Environment.getDataDirectory().getPath()).getAvailableBytes());
            } catch (RuntimeException e) {
                // この項目だけ不明にする
            }
            return version + SEP + device + SEP + memory + SEP + free;
        } catch (RuntimeException e) {
            return null;
        }
    }

    // null・Build.UNKNOWN は空、区切り文字は除く。
    private static String text(String s) {
        if (s == null || Build.UNKNOWN.equals(s)) {
            return "";
        }
        return s.replace(String.valueOf(SEP), "");
    }
}
