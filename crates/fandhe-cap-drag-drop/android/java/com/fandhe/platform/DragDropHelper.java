package com.fandhe.platform;

import android.app.Activity;
import android.content.ClipData;
import android.database.Cursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.view.DragEvent;
import android.view.View;
import android.view.DragAndDropPermissions;

import androidx.activity.ComponentActivity;

import java.io.IOException;
import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;
import java.util.concurrent.atomic.AtomicInteger;

// ドラッグ & ドロップ受け取り（Issue #1016、親 #1013 の子 (b')、#1302 で MainActivity から移動）の Android 実装ヘルパー。
// 受け取り側のみ（送り側 `View.startDragAndDrop` は親 issue の方針でスコープ外）。利用者の `MainActivity` には
// 何も書かせない。DecorView へ `View.OnDragListener` を取り付け、観測した 1 件ずつのイベントを Rust 側の集約器
// （`fandhe-cap-drag-drop/src/drop_session.rs`）へ push する。要求 1 件 → 応答 1 件の `RequestTracker` 型ではなく、
// iOS（`fandhe_ios_drag_drop.m`）と同じ「セッション id を世代として push し続ける」型。
// Activity への参加は `DragDropLifecycle`（非公開 ContentProvider、`FeatureLifecycleProvider` 継承）が行う。
// 生成時に `attach`、前面へ戻るたびに `install`（DecorView の取り付け）、破棄時に `detach` を呼ぶ。
//
// 表示中の画面が drag_drop かどうかは Rust 側 `nativeDragDropIsActive()` に問い合わせる。false なら
// `ACTION_DRAG_STARTED` の時点でこのドラッグ全体を無視し、以降の native を一切呼ばない（契約表 1）。
// 読み取り（fd の読み切り・サイズ判定）は Rust 側 `fandhe_device::drag_drop::read_dropped_from` に一本化し、
// ここでは `detachFd()` で所有権を渡した生 fd を渡すだけで読み取りループを持たない。
// JNI のエクスポート名は `Java_com_fandhe_platform_DragDropHelper_*`。署名は Rust 側 `src/android.rs` と 2 か所でそろえる。
//
// 守る範囲: 1 プロセス内のセッション順序・画面外イベントの破棄・fd の所有権の受け渡し・読み取りスレッドの本数の上限。
// 守らない範囲: ドロップ処理中のプロセス死・複数ウィンドウ・DecorView で届かない端末（実機確認の事項）。
// URI・ファイル名・例外メッセージはログへ出さない（他アプリの機密文書になり得る、security.md A09）。
final class DragDropHelper {
    private static final int CODE_FILE = 0;
    private static final int CODE_FOLDER = 1;
    private static final int CODE_UNREADABLE = 2;

    // 同時に生きる読み取りスレッドの本数の上限（Rust 側 desktop の `MAX_READERS` と同じ値）。
    // ponytail: 固定の本数上限。応答しないプロバイダの読み取りが上限まで溜まると、それらが返るまで新しいドロップは
    // 全 slot が「読めない」になる。I/O を中断できる API へ移すときに見直す。
    private static final int MAX_READERS = 4;
    private static final AtomicInteger activeReaders = new AtomicInteger(0);

    // Activity ごとの helper（Activity 破棄時に必ず外す）。
    private static final Map<Activity, DragDropHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private static native boolean nativeDragDropIsActive();

    // `ACTION_DRAG_ENTERED` / `ACTION_DRAG_EXITED` から呼ぶ。`entered` は「領域内に入った」か。
    private static native void nativeOnDragDropHover(boolean entered);

    // `ACTION_DROP` の冒頭から呼ぶ。`total` 件のドロップを開始し、セッション id（世代番号を兼ねる）を返す。
    private static native long nativeOnDragDropBegin(int total);

    // 読み取りスレッドが 1 件読む **前** に呼ぶ。false なら読み取らず break する
    // （旧世代への差し替え・上限 `MAX_DROP_FILES`（Rust 側 SSOT）超過分の両方をここで打ち切る）。
    private static native boolean nativeDragDropWants(long session, int index);

    // 1 件読み終える（またはフォルダ・読めないと判定する）たびに呼ぶ。code: 0=ファイル（name/fd 必須）／1=フォルダ／
    // 2=読めない（URI 無し・上限到達による埋め・`openFileDescriptor` 失敗等を一律ここへ畳む）。fd は code == 0 のときのみ有効で、
    // `ParcelFileDescriptor.detachFd()` で所有権を Rust へ渡した生 fd（detach 後は Java 側で close しない）。
    private static native void nativeOnDragDropItem(
            long session, int index, int code, String name, int fd);

    private final ComponentActivity activity;

    // `ACTION_DRAG_STARTED` を受理したのに `ACTION_DROP` が一度も来ないまま `ACTION_DRAG_ENDED` に至ったか
    // （＝領域外で離された・取り消された）を判定するための状態。UI スレッドでのみ読み書きする。
    // `onPause` は何もしない（表示中でも pause 状態の Activity がドロップを受け取るマルチウィンドウ構成が正常なため）。
    private boolean dragAccepted;
    private boolean dropReceived;

    private final View.OnDragListener dragListener = this::onDrag;

    private DragDropHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // Activity の生成時（`DragDropLifecycle` から）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new DragDropHelper(activity));
    }

    // 前面へ戻るたび（`DragDropLifecycle` から）。DecorView へリスナーを取り付ける。同じリスナーの再設定は無害。
    // DecorView（SurfaceView ではなく）に付ける理由は、画面全体を受け口にするため。届かない端末は実機確認で見直す。
    static void install(Activity activity) {
        DragDropHelper helper = HELPERS.get(activity);
        if (helper != null) {
            activity.getWindow().getDecorView().setOnDragListener(helper.dragListener);
        }
    }

    // Activity の破棄時（`DragDropLifecycle` から）。
    static void detach(Activity activity) {
        HELPERS.remove(activity);
    }

    private boolean onDrag(View view, DragEvent event) {
        switch (event.getAction()) {
            case DragEvent.ACTION_DRAG_STARTED:
                if (!nativeDragDropIsActive()) {
                    return false;
                }
                dragAccepted = true;
                dropReceived = false;
                return true;
            case DragEvent.ACTION_DRAG_ENTERED:
                nativeOnDragDropHover(true);
                return true;
            case DragEvent.ACTION_DRAG_EXITED:
                nativeOnDragDropHover(false);
                return true;
            case DragEvent.ACTION_DROP:
                dropReceived = true;
                handleDrop(event);
                return true;
            case DragEvent.ACTION_DRAG_ENDED:
                // ドロップされないまま終わった（領域外で離された・取り消された）場合のみキャンセルを通知する。
                // `ACTION_DROP` 済みなら `handleDrop` が別途完了を通知するため、ここでは何もしない（契約表 10「三値」）。
                if (dragAccepted && !dropReceived) {
                    nativeOnDragDropHover(false);
                }
                dragAccepted = false;
                return true;
            default:
                return true;
        }
    }

    // `ACTION_DROP` を受け取った UI スレッドから呼ぶ。権限取得・セッション開始・読み取り枠の確保は同期に行い
    // （軽量な同期呼び出しのみ）、実読み取りは専用スレッドへ渡す。読み取り枠は必ずスレッド生成前に確保する
    // （生成後の判定だと、短時間に多数のドロップが届いたとき枠を取れない要求にもスレッドが立ち上がり続け、
    // スレッド数・メモリ使用量が無制限に増える。PR #1022）。
    private void handleDrop(DragEvent event) {
        ClipData clipData = event.getClipData();
        int total = clipData == null ? 0 : clipData.getItemCount();
        DragAndDropPermissions perms = activity.requestDragAndDropPermissions(event);
        long session = nativeOnDragDropBegin(total);
        if (!acquireReaderSlot()) {
            // 上限に達している: スレッドを立てず、読み込み中のまま固まった表示にせず各 slot を「読めない」で埋める。
            for (int i = 0; i < total && nativeDragDropWants(session, i); i++) {
                nativeOnDragDropItem(session, i, CODE_UNREADABLE, null, -1);
            }
            if (perms != null) {
                perms.release();
            }
            return;
        }
        new Thread(() -> readDroppedItems(session, clipData, total, perms), "fandhe-drag-drop")
                .start();
    }

    // 読み取りスレッド本体（1 ドロップにつき 1 本、読み取り枠は `handleDrop` で確保済み）。`perms` は成否を問わず
    // 必ず release する（契約表 9「失敗時のリソース解放」。`perms` は null になり得る）。
    private void readDroppedItems(
            long session, ClipData clipData, int total, DragAndDropPermissions perms) {
        try {
            try {
                for (int i = 0; i < total; i++) {
                    if (!nativeDragDropWants(session, i)) {
                        // 旧世代への差し替え、または上限超過分（`drop_session::wants` 側の判定）。
                        break;
                    }
                    deliverDropItem(session, i, clipData.getItemAt(i).getUri());
                }
            } finally {
                activeReaders.decrementAndGet();
            }
        } finally {
            if (perms != null) {
                perms.release();
            }
        }
    }

    // `MAX_READERS` 未満なら枠を 1 つ確保する（Rust 側 desktop の `ReaderSlot` と同型）。
    private static boolean acquireReaderSlot() {
        while (true) {
            int current = activeReaders.get();
            if (current >= MAX_READERS) {
                return false;
            }
            if (activeReaders.compareAndSet(current, current + 1)) {
                return true;
            }
        }
    }

    // 1 件ぶんの読み込み（フォルダ判定 → 名前解決 → fd 取得）を行い、結果を `nativeOnDragDropItem` で通知する。
    private void deliverDropItem(long session, int index, Uri uri) {
        if (uri == null) {
            nativeOnDragDropItem(session, index, CODE_UNREADABLE, null, -1);
            return;
        }
        String type;
        try {
            type = activity.getContentResolver().getType(uri);
        } catch (RuntimeException e) {
            nativeOnDragDropItem(session, index, CODE_UNREADABLE, null, -1);
            return;
        }
        if (DocumentsContract.Document.MIME_TYPE_DIR.equals(type)) {
            nativeOnDragDropItem(session, index, CODE_FOLDER, null, -1);
            return;
        }

        String name = resolveDisplayName(uri);
        // `detachFd()` は fd の所有権を Rust 側へ移す。移した後はこの `ParcelFileDescriptor` の `close()`
        // （try-with-resources の暗黙の呼び出しを含む）は no-op になる（Android の契約）。失敗時も解決済みの
        // `name` を渡す（null に畳むと、複数ドロップ混在時にどの項目が失敗したか利用者に伝わらない。PR #1022）。
        try (ParcelFileDescriptor pfd = activity.getContentResolver().openFileDescriptor(uri, "r")) {
            if (pfd == null) {
                nativeOnDragDropItem(session, index, CODE_UNREADABLE, name, -1);
                return;
            }
            int fd = pfd.detachFd();
            nativeOnDragDropItem(session, index, CODE_FILE, name, fd);
        } catch (IOException | RuntimeException e) {
            nativeOnDragDropItem(session, index, CODE_UNREADABLE, name, -1);
        }
    }

    // SAF の DISPLAY_NAME を解決する。プロバイダの `query` 自体が例外を送出しても `getLastPathSegment()` の
    // fallback へ続ける（名前解決の失敗として扱うのみ）。名前の検証は Rust 側 `sanitize_picked_name` が行う。
    // ファイル選択（`MainActivity` の同名メソッド）と同じ手順を持つが、機能 crate は `MainActivity` を参照しないため
    // ここに置く（Rust 側の検証規則は `fandhe_device::file_picker` に一本化している）。
    private String resolveDisplayName(Uri uri) {
        String name = null;
        try (Cursor cursor = activity.getContentResolver().query(
                uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int nameIndex = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (nameIndex >= 0) {
                    name = cursor.getString(nameIndex);
                }
            }
        } catch (RuntimeException e) {
            // 握りつぶして fallback へ続ける（上記コメント参照）。
        }
        if (name == null || name.isEmpty()) {
            name = uri.getLastPathSegment();
        }
        return name;
    }
}
