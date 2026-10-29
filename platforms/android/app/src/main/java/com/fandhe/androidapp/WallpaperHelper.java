package com.fandhe.androidapp;

import android.app.WallpaperManager;
import android.content.Context;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

// 壁紙・ロック画面の設定画面（Issue #971、親 #188）の Java 側ヘルパー。Rust 側
// `device/android_wallpaper.rs` の `begin_set` が `MainActivity.requestWallpaperSet` 経由で
// `set` を呼び、結果を `nativeOnWallpaperResult` で返す（`device/wallpaper.rs::bridge` と
// pull 型の橋渡し）。MainActivity を肥大させないため機能ごとに分けたクラス。
//
// 結果コード表の SSOT（Rust 側 `decode` が同じ値で写像する）:
//   0 = 設定できた / 1 = 端末が壁紙設定に非対応（isWallpaperSupported=false）
//   / 2 = 利用者・管理者が禁止（isSetWallpaperAllowed=false）/ 3 = 戻り値 0・例外・未知の target。
// target: 0 = ホーム（FLAG_SYSTEM）/ 1 = ロック（FLAG_LOCK）。minSdk 24 で全 API が使える。
//
// 権限は normal の SET_WALLPAPER（実行時ダイアログなし）。I/O は単一スレッドの直列キューへ
// 集約し、描画スレッドを止めない。Activity は握らない（application context のみ）ため
// 再生成で参照が切れない。
// 守る範囲: 1 プロセス内の順序、嘘の成功の禁止、完了通知を 1 回だけ反映。
// 守らない範囲: 設定後に利用者や他アプリが壁紙を変えること、Android 12 以降の壁紙変更に伴う
// Activity 再生成で結果の表示が消えること。
// 画像バイト列・例外の文言はログへ出さない（security.md A09）。
// no-op スタブは置かない: native は Rust の `requestWallpaperSet` を経た後でしか呼ばれない。
final class WallpaperHelper {
    static final int TARGET_HOME = 0;
    static final int TARGET_LOCK = 1;

    static final int RESULT_SET = 0;
    static final int RESULT_UNSUPPORTED = 1;
    static final int RESULT_DISALLOWED = 2;
    static final int RESULT_FAILED = 3;

    private static final RequestTracker TRACKER = new RequestTracker();
    private static final ExecutorService EXECUTOR = Executors.newSingleThreadExecutor();

    private WallpaperHelper() {}

    static native void nativeOnWallpaperResult(long requestId, int target, int code);

    /** 設定を直列キューへ積んで即 return する。結果は `nativeOnWallpaperResult` で届く。 */
    static void set(Context appContext, long requestId, byte[] png, int target) {
        TRACKER.track(requestId);
        EXECUTOR.execute(() -> {
            int code = doSet(appContext, png, target);
            // 新しい要求に差し替えられた旧要求は通知しない（last-wins。Rust 側 RequestSlot.begin と整合）。
            if (TRACKER.complete(requestId)) {
                nativeOnWallpaperResult(requestId, target, code);
            }
        });
    }

    private static int doSet(Context appContext, byte[] png, int target) {
        int flag;
        if (target == TARGET_HOME) {
            flag = WallpaperManager.FLAG_SYSTEM;
        } else if (target == TARGET_LOCK) {
            flag = WallpaperManager.FLAG_LOCK;
        } else {
            return RESULT_FAILED;
        }
        try {
            WallpaperManager wm = WallpaperManager.getInstance(appContext);
            if (wm == null) {
                return RESULT_FAILED;
            }
            if (!wm.isWallpaperSupported()) {
                return RESULT_UNSUPPORTED;
            }
            if (!wm.isSetWallpaperAllowed()) {
                return RESULT_DISALLOWED;
            }
            try (ByteArrayInputStream in = new ByteArrayInputStream(png)) {
                // allowBackup=false: 見本画像を端末移行の復元に乗せない。戻り値 0 は失敗。
                return wm.setStream(in, null, false, flag) == 0 ? RESULT_FAILED : RESULT_SET;
            }
        } catch (IOException | RuntimeException e) {
            return RESULT_FAILED;
        }
    }
}
