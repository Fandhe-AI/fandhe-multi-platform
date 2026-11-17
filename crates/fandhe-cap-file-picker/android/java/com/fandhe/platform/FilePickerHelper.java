package com.fandhe.platform;

import android.app.Activity;
import android.database.Cursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;

import java.io.IOException;
import java.util.Map;
import java.util.WeakHashMap;

// ファイル選択（Issue #874、案 B）の Android 実装ヘルパー（Issue #1354 で `MainActivity` から機能 crate へ移した）。
// `FilePickerLifecycle` が Activity ごとに生成する（Rust の `fandhe-cap-file-picker/src/android.rs` が static メソッド
// `request(Activity, long)` を JNI で呼び、結果を `nativeOnFilePickerResult` で受ける）。
//
// 案 B: OS 側は「提示・1 回だけの通知・後始末」のみを担い、`FilePicker` trait は通らない（`crate::contract` の doc の
// 「モバイルは trait を通らない」参照）。Storage Access Framework の `ACTION_OPEN_DOCUMENT` + `CATEGORY_OPENABLE` を組み立てる
// `OpenDocument` contract を使う。`registerForActivityResult` は STARTED より前に呼ぶ契約のため、`attach`（`onHostCreated`、
// onCreate 内）で登録する。キーは従来の `MainActivity` の文字列のまま（再生成をまたいで保留中の結果を同じ登録へ配送するため）。
//
// helper が無い（生成前・破棄後）要求は、成功にも待ちにもせず失敗（code 2）で確定する。
//
// requestId: `request(Activity, long)` が受け取った値をそのまま返す。Rust 側 `bridge` がプロセス内で一意な連番を発行して渡す
// （Activity 再生成で画面状態が作り直され generation が 1 から再開するため、generation をそのまま OS へ渡すと旧要求の遅延完了が
// 新要求に一致してしまう。`bridge::deliver` が現在追跡中の requestId と不一致 =
// プロセス死後の再配送・旧要求の遅延到着なら黙って捨てる）。
// code: 0=選択（name/fd が有効）／1=キャンセル／2=失敗（プロバイダなし・名前解決不能・`launch()` 例外・fd 取得失敗のいずれも
// 一律ここへ畳む。詳細な原因分類は行わない）。
// fd: `code == 0` のときのみ有効。`ParcelFileDescriptor.detachFd()` で所有権を Rust へ渡した生 fd（Android は原本パスを持たないため、
// パス文字列ではなく fd を渡す）。detach 後は Java 側で close しない。読み取り・8 MiB 判定は Rust 側
// `fandhe_device::file_picker::read_picked_from` に一本化する（Java 側では読み取りループを書かない）。
//
// 「いま待っている要求 id」は static: 「進行中か」「中断されたか」を表すフラグ・ロックは持たない（単一飛行はコアが
// 「ファイルを選ぶ」ボタンを隠すことで保証済み）。画面回転等の Activity 再生成は同一プロセス内で新しい helper を作るが、
// `ActivityResultRegistry` は再生成前から保留中だった選択結果を新 helper の callback へ実際に配送し得る。id を instance field に
// すると新 helper では `NONE` に初期化され、実際に届く結果を「`NONE` なら捨てる」判定で誤って破棄し、コアが `Picking` のまま
// 固着する（画面回転中に選択、という頻出シナリオ）。static なら再生成を跨いで生き残るため、新 helper の callback でも requestId が
// 一致し正しく反映される。プロセス死では `NONE` に戻るため、死後に OS が新プロセスへ配送してきた旧結果は
// 「`NONE` なら捨てる」判定で無視される（Rust 側 `bridge` もプロセスと運命を共にし空になるため、二重に防御される）。
// 書き込みは `request` を呼ぶネイティブスレッド（`android_main`）。直後の `runOnUiThread` の post が happens-before を張るため、
// UI スレッド（callback）から正しく見える。id の保持・照合そのものは `RequestTracker`（Issue #944）へ委譲する。
final class FilePickerHelper {
    private static native void nativeOnFilePickerResult(
            long requestId, int code, String name, int fd);

    private static final RequestTracker filePickRequest = new RequestTracker();

    private final Activity activity;
    // Storage Access Framework の「開く」ダイアログ。Activity 再生成後も同一キーで再登録されるため、`ActivityResultRegistry` は
    // 旧 helper で保留中だった選択結果を新 helper のこの callback へ実際に配送し得る。`filePickRequest.take()` で読み取りと同時に
    // `NONE` へ戻すことで、(1) プロセス死後の再配送（新プロセスでは `NONE` のまま）と (2) 対応する要求が既に他の経路で消費済みの
    // 重複 callback の両方を「`NONE` なら捨てる」一箇所の判定で弾く。requestId は Rust 側の照合にそのまま渡すため、ここでは
    // 「追跡中か」以上の判定をしない。
    private final ActivityResultLauncher<String[]> pickDocument;

    // Activity ごとの helper。`FilePickerLifecycle`（`FeatureLifecycleProvider`）が生成時に付け、破棄時に外す。
    private static final Map<Activity, FilePickerHelper> HELPERS = new WeakHashMap<>();

    static void attach(ComponentActivity activity) {
        FilePickerHelper helper = new FilePickerHelper(activity);
        synchronized (FilePickerHelper.class) {
            HELPERS.put(activity, helper);
        }
    }

    // Activity 破棄時は表から外すだけ。保留中の要求は static の id のまま残し、再生成後の新 helper が結果を拾う
    // （終了による破棄では結果が届かず `Picking` のまま残る。固定タイムアウトを掛けない既知の天井）。
    static void detach(Activity activity) {
        synchronized (FilePickerHelper.class) {
            HELPERS.remove(activity);
        }
    }

    private static synchronized FilePickerHelper of(Activity activity) {
        return HELPERS.get(activity);
    }

    private FilePickerHelper(ComponentActivity activity) {
        this.activity = activity;
        this.pickDocument = activity.getActivityResultRegistry().register(
                "fandhe:MainActivity:4", activity, new ActivityResultContracts.OpenDocument(), uri -> {
                    long requestId = filePickRequest.take();
                    if (requestId == RequestTracker.NONE) {
                        return;
                    }
                    if (uri == null) {
                        // ユーザーがピッカーを戻る操作でキャンセルした場合。
                        nativeOnFilePickerResult(requestId, 1, null, -1);
                        return;
                    }
                    // 名前解決・fd 取得を UI スレッドでやらない（`deliverPickedDocument` 参照）。
                    new Thread(() -> deliverPickedDocument(requestId, uri), "fandhe-file-pick")
                            .start();
                });
    }

    // Rust 側 bridge（`driver.rs`）の `drive_mobile_with` から JNI 経由で呼ばれる（Issue #874、案 B）。`android_main` は Java の UI
    // スレッドとは別のネイティブスレッドで走るため、`ActivityResultLauncher` の起動を `runOnUiThread` へ委譲する。呼び出し元は
    // 即時 return（FFI）で、結果は非同期に `nativeOnFilePickerResult` で通知する。helper が無い（生成前・破棄後）ときは
    // 失敗（code 2）で確定する（結果が来ないまま待たせない）。
    static void request(Activity activity, long requestId) {
        FilePickerHelper helper = of(activity);
        if (helper == null) {
            nativeOnFilePickerResult(requestId, 2, null, -1);
            return;
        }
        helper.requestFilePick(requestId);
    }

    private void requestFilePick(long requestId) {
        filePickRequest.track(requestId);
        activity.runOnUiThread(() -> {
            try {
                pickDocument.launch(new String[] {"*/*"});
            } catch (RuntimeException e) {
                // ドキュメントプロバイダが端末に存在しない、または `launch()` 自体が例外を送出した場合
                // （`onSaveInstanceState` 後の呼び出し等）。ピッカーは開かれておらず callback も来ないため、
                // ここで直接 code 2 を通知する。
                filePickRequest.take();
                nativeOnFilePickerResult(requestId, 2, null, -1);
            }
        });
    }

    // SAF の DISPLAY_NAME を解決する。プロバイダの `query` 自体が例外を送出しても `getLastPathSegment()` の fallback へ続ける
    // （内容は不明なため名前解決の失敗として扱うのみで、まだ内部エラー確定はしない）。ファイル名・Uri はログへ出力しない
    // （security.md A09）。ドラッグ & ドロップ（`DragDropHelper`）は機能 crate ごとに独立して同じ規則を持つ。
    private String resolveDisplayName(Uri uri) {
        String name = null;
        try (Cursor cursor =
                activity.getContentResolver().query(uri, new String[] {OpenableColumns.DISPLAY_NAME},
                        null, null, null)) {
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

    // `pickDocument` が選択を受け取った後、バックグラウンドスレッドで名前解決と fd 取得を行う（Issue #874、案 B、
    // 「Android: fd を開き直さない」）。内容の読み取り・8 MiB 判定は Rust 側 `read_picked_from` に一本化するため、ここでは
    // `detachFd()` で所有権を渡した生 fd を渡すのみで読み取りループを持たない。`/proc/self/fd/N` の再 open は pipe 型プロバイダで
    // 権限再確認に失敗するため使わない（fd をそのまま借用させる）。
    //
    // 選択された Uri・ファイル名・例外メッセージはログへ出力しない（他アプリの機密文書になり得る、security.md A09）。
    private void deliverPickedDocument(long requestId, Uri uri) {
        String name = resolveDisplayName(uri);
        if (name == null || name.isEmpty()) {
            nativeOnFilePickerResult(requestId, 2, null, -1);
            return;
        }

        // `detachFd()` は fd の所有権を呼び出し元（ここでは Rust 側）へ移す。移した後はこの `ParcelFileDescriptor` の `close()`
        // （try-with-resources の暗黙の呼び出しを含む）は no-op になる（Android の契約）ため、二重 close にはならない。
        // `detachFd()` 自体が例外を送出した場合（fd 未取得・既に detach 済み等）はまだ所有権を渡していないため、
        // try-with-resources が通常どおり `pfd` を閉じる。
        try (ParcelFileDescriptor pfd = activity.getContentResolver().openFileDescriptor(uri, "r")) {
            if (pfd == null) {
                nativeOnFilePickerResult(requestId, 2, null, -1);
                return;
            }
            int fd = pfd.detachFd();
            nativeOnFilePickerResult(requestId, 0, name, fd);
        } catch (IOException | RuntimeException e) {
            nativeOnFilePickerResult(requestId, 2, null, -1);
        }
    }
}
