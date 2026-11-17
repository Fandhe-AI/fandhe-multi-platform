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
// 本クラスの責務は「ライブラリロードの宣言」に加え、
// Phase 11（Issue #1104）の写真選択（PickVisualMedia）中継（feature `device` 切り離し時に削除）。
// 機能 crate へ移した機能（カメラ撮影・書類スキャン・NFC カードエミュレーション・ホーム画面ウィジェット・
// 共有テキスト受信・ゲームコントローラ・カスタム URL scheme・ディープリンク・Web 認証セッション・アプリショートカット・
// OS の検索への登録・バックグラウンドダウンロード・バックグラウンド送信・読み上げ・ファイル選択・マイク録音・音声認識）の
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
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.icu.text.BreakIterator;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
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
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.PickVisualMediaRequest;
import androidx.activity.result.contract.ActivityResultContracts;
import com.google.androidgamesdk.GameActivity;
import java.io.File;
import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
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

    // Rust 側（crates/fandhe-egui-proxy/src/device/android.rs）の JNI エクスポート関数
    // （Issue #1104、Phase 11 型サンプル「写真選択」の Android 経路。ファイル選択
    // （`fandhe-cap-file-picker` の `FilePickerHelper.nativeOnFilePickerResult`）と同型の pull 型で、
    // `FilePicker`/`PhotoPicker` trait は通らない）。
    // requestId: `requestPhotoPick(long)` が受け取った値をそのまま返す（Rust 側 `bridge` が
    // 現在の要求と照合し、Activity 再生成後の遅延到着は黙って捨てる）。
    // code: 0=選択（name/fd が有効）／1=キャンセル／2=失敗（`launch()` 例外・fd 取得失敗・
    // JPEG 以外の変換失敗・復号不能・長辺超過のいずれも一律ここへ畳む）。
    // name: 固定値 "photo"（表示にもログにも出さない。`FilePickerHelper.nativeOnFilePickerResult` と署名を
    // 揃えて Rust 側の受け取り（`fandhe_device::file_picker::picked_from_document`）を共有するための引数）。
    // fd: `code == 0` のときのみ有効。`detachFd()` で所有権を Rust へ渡した生 fd。
    // 読み取り・8 MiB 判定・形式判定・復号確認はすべて Rust 側で行う。
    private static native void nativeOnPhotoPickerResult(
            long requestId, int code, String name, int fd);

    // 写真選択（Issue #1104）の「いま待っている要求 id」。ファイル選択の要求 id と同じく static。
    // ただし契約は「Activity 再生成で保留中の要求は復元せず破棄」: Rust 側
    // （`android_main`）が再生成のたびに受け皿を捨てるため、再生成をまたいで届いた結果は
    // Rust 側の id 照合で捨てられる。
    private static final RequestTracker photoPickRequest = new RequestTracker();
    // 変換前の元データに許す上限（バイト）。Rust 側 `MAX_PHOTO_BYTES` と揃える。
    private static final long PHOTO_MAX_BYTES = 8L * 1024 * 1024;
    // 変換前に許す画像の長辺（画素）。Rust 側 `MAX_PHOTO_SIDE` と揃える。
    private static final int PHOTO_MAX_SIDE = 16384;
    // 変換前に許す総画素数。Rust 側 `MAX_PHOTO_PIXELS`（64 Mi 画素）と揃える。
    private static final long PHOTO_MAX_PIXELS = 64L * 1024 * 1024;
    // JPEG へ変換するときの長辺の目標（画素）。これ以下になるまで `inSampleSize` で縮める。
    private static final int PHOTO_CONVERT_SIDE = 4096;

    @Override
    protected void onStart() {
        super.onStart();
    }


    // 機能 crate へ移した機能（読み上げ・ファイル選択・マイク録音・音声認識を含む）は、機能 crate が JNI の入口（static）と
    // Activity への参加（非公開 provider）を持つため、ここには何も書かない
    // （#1257・#1354、docs/architecture/consumer-app-structure.md §Android の MainActivity）。

    // 写真選択（Issue #1104）。`PickVisualMedia` は端末が対応していれば ACTION_PICK_IMAGES
    // （権限不要のシステムピッカー）を、使えない端末では ACTION_OPEN_DOCUMENT（`image/*`）を
    // 自動で選ぶ。Activity 再生成後は `ActivityResultRegistry` が旧要求の結果を新インスタンスへ
    // 配送し得るが、`photoPickRequest.take()` が NONE なら捨て、そうでなくても Rust 側の
    // id 照合で捨てられる（ファイル選択のランチャーと同じ理由）。
    private final ActivityResultLauncher<PickVisualMediaRequest> pickPhoto =
            getActivityResultRegistry().register("fandhe:MainActivity:5", this, new ActivityResultContracts.PickVisualMedia(), uri -> {
                long requestId = photoPickRequest.take();
                if (requestId == RequestTracker.NONE) {
                    return;
                }
                if (uri == null) {
                    nativeOnPhotoPickerResult(requestId, 1, null, -1);
                    return;
                }
                // 変換・fd 取得を UI スレッドでやらない（`deliverPickedPhoto` 参照）。
                new Thread(() -> deliverPickedPhoto(requestId, uri), "fandhe-photo-pick").start();
            });

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

    // Rust 側 `device/photo_pick.rs`（Android 分岐）から JNI 経由で呼ばれる（Issue #1104、
    // 署名 `(J)V`）。「即時 return・結果は `nativeOnPhotoPickerResult`」（ファイル選択 `FilePickerHelper.request` と同じ形）。
    // 権限は要らず、マニフェスト・依存も追加しない。
    private void requestPhotoPick(long requestId) {
        photoPickRequest.track(requestId);
        runOnUiThread(() -> {
            try {
                pickPhoto.launch(new PickVisualMediaRequest.Builder()
                        .setMediaType(ActivityResultContracts.PickVisualMedia.ImageOnly.INSTANCE)
                        .build());
            } catch (RuntimeException e) {
                // ピッカーが開かれずコールバックも来ない経路（ファイル選択の `FilePickerHelper.requestFilePick` と同じ）。
                photoPickRequest.take();
                nativeOnPhotoPickerResult(requestId, 2, null, -1);
            }
        });
    }

    // 選ばれた写真を Rust へ渡す（ワーカースレッド）。JPEG / PNG は fd をそのまま渡し、それ以外
    // （HEIC・GIF・WebP 等。Rust の復号器は JPEG / PNG のみ）は JPEG の一時ファイルへ変換して
    // 渡す。MIME は OS の自己申告で、最終的な形式判定は Rust 側 `validate_photo` が行う。
    // 一時ファイルは成功・失敗のどちらでも削除する（Rust は呼び出しの中で読み切る）。
    // 変換の失敗（復号不能・OOM・長辺超過）は code 2。Uri・例外メッセージはログへ出さない（A09）。
    private void deliverPickedPhoto(long requestId, Uri uri) {
        File converted = null;
        try {
            String type = getContentResolver().getType(uri);
            ParcelFileDescriptor pfd;
            // 変換前の元データが上限超なら変換せず fd をそのまま渡し、Rust 側の上限判定に
            // `InvalidInput` で拒否させる（申告サイズは信用せず、実際に読んで数える）。
            if ("image/jpeg".equals(type) || "image/png".equals(type) || photoExceedsLimit(uri)) {
                pfd = getContentResolver().openFileDescriptor(uri, "r");
            } else {
                converted = convertPhotoToJpeg(uri);
                pfd = converted == null
                        ? null
                        : ParcelFileDescriptor.open(converted, ParcelFileDescriptor.MODE_READ_ONLY);
            }
            if (pfd == null) {
                nativeOnPhotoPickerResult(requestId, 2, null, -1);
                return;
            }
            // `detachFd()` で所有権が Rust へ移った後の close は no-op（ファイル選択の
            // `FilePickerHelper.deliverPickedDocument` と同じ契約）。detach 前に失敗した場合は try-with-resources が閉じる。
            try (ParcelFileDescriptor owned = pfd) {
                int fd = owned.detachFd();
                nativeOnPhotoPickerResult(requestId, 0, "photo", fd);
            }
        } catch (IOException | RuntimeException | OutOfMemoryError e) {
            nativeOnPhotoPickerResult(requestId, 2, null, -1);
        } finally {
            if (converted != null) {
                converted.delete();
            }
        }
    }

    // 元データが PHOTO_MAX_BYTES を超えるか。最大 PHOTO_MAX_BYTES + 1 バイトまで読んで数える。
    private boolean photoExceedsLimit(Uri uri) throws IOException {
        try (InputStream in = getContentResolver().openInputStream(uri)) {
            if (in == null) {
                return false;
            }
            byte[] buf = new byte[64 * 1024];
            long total = 0;
            int n;
            while ((n = in.read(buf)) > 0) {
                total += n;
                if (total > PHOTO_MAX_BYTES) {
                    return true;
                }
            }
            return false;
        }
    }

    // 画像を JPEG（品質 90）の一時ファイルへ変換する。失敗は null（呼び出し側が code 2 にする）。
    // ponytail: 長辺を PHOTO_CONVERT_SIDE 以下へ縮めて復号する（変換したときだけ。報告される
    // 寸法は変換後の値）。透明は JPEG で黒になる。必要になったら背景色を指定する。
    private File convertPhotoToJpeg(Uri uri) throws IOException {
        // HEIC / WebP のデコーダはシーク可能な入力を要する。openInputStream は非シーク
        // （パイプ等）になり得るため、fd（シーク可能なら）か上限付き一時コピーから復号する。
        ParcelFileDescriptor pfd = getContentResolver().openFileDescriptor(uri, "r");
        if (pfd == null) {
            return null;
        }
        File copy = null;
        try {
            FileDescriptor src = pfd.getFileDescriptor();
            boolean seekable = true;
            try {
                Os.lseek(src, 0, OsConstants.SEEK_CUR);
            } catch (ErrnoException e) {
                seekable = false;
            }
            if (!seekable) {
                copy = File.createTempFile("fandhe_photo_src_", ".bin", getCacheDir());
                long total = 0;
                try (InputStream in = new ParcelFileDescriptor.AutoCloseInputStream(pfd);
                        FileOutputStream os = new FileOutputStream(copy)) {
                    byte[] buf = new byte[64 * 1024];
                    int n;
                    while ((n = in.read(buf)) > 0) {
                        total += n;
                        if (total > PHOTO_MAX_BYTES) {
                            return null;
                        }
                        os.write(buf, 0, n);
                    }
                }
                try (ParcelFileDescriptor cpfd =
                        ParcelFileDescriptor.open(copy, ParcelFileDescriptor.MODE_READ_ONLY)) {
                    return decodeAndEncodeJpeg(cpfd.getFileDescriptor());
                }
            }
            return decodeAndEncodeJpeg(src);
        } finally {
            try {
                pfd.close();
            } catch (IOException ignored) {
                // 二重 close（AutoCloseInputStream 経由）や close 失敗は無視してよい。
            }
            if (copy != null) {
                copy.delete();
            }
        }
    }

    // シーク可能な fd から復号して JPEG 一時ファイルへ書く。失敗は null。
    private File decodeAndEncodeJpeg(FileDescriptor fd) throws IOException {
        BitmapFactory.Options bounds = new BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        BitmapFactory.decodeFileDescriptor(fd, null, bounds);
        int longSide = Math.max(bounds.outWidth, bounds.outHeight);
        if (longSide <= 0 || longSide > PHOTO_MAX_SIDE) {
            return null;
        }
        // 総画素数も復号前に検査する（Rust 側 `MAX_PHOTO_PIXELS`・iOS 実装と揃える）。
        if ((long) bounds.outWidth * (long) bounds.outHeight > PHOTO_MAX_PIXELS) {
            return null;
        }
        BitmapFactory.Options opts = new BitmapFactory.Options();
        // 既定値は 0（未設定）で、割り算の分母にできない。
        opts.inSampleSize = 1;
        while (longSide / opts.inSampleSize > PHOTO_CONVERT_SIDE) {
            opts.inSampleSize *= 2;
        }
        Bitmap bitmap = null;
        File out = null;
        boolean done = false;
        try {
            bitmap = BitmapFactory.decodeFileDescriptor(fd, null, opts);
            if (bitmap == null) {
                return null;
            }
            out = File.createTempFile("fandhe_photo_", ".jpg", getCacheDir());
            try (FileOutputStream os = new FileOutputStream(out)) {
                if (!bitmap.compress(Bitmap.CompressFormat.JPEG, 90, os)) {
                    return null;
                }
            }
            // 所有権を呼び出し側へ移す。以降の削除は呼び出し側の責務。
            done = true;
            return out;
        } finally {
            if (bitmap != null) {
                bitmap.recycle();
            }
            // 成功以外の全終了経路（OOM を含む）で一時ファイルを消す。
            if (!done && out != null) {
                out.delete();
            }
        }
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
