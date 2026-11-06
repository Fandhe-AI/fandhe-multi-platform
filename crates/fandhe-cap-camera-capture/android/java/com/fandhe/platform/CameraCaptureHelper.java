package com.fandhe.platform;

import android.Manifest;
import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.os.Bundle;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// カメラ撮影（camera-capture、Issue #750 / #1170 / #1257）の Android 実装ヘルパー。
// 機能 crate `fandhe-cap-camera-capture` が持つ Java で、利用者の `MainActivity` には何も書かせない。
// Rust の `src/android.rs`（`AndroidCamera::capture`）が static の `request(Activity, long)` を JNI で呼び、
// 結果を本クラスの `nativeOnCameraResult`（Rust の `Java_com_fandhe_platform_CameraCaptureHelper_nativeOnCameraResult`）で受ける。
//
// Activity への参加は `CameraCaptureLifecycle`（非公開の ContentProvider、`FeatureLifecycleProvider` 継承）が行う。
// Activity の生成時に `attach`（ランチャー登録と再生成後の復元）、状態保存時に `save`、破棄時に `detach` を呼ぶ。
// `registerForActivityResult` は STARTED より前に呼ぶ契約なので `attach` は `Activity.onCreate` 内で呼ばれる。
// 登録キーは明示する（連番キーだと生成順への依存が残るため）。
// 共有の `RequestTracker` は共通層 `fandhe-platform-android` の Java を同じパッケージから使う。
//
// Activity ごとの helper は `HELPERS` に持つ。Rust のワーカースレッド（`request`）と UI スレッド
// （ライフサイクル）の両方から触るため同期化し、破棄時に必ず外す（Activity を保持し続けない）。
// 表に無い Activity への要求は受け付けず false を返す（Rust が成功へ畳まず失敗として返す）。
//
// status（Rust 側 `decode_capture_result` と対）:
//   0=成功／1=権限拒否／2=カメラアプリなし／3=キャンセル／4=旧要求が未完了のため拒否／
//   5=画像サイズが上限超過／6=launch() 自体が想定外の実行時例外で失敗／7=Activity 再生成により中断
//
// ライフサイクル: Activity 再生成後の新インスタンスにも、旧インスタンスで保留中だった権限ダイアログ・
// カメラアプリの結果が同一キーで配送され得る（androidx ActivityResultRegistry の契約）。このため復元時に
// 保留中の要求を status 7 で先に決着させ（Rust 側 300 秒タイムアウトを待たせない）、旧結果は
// `pendingCaptureInterrupted` で消費・破棄する。その間 `awaitingCameraResult` は true のまま保ち、
// 新要求を status 4 で拒否する（旧要求の遅延結果を新要求の ID で誤配送しないため）。
//
// 画素・例外メッセージはログに出さない。
final class CameraCaptureHelper {
    // `nativeOnCameraResult` の JByteArray 確保前検査（Rust 側 `MAX_IMAGE_BYTES` と同値、64 MiB）を
    // Java 側の配列確保前にも適用する。大寸法 Bitmap で Rust 検査前に Java 側だけで追加配列を確保して
    // OutOfMemoryError になるのを防ぐ。
    private static final long MAX_IMAGE_BYTES = 64L * 1024 * 1024;

    // Activity 再生成後も撮影要求の状態を復元するための Bundle キー。
    private static final String STATE_PENDING_CAPTURE_REQUEST_ID = "pendingCaptureRequestId";
    private static final String STATE_AWAITING_CAMERA_RESULT = "awaitingCameraResult";
    private static final String STATE_PENDING_CAPTURE_INTERRUPTED = "pendingCaptureInterrupted";

    // Rust 側（`src/android.rs`）の JNI エクスポート関数。
    // requestId: `request(long)` が受け取った値をそのまま返す（Rust 側 mailbox が要求 ID の一致を検査し、
    // タイムアウト後に届いた古い要求のコールバックを新しい要求の結果と取り違えないため）。
    private static native void nativeOnCameraResult(
            long requestId, byte[] rgba, int width, int height, int status);

    // Activity ごとの helper（上記クラス冒頭コメント参照）。
    private static final Map<Activity, CameraCaptureHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private static final String KEY_PERMISSION = "fandhe.camera-capture.permission";
    private static final String KEY_TAKE_PICTURE = "fandhe.camera-capture.take-picture";

    private final ComponentActivity activity;

    // 進行中の撮影要求の ID。GameActivity は 1 Activity インスタンスにつき 1 撮影のみ許容する契約
    // （Rust 側 `CaptureMailbox` の単一飛行）のため、フィールド 1 個で足りる。再生成で新インスタンスの
    // 初期値（`RequestTracker.NONE`）に戻ると、復元後に届く ActivityResult が誤った ID で配送されるため
    // Bundle で保存・復元する。id の保持・照合そのものは `RequestTracker` へ委譲する。
    private final RequestTracker pendingCaptureRequest = new RequestTracker();

    // true の間は Android 側のコールバック（権限ダイアログ・カメラアプリ）待ちで、旧要求の結果が
    // 未回収であることを示す。Rust 側 `CaptureMailbox` はタイムアウトで Idle に戻り新要求 B の `begin()`
    // を許可し得るが、Java 側でまだ旧要求 A の ActivityResult を待っている間に `request(B)` が呼ばれると、
    // 後から届く A の結果が `pendingCaptureRequest`（B に上書き済み）で配送され、B の結果として誤って
    // 受理される。このフラグが true の間は新要求を即座に拒否することで防ぐ。
    // `pendingCaptureInterrupted` が true の間もこのフラグは true のまま維持する（旧コールバックが
    // 実際に届いて消費されるまで新要求を拒否し続ける）。Rust 側 worker スレッドと UI スレッドの
    // 双方から読み書きされるため volatile にする。
    private volatile boolean awaitingCameraResult;

    // Activity 再生成で status 7 として中断決着させた「旧」要求に対し、後から ActivityResultRegistry が
    // 再配送してくる許可ダイアログ・カメラアプリの結果を無視して破棄するためのフラグ。コールバック側で
    // 最初にこれを検査して消費（クリアして return）することで、既に status 7 で決着済みの要求について
    // 二重の nativeOnCameraResult 配送・意図しないカメラアプリ再起動・新しい要求 ID への誤配送を防ぐ。
    // 「新要求開始時」に早期クリアしない（旧要求 A の実コールバックが未着のまま新要求 B が始まると
    // A の遅延結果が B の ID で誤配送されるため）。連続再生成でも中断状態を失わないよう Bundle へ保存・復元する。
    // 読み書きは UI スレッドからのみ行うため追加の同期は不要。
    private boolean pendingCaptureInterrupted;

    // CAMERA 権限のランタイム要求と、標準カメラアプリ（ACTION_IMAGE_CAPTURE）のサムネイル起動。
    private final ActivityResultLauncher<String> requestCameraPermission;
    private final ActivityResultLauncher<Void> takePicturePreview;

    private CameraCaptureHelper(ComponentActivity activity) {
        this.activity = activity;
        this.requestCameraPermission = activity.getActivityResultRegistry().register(
                KEY_PERMISSION, activity, new ActivityResultContracts.RequestPermission(), granted -> {
                    if (pendingCaptureInterrupted) {
                        // 再生成前に status 7 で決着済みの旧要求への遅延配送。許可の可否に関わらずここで
                        // 消費し、以後の分岐へ進めない。ここで初めて awaitingCameraResult を解除し、
                        // 新要求を受け付け可能にする。
                        pendingCaptureInterrupted = false;
                        awaitingCameraResult = false;
                        return;
                    }
                    if (granted) {
                        launchCameraCapture();
                    } else {
                        // `pendingCaptureRequest` は awaitingCameraResult を解除する前にローカル変数へ
                        // 退避する。解除後は worker スレッドの次要求が `request` でこのフィールドを
                        // 上書きし得るため、フィールドを直接 `nativeOnCameraResult` へ渡すと別要求の
                        // ID を誤って配送する競合が生じる。
                        long resultRequestId = pendingCaptureRequest.current();
                        awaitingCameraResult = false;
                        nativeOnCameraResult(resultRequestId, null, 0, 0, 1);
                    }
                });
        this.takePicturePreview = activity.getActivityResultRegistry().register(
                KEY_TAKE_PICTURE, activity, new ActivityResultContracts.TakePicturePreview(), bitmap -> {
                    if (pendingCaptureInterrupted) {
                        // 許可コールバック側と同じ理由。
                        pendingCaptureInterrupted = false;
                        awaitingCameraResult = false;
                        return;
                    }
                    if (bitmap == null) {
                        // ユーザーがカメラアプリを戻る操作でキャンセルした場合。
                        // ID 退避の理由は許可コールバック側コメント参照。
                        long resultRequestId = pendingCaptureRequest.current();
                        awaitingCameraResult = false;
                        nativeOnCameraResult(resultRequestId, null, 0, 0, 3);
                        return;
                    }
                    deliverBitmap(bitmap);
                });
    }

    // Activity の生成時（`CameraCaptureLifecycle` から、`Activity.onCreate` 内で呼ばれる）。ランチャーを登録し、
    // 再生成後なら保存した要求の状態を復元する。
    static void attach(ComponentActivity activity, Bundle savedInstanceState) {
        CameraCaptureHelper helper = new CameraCaptureHelper(activity);
        HELPERS.put(activity, helper);
        helper.onCreate(savedInstanceState);
    }

    // Activity の状態保存時（`CameraCaptureLifecycle` から）。
    static void save(Activity activity, Bundle outState) {
        CameraCaptureHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.onSaveInstanceState(outState);
        }
    }

    // Activity の破棄時（`CameraCaptureLifecycle` から）。表から外して Activity を手放す。
    static void detach(Activity activity) {
        HELPERS.remove(activity);
    }

    // Rust 側の worker スレッド（`AndroidCamera::capture`）から JNI で呼ばれる入口。`activity` は Rust が
    // 保持する現在の Activity。helper が無い（生成前・破棄後・対象外の Activity）なら false を返し、
    // Rust が失敗として返す（`nativeOnCameraResult` は呼ばれない）。
    static boolean request(Activity activity, long requestId) {
        CameraCaptureHelper helper = HELPERS.get(activity);
        if (helper == null) {
            return false;
        }
        helper.begin(requestId);
        return true;
    }

    // Activity 再生成後の状態復元（`attach` から）。`awaitingCameraResult` を false へ
    // 戻して新要求を即座に受け付け可能にしてはならない（クラス冒頭コメント参照）。中断された要求が
    // あれば復元直後に status 7 で即座に決着させつつ、`awaitingCameraResult` は true のまま維持して
    // 新要求を拒否し続ける。解除は各コールバックが旧結果を実際に消費した時点でのみ行う。
    // `pendingCaptureInterrupted` 自体も復元する。未保存だと、旧コールバック未消費のまま連続で再生成
    // された場合に中断状態が失われ、(1) 同じ要求へ status 7 を多重通知し、(2) awaitingCameraResult の
    // ブロックが意図せず解除される。既に中断決着済みなら今回の再生成では再通知しない。
    private void onCreate(Bundle savedInstanceState) {
        if (savedInstanceState == null) {
            return;
        }
        pendingCaptureRequest.track(savedInstanceState.getLong(
                STATE_PENDING_CAPTURE_REQUEST_ID, RequestTracker.NONE));
        awaitingCameraResult = savedInstanceState.getBoolean(STATE_AWAITING_CAMERA_RESULT);
        pendingCaptureInterrupted =
                savedInstanceState.getBoolean(STATE_PENDING_CAPTURE_INTERRUPTED);
        if (awaitingCameraResult && !pendingCaptureInterrupted) {
            pendingCaptureInterrupted = true;
            nativeOnCameraResult(pendingCaptureRequest.current(), null, 0, 0, 7);
        }
    }

    // Activity 再生成前に撮影要求の状態を保存する（`onCreate` の復元と対）。
    private void onSaveInstanceState(Bundle outState) {
        outState.putLong(STATE_PENDING_CAPTURE_REQUEST_ID, pendingCaptureRequest.current());
        outState.putBoolean(STATE_AWAITING_CAMERA_RESULT, awaitingCameraResult);
        outState.putBoolean(STATE_PENDING_CAPTURE_INTERRUPTED, pendingCaptureInterrupted);
    }

    // `request` から呼ばれる。UI 操作を伴うため `runOnUiThread` へ委譲する。`requestId` は先に `pendingCaptureRequest`
    // へ保持してから `nativeOnCameraResult` へ返す（`runOnUiThread` はワーカースレッドの呼び出しと
    // 非同期のため、フィールドへの書き込みをコールバック登録より先に済ませておく）。
    private void begin(long requestId) {
        if (awaitingCameraResult) {
            // 旧要求の Android コールバック（権限ダイアログ・カメラアプリ）が未回収。
            // `pendingCaptureRequest` を上書きすると旧要求の遅延結果がこの新要求の ID で誤って
            // 配送されるため、ここで即座に拒否する（UI 操作を伴わないので `runOnUiThread` を経由しない）。
            nativeOnCameraResult(requestId, null, 0, 0, 4);
            return;
        }
        // この時点で `awaitingCameraResult` は false であり、それは `pendingCaptureInterrupted` も
        // false であることを含意する（`pendingCaptureInterrupted` は常に `awaitingCameraResult` の
        // 部分集合として維持され、解除は必ず両方同時に行う）。よってここで触る必要はない。
        pendingCaptureRequest.track(requestId);
        awaitingCameraResult = true;
        activity.runOnUiThread(() -> {
            try {
                if (ContextCompat.checkSelfPermission(activity, Manifest.permission.CAMERA)
                        == PackageManager.PERMISSION_GRANTED) {
                    launchCameraCapture();
                } else {
                    requestCameraPermission.launch(Manifest.permission.CAMERA);
                }
            } catch (RuntimeException e) {
                // `launch()` 自体が例外を送出した場合（onSaveInstanceState 後の呼び出し等、
                // ActivityResultRegistry の状態不整合）に awaitingCameraResult が true のまま
                // 恒久的に固着し、以後すべての要求が status 4 で拒否され続けるのを防ぐ。
                long resultRequestId = pendingCaptureRequest.current();
                awaitingCameraResult = false;
                nativeOnCameraResult(resultRequestId, null, 0, 0, 6);
            }
        });
    }

    private void launchCameraCapture() {
        try {
            takePicturePreview.launch(null);
        } catch (ActivityNotFoundException e) {
            // 標準カメラアプリが端末に存在しない（エミュレータ等）。
            long resultRequestId = pendingCaptureRequest.current();
            awaitingCameraResult = false;
            nativeOnCameraResult(resultRequestId, null, 0, 0, 2);
        } catch (RuntimeException e) {
            // 許可コールバック経由でここへ来た場合は `request` の runOnUiThread の try/catch を経由しない
            // ため、ここでも launch() 失敗によるフラグ固着を防ぐ必要がある。
            long resultRequestId = pendingCaptureRequest.current();
            awaitingCameraResult = false;
            nativeOnCameraResult(resultRequestId, null, 0, 0, 6);
        }
    }

    // `Bitmap.getPixels` の ARGB_8888 int[] を RGBA byte[]（Rust の `Image` と同じ行優先・パディングなし
    // レイアウト）へ詰め替えて native 側へ渡す。
    private void deliverBitmap(Bitmap bitmap) {
        // ID 退避の理由は許可コールバック側コメント参照。
        long resultRequestId = pendingCaptureRequest.current();
        awaitingCameraResult = false;
        int width = bitmap.getWidth();
        int height = bitmap.getHeight();

        // 配列確保前にサイズを検証する。`long` で計算し、`int` オーバーフロー（`width * height` や `* 4`）
        // を経由せず Rust 側と同じ上限（64 MiB）を先に確認する。外部カメラアプリが返す Bitmap の寸法は
        // 検証していないため、大寸法（例: 4097x4096）だと Rust 側の検査に届く前に Java 側だけで
        // 128 MiB 超の int[]/byte[] を確保してしまい、ヒープの小さい端末で OutOfMemoryError になり得る。
        long pixelCount = (long) width * (long) height;
        long byteCount = pixelCount * 4;
        if (byteCount > MAX_IMAGE_BYTES) {
            nativeOnCameraResult(resultRequestId, null, width, height, 5);
            return;
        }

        int[] argb = new int[width * height];
        bitmap.getPixels(argb, 0, width, 0, 0, width, height);

        byte[] rgba = new byte[width * height * 4];
        for (int i = 0; i < argb.length; i++) {
            int pixel = argb[i];
            int offset = i * 4;
            rgba[offset] = (byte) ((pixel >> 16) & 0xFF); // R
            rgba[offset + 1] = (byte) ((pixel >> 8) & 0xFF); // G
            rgba[offset + 2] = (byte) (pixel & 0xFF); // B
            rgba[offset + 3] = (byte) ((pixel >> 24) & 0xFF); // A
        }
        nativeOnCameraResult(resultRequestId, rgba, width, height, 0);
    }
}
