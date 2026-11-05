package com.fandhe.platform;

import android.Manifest;
import android.content.ActivityNotFoundException;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.os.Bundle;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;

// カメラ撮影（camera-capture、Issue #750 / #1170）の Android 実装ヘルパー。MainActivity から生成・委譲される。
// 機能 crate `fandhe-cap-camera-capture` が持つ Java で、Rust の `src/android.rs`（`AndroidCamera::capture`）が
// `MainActivity.requestCameraCapture(long)` を JNI で呼び、結果を本クラスの `nativeOnCameraResult`
// （Rust の `Java_com_fandhe_platform_CameraCaptureHelper_nativeOnCameraResult`）で受ける。
//
// MainActivity に残るのは生成と委譲だけ（`request`・`onCreate`・`onSaveInstanceState`）。
// `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity のフィールド初期化子から
// 生成してコンストラクタで登録する（登録キーは生成順の連番で決まるため、生成位置を動かさない）。
// 共有の `RequestTracker`（app 側）を同じパッケージから使う。単体での自立は #1183 / #1176 の範囲。
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

    CameraCaptureHelper(ComponentActivity activity) {
        this.activity = activity;
        this.requestCameraPermission = activity.registerForActivityResult(
                new ActivityResultContracts.RequestPermission(), granted -> {
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
        this.takePicturePreview = activity.registerForActivityResult(
                new ActivityResultContracts.TakePicturePreview(), bitmap -> {
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

    // Activity 再生成後の状態復元（MainActivity.onCreate から委譲）。`awaitingCameraResult` を false へ
    // 戻して新要求を即座に受け付け可能にしてはならない（クラス冒頭コメント参照）。中断された要求が
    // あれば復元直後に status 7 で即座に決着させつつ、`awaitingCameraResult` は true のまま維持して
    // 新要求を拒否し続ける。解除は各コールバックが旧結果を実際に消費した時点でのみ行う。
    // `pendingCaptureInterrupted` 自体も復元する。未保存だと、旧コールバック未消費のまま連続で再生成
    // された場合に中断状態が失われ、(1) 同じ要求へ status 7 を多重通知し、(2) awaitingCameraResult の
    // ブロックが意図せず解除される。既に中断決着済みなら今回の再生成では再通知しない。
    void onCreate(Bundle savedInstanceState) {
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
    void onSaveInstanceState(Bundle outState) {
        outState.putLong(STATE_PENDING_CAPTURE_REQUEST_ID, pendingCaptureRequest.current());
        outState.putBoolean(STATE_AWAITING_CAMERA_RESULT, awaitingCameraResult);
        outState.putBoolean(STATE_PENDING_CAPTURE_INTERRUPTED, pendingCaptureInterrupted);
    }

    // Rust 側の worker スレッド（`AndroidCamera::capture`）から `MainActivity.requestCameraCapture` 経由で
    // 呼ばれる。UI 操作を伴うため `runOnUiThread` へ委譲する。`requestId` は先に `pendingCaptureRequest`
    // へ保持してから `nativeOnCameraResult` へ返す（`runOnUiThread` はワーカースレッドの呼び出しと
    // 非同期のため、フィールドへの書き込みをコールバック登録より先に済ませておく）。
    void request(long requestId) {
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
