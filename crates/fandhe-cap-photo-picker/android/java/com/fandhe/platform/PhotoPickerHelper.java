package com.fandhe.platform;

import android.app.Activity;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.PickVisualMediaRequest;
import androidx.activity.result.contract.ActivityResultContracts;

import java.io.File;
import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.Map;
import java.util.WeakHashMap;

// 写真選択（Issue #1104）の Android 実装ヘルパー（Issue #1355 で `MainActivity` から機能 crate へ移した）。
// `PhotoPickerLifecycle` が Activity ごとに生成する（Rust の `fandhe-cap-photo-picker/src/android.rs` が static メソッド
// `request(Activity, long)` を JNI で呼び、結果を `nativeOnPhotoPickerResult` で受ける）。
//
// `PickVisualMedia` は端末が対応していれば ACTION_PICK_IMAGES（権限不要のシステムピッカー）を、使えない端末では
// ACTION_OPEN_DOCUMENT（`image/*`）を自動で選ぶ。権限は要らず、マニフェスト・依存も追加しない。`registerForActivityResult` は
// STARTED より前に呼ぶ契約のため、`attach`（`onHostCreated`、onCreate 内）で登録する。キーは従来の `MainActivity` の文字列のまま
// （`fandhe:MainActivity:5`）。
//
// helper が無い（生成前・破棄後）要求は、成功にも待ちにもせず失敗（code 2）で確定する。
//
// requestId: `request(Activity, long)` が受け取った値をそのまま返す。Rust 側 `bridge` がプロセス内で一意な連番を発行して渡し、
// 現在の要求と照合する（Activity 再生成後の遅延到着は黙って捨てる）。
// code: 0=選択（name/fd が有効）／1=キャンセル／2=失敗（`launch()` 例外・fd 取得失敗・JPEG 以外の変換失敗・復号不能・長辺超過・
// helper なしのいずれも一律ここへ畳む）／3=busy（追跡中の要求がある間の新しい要求。name/fd は無効）。
// name: 固定値 "photo"（表示にもログにも出さない。`FilePickerHelper.nativeOnFilePickerResult` と署名を揃えて、Rust 側の受け取り
// （`fandhe_device::file_picker::picked_from_document`）を共有するための引数）。
// fd: `code == 0` のときのみ有効。`detachFd()` で所有権を Rust へ渡した生 fd。読み取り・8 MiB 判定・形式判定・復号確認はすべて
// Rust 側で行う（Java 側では読み取りループを書かない）。
//
// 契約は「Activity 再生成で保留中の要求は復元せず破棄」: 「いま待っている要求 id」は static で、Rust 側（`fandhe-gallery` の
// `android_main` が呼ぶ `reset_pending_tracking`）が再生成のたびに受け皿を捨てるため、再生成をまたいで届いた結果は Rust 側の id 照合で
// 捨てられる。id の保持・照合そのものは `RequestTracker`（Issue #944）へ委譲する。
final class PhotoPickerHelper {
    private static native void nativeOnPhotoPickerResult(
            long requestId, int code, String name, int fd);

    private static final RequestTracker photoPickRequest = new RequestTracker();
    // 変換前の元データに許す上限（バイト）。Rust 側 `MAX_PHOTO_BYTES` と揃える。
    private static final long PHOTO_MAX_BYTES = 8L * 1024 * 1024;
    // 変換前に許す画像の長辺（画素）。Rust 側 `MAX_PHOTO_SIDE` と揃える。
    private static final int PHOTO_MAX_SIDE = 16384;
    // 変換前に許す総画素数。Rust 側 `MAX_PHOTO_PIXELS`（64 Mi 画素）と揃える。
    private static final long PHOTO_MAX_PIXELS = 64L * 1024 * 1024;
    // JPEG へ変換するときの長辺の目標（画素）。これ以下になるまで `inSampleSize` で縮める。
    private static final int PHOTO_CONVERT_SIDE = 4096;

    private final Activity activity;
    // Activity 再生成後は `ActivityResultRegistry` が旧要求の結果を新インスタンスへ配送し得るが、`photoPickRequest.take()` が NONE なら
    // 捨て、そうでなくても Rust 側の id 照合で捨てられる（ファイル選択のランチャーと同じ理由）。
    private final ActivityResultLauncher<PickVisualMediaRequest> pickPhoto;

    // Activity ごとの helper。`PhotoPickerLifecycle`（`FeatureLifecycleProvider`）が生成時に付け、破棄時に外す。
    private static final Map<Activity, PhotoPickerHelper> HELPERS = new WeakHashMap<>();

    static void attach(ComponentActivity activity) {
        PhotoPickerHelper helper = new PhotoPickerHelper(activity);
        synchronized (PhotoPickerHelper.class) {
            HELPERS.put(activity, helper);
        }
    }

    // Activity 破棄時は表から外す。保留中の要求は static の id のまま残し、Rust 側の `reset_pending_tracking` が捨てる。
    // ただし終了（`isFinishing`）による破棄では、選択画面の結果が二度と届かないため、追跡を外して失敗（code 2）で確定する。
    static void detach(Activity activity) {
        synchronized (PhotoPickerHelper.class) {
            HELPERS.remove(activity);
        }
        if (activity.isFinishing()) {
            long requestId = photoPickRequest.take();
            if (requestId != RequestTracker.NONE) {
                nativeOnPhotoPickerResult(requestId, 2, null, -1);
            }
        }
    }

    private static synchronized PhotoPickerHelper of(Activity activity) {
        return HELPERS.get(activity);
    }

    private PhotoPickerHelper(ComponentActivity activity) {
        this.activity = activity;
        this.pickPhoto = activity.getActivityResultRegistry().register(
                "fandhe:MainActivity:5", activity, new ActivityResultContracts.PickVisualMedia(), uri -> {
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
    }

    // Rust 側 `driver.rs`（Android 分岐）から JNI 経由で呼ばれる（署名 `(Landroid/app/Activity;J)V`）。「即時 return・結果は
    // `nativeOnPhotoPickerResult`」（ファイル選択 `FilePickerHelper.request` と同じ形）。helper が無い（生成前・破棄後）ときは
    // 失敗（code 2）で確定する（結果が来ないまま待たせない）。
    static void request(Activity activity, long requestId) {
        PhotoPickerHelper helper = of(activity);
        if (helper == null) {
            nativeOnPhotoPickerResult(requestId, 2, null, -1);
            return;
        }
        helper.requestPhotoPick(requestId);
    }

    private void requestPhotoPick(long requestId) {
        // 追跡中の要求（選択画面が開いている）があれば、上書きも再 launch もせず busy を返す。上書きすると、先の画面の結果が
        // 新しい要求として届く（取り違え）。追跡は callback の `take()`・`launch()` 失敗・終了による破棄（`detach`）で必ず外れる。
        // Rust 側が要求を捨てた（`discard_photo_pick`）後も、選択画面が閉じるまでここで busy になる（OS の観測による判定）。
        if (!photoPickRequest.tryTrack(requestId)) {
            nativeOnPhotoPickerResult(requestId, 3, null, -1);
            return;
        }
        activity.runOnUiThread(() -> {
            // 積んでから実行されるまでの間に、別の要求への置き換え・終了による破棄が済んでいたら、選択画面を開かない
            // （追跡の解除と launch はどちらも UI スレッドで行うため、この照合は競合しない）。
            if (photoPickRequest.current() != requestId) {
                return;
            }
            // 破棄済みの launcher は例外を投げずに何もしないことがあり、追跡が残って結果が来ないまま待たされる。
            // この helper が表から外れている・Activity が終了中・破棄済みなら、選択画面を開けないので失敗（code 2）で確定する。
            if (activity.isFinishing() || activity.isDestroyed() || of(activity) != this) {
                if (photoPickRequest.complete(requestId)) {
                    nativeOnPhotoPickerResult(requestId, 2, null, -1);
                }
                return;
            }
            try {
                pickPhoto.launch(new PickVisualMediaRequest.Builder()
                        .setMediaType(ActivityResultContracts.PickVisualMedia.ImageOnly.INSTANCE)
                        .build());
            } catch (RuntimeException e) {
                // ピッカーが開かれずコールバックも来ない経路（ファイル選択の `FilePickerHelper.requestFilePick` と同じ）。
                // 自分の要求のときだけ外して通知する（別の経路が先に確定していれば二重にしない）。
                if (photoPickRequest.complete(requestId)) {
                    nativeOnPhotoPickerResult(requestId, 2, null, -1);
                }
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
            String type = activity.getContentResolver().getType(uri);
            ParcelFileDescriptor pfd;
            // 変換前の元データが上限超なら変換せず fd をそのまま渡し、Rust 側の上限判定に
            // `InvalidInput` で拒否させる（申告サイズは信用せず、実際に読んで数える）。
            if ("image/jpeg".equals(type) || "image/png".equals(type) || photoExceedsLimit(uri)) {
                pfd = activity.getContentResolver().openFileDescriptor(uri, "r");
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
        try (InputStream in = activity.getContentResolver().openInputStream(uri)) {
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
        ParcelFileDescriptor pfd = activity.getContentResolver().openFileDescriptor(uri, "r");
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
                copy = File.createTempFile("fandhe_photo_src_", ".bin", activity.getCacheDir());
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
            out = File.createTempFile("fandhe_photo_", ".jpg", activity.getCacheDir());
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
}
