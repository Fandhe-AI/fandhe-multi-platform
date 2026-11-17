// GameActivity サブクラス（TASK-9.3-S1 / REQ-9 / M4、Issue #242）。
//
// android-activity（"android-game-activity" feature、crates/fandhe-gallery・
// fandhe-winit-ime の Cargo.toml 参照）は GameActivity バックエンドを使うが、
// GameActivity 自体は Java/Kotlin 側でサブクラス化しないと動作しない
// （com.google.androidgamesdk.GameActivity は抽象的な起動口のみを提供し、
// NativeActivity と異なりネイティブライブラリの自動ロードを行わない。
// developer.android.com/games/agdk/game-activity/get-started の
// 「Implement GameActivity in your game」節、取得日 2026-08-01。
// 決定記録: docs/architecture/android-activity-method.md）。
//
// 本クラスの責務は「ライブラリロードの宣言」と a11y 無効時のイベント遮断だけ。
// 機能 crate へ移した機能（カメラ撮影・書類スキャン・NFC カードエミュレーション・ホーム画面ウィジェット・
// 共有テキスト受信・ゲームコントローラ・カスタム URL scheme・ディープリンク・Web 認証セッション・アプリショートカット・
// OS の検索への登録・バックグラウンドダウンロード・バックグラウンド送信・読み上げ・ファイル選択・マイク録音・音声認識・写真選択）の
// JNI 入口と Activity への参加は、各 crate の Java（static 入口と非公開 provider）が持つ
// ためここには無い（Issue #1257・#1354）。
// window insets 中継（現在は共通層 `HostInsets` が持つ、#1327）の経緯（Issue #579: `AndroidApp::content_rect()` は GameActivity
// 経路（android-activity 0.6.1、Pixel 10a / Android 16 実機）で insets を反映せず常に
// 画面全体を返すことを logcat で実測確認した。`GameActivity_getWindowInsets` C API は
// `GameActivity*` 構造体ポインタを要求するが、Rust 公開 API（`activity_as_ptr()` =
// JNI jobject）からは得られないため、Java 側で `WindowInsetsCompat` を取得し
// native メソッドで Rust 側 static へ橋渡しする以外に手段がない）。
// これ以外の実描画・入力処理はすべて cdylib 側（fandhe-gallery が公開する
// android_main、crates/fandhe-gallery/src/lib.rs 参照）に委譲する
// （Java/Kotlin コードを最小限に保つ方針は維持）。
// AndroidManifest.xml の android:name はこのクラスを指す。
package com.fandhe.platform;

import android.app.PendingIntent;
import android.content.ActivityNotFoundException;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.IntentFilter;
import android.content.IntentSender;
import android.content.SharedPreferences;
import android.icu.text.BreakIterator;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.os.VibratorManager;
import android.provider.CalendarContract;
import android.provider.DocumentsContract;
import android.view.DragAndDropPermissions;
import android.view.DragEvent;
import android.view.Surface;
import android.view.View;
import android.view.ViewGroup;
import android.view.ViewParent;
import android.view.accessibility.AccessibilityEvent;
import android.view.accessibility.AccessibilityManager;
import com.google.androidgamesdk.GameActivity;
import java.util.Calendar;
import java.util.GregorianCalendar;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.atomic.AtomicInteger;

public class MainActivity extends GameActivity {
    static {
        // ライブラリ名は AndroidManifest.xml の android.app.lib_name meta-data
        // （値 "fandhe_gallery"）および crates/fandhe-gallery/Cargo.toml の
        // [lib] name と一致させること（3 箇所の SSOT はこのライブラリ名文字列）。
        System.loadLibrary("fandhe_gallery");
    }

    @Override
    protected void onStart() {
        super.onStart();
    }


    // 機能 crate へ移した機能（読み上げ・ファイル選択・マイク録音・音声認識・写真選択を含む）は、機能 crate が JNI の入口（static）と
    // Activity への参加（非公開 provider）を持つため、ここには何も書かない
    // （#1257・#1354・#1355、docs/architecture/consumer-app-structure.md §Android の MainActivity）。

    /**
     * a11y 無効時の accesskit イベント送信を親 View で止める（Issue #2）。
     *
     * <p>役割: accesskit_android は `send_completed_event` で UI スレッドから
     * `host.getParent().requestSendAccessibilityEvent` を呼び、結果を `unwrap()` する。a11y
     * サービスが一時接続して adapter が活性化したまま a11y が OFF に戻ると、Android が
     * `IllegalStateException("Accessibility off")` を投げ、Rust の panic（SIGABRT）になる。
     * host（`mSurfaceView`）の親 FrameLayout で、Android 標準の `View.sendAccessibilityEvent`
     * と同じ `isEnabled()` 確認を行い、無効なら false を返して親への伝播（例外）を断つ。
     *
     * <p>守備範囲: UI スレッド直列・1 プロセス内。a11y OFF 中のイベント破棄は Android 標準と
     * 同じ扱い。upstream の accesskit_android が修正されたら外す。イベント内容はログに出さない。
     */
    private void installAccessibilityOffGuard() {
        ViewParent parent = mSurfaceView != null ? mSurfaceView.getParent() : null;
        if (!(parent instanceof ViewGroup)) {
            return;
        }
        final AccessibilityManager a11y = getSystemService(AccessibilityManager.class);
        ((ViewGroup) parent).setAccessibilityDelegate(new View.AccessibilityDelegate() {
            @Override
            public boolean onRequestSendAccessibilityEvent(
                    ViewGroup host, View child, AccessibilityEvent event) {
                if (a11y != null && !a11y.isEnabled()) {
                    return false;
                }
                return super.onRequestSendAccessibilityEvent(host, child, event);
            }
        });
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        installAccessibilityOffGuard();
    }

    // WorkManager の `Operation`（背景アップロードの予約・取り消し）の結果を、確定するまで待つ。
    // 待ち上限を設けると、上限を過ぎた後に操作が確定して戻り値と実際の予約が
    // 食い違い、取り消しで巻き戻そうとすると置き換え前の予約まで失う（PR #1052
    // codex 指摘）。呼び出し元（Rust 側 `AndroidBackgroundTask`）は UI スレッド外の
    // 作業スレッドから呼ぶため、無期限に待っても画面は固まらない（システムトレイと
    // 同じ判断）。割込みでも中断せず
    // 確定まで待ち、割込み状態は最後に戻す。戻り値: 0 = 成功、1 = 失敗。
    static int awaitOperation(androidx.work.Operation operation) {
        boolean interrupted = false;
        try {
            while (true) {
                try {
                    operation.getResult().get();
                    return 0;
                } catch (InterruptedException e) {
                    interrupted = true;
                } catch (ExecutionException | RuntimeException e) {
                    return 1;
                }
            }
        } finally {
            if (interrupted) {
                Thread.currentThread().interrupt();
            }
        }
    }
}
