package com.fandhe.androidapp;

import android.content.pm.PackageManager;
import android.health.connect.AggregateRecordsRequest;
import android.health.connect.AggregateRecordsResponse;
import android.health.connect.HealthConnectException;
import android.health.connect.HealthConnectManager;
import android.health.connect.TimeInstantRangeFilter;
import android.health.connect.datatypes.StepsRecord;
import android.os.Build;
import android.os.OutcomeReceiver;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;

import java.time.Instant;
import java.time.LocalDate;
import java.time.ZoneId;

// ヘルスデータ（今日の歩数）画面（Issue #1100、親 #149）の Android 実装ヘルパー。
// MainActivity から生成・委譲される（Rust の `device/android_health_data.rs` が
// `MainActivity.requestHealthDataRead` / `cancelHealthDataRead` を JNI で呼び、結果を
// `nativeOnHealthDataResult` で受ける）。契約の SSOT は `fandhe_device::health_data`。
//
// 1 回の要求 = 許可の確認（未許可なら Health Connect の許可画面）→ `HealthConnectManager.aggregate`
// （`StepsRecord.STEPS_COUNT_TOTAL`、今日 0 時〜現在）。`aggregate` に取り消し API は無いため、
// 期限切れ・差し替え後に届いた結果は `RequestTracker.complete(id)` の照合で捨てる。
// 解放する資源は無い。開始・許可結果・集計の callback はすべて UI スレッドで直列化する。
//
// ライフサイクル: `onPause` / `onStop`（許可画面自体が起こす）では中断しない。`onDestroy`
// （Activity 再生成を含む）は保留中の要求を FAILED で確定する。`request` はインスタンス
// フィールドなので、再生成後の新インスタンスは NONE から始まり旧結果を拾わない。
// 歩数・例外メッセージは logcat に出さない。
final class HealthDataHelper {
    // 結果コード。SSOT は Rust 側 `fandhe_device::health_data::decode_code`（同じ番号）。
    // 3（期限切れ）は Rust 側だけが作り、4（取り消し）は iOS 専用のため Android は返さない。
    static final int RESULT_STEPS = 0;
    static final int RESULT_NO_VISIBLE_DATA = 1;
    static final int RESULT_UNSUPPORTED = 2;
    static final int RESULT_FAILED = 5;

    private static final String READ_STEPS = "android.permission.health.READ_STEPS";

    private static native void nativeOnHealthDataResult(long requestId, int code, long steps);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    // 取り消し済みの最大 id。cancel が start より先に走っても古い要求を開始しない。
    private volatile long cancelledId = RequestTracker.NONE;
    // 許可画面を起動した要求 id（UI スレッド専用）。NONE なら画面は出ていない。
    private long permissionId = RequestTracker.NONE;
    // UI スレッドで最後に受理した id。これ以下の開始要求は追い越された古い要求として捨てる。
    private long latestId = RequestTracker.NONE;

    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity の
    // フィールド初期化子から本クラスを生成してここで登録する。
    private final ActivityResultLauncher<String> permissionLauncher;

    HealthDataHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.RequestPermission(), this::onPermissionResult);
    }

    // JNI スレッドから呼ばれる。
    void read(long id) {
        activity.runOnUiThread(() -> start(id));
    }

    // Rust 側の期限切れから呼ばれる。資源は無いので追跡 id を消すだけ（以後の結果は捨てられる）。
    void cancel(long id) {
        cancelledId = Math.max(cancelledId, id);
        request.complete(id);
    }

    // MainActivity.onDestroy から UI スレッドで呼ぶ。
    void onDestroy() {
        long id = request.take();
        if (id != RequestTracker.NONE) {
            nativeOnHealthDataResult(id, RESULT_FAILED, 0);
        }
    }

    private boolean destroyed() {
        return activity.isFinishing() || activity.isDestroyed();
    }

    private void start(long id) {
        if (id <= latestId) {
            return;
        }
        latestId = id;
        if (id <= cancelledId) {
            return;
        }
        // 保留中の再要求は Failed（既存の要求には触れない）。
        if (request.current() != RequestTracker.NONE || destroyed()) {
            nativeOnHealthDataResult(id, RESULT_FAILED, 0);
            return;
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE
                || activity.getSystemService(HealthConnectManager.class) == null) {
            nativeOnHealthDataResult(id, RESULT_UNSUPPORTED, 0);
            return;
        }
        request.track(id);
        checkPermissionAndRead(id);
    }

    private void checkPermissionAndRead(long id) {
        if (activity.checkSelfPermission(READ_STEPS) == PackageManager.PERMISSION_GRANTED) {
            aggregate(id);
        } else if (permissionId == RequestTracker.NONE) {
            // 画面は同時に 1 つだけ。出ている間に差し替わった要求は結果到着時に再判定する。
            permissionId = id;
            permissionLauncher.launch(READ_STEPS);
        }
    }

    // 許可画面自体が Activity を pause させるため、resumed では分岐しない。
    private void onPermissionResult(boolean granted) {
        long launched = permissionId;
        permissionId = RequestTracker.NONE;
        long current = request.current();
        if (current == RequestTracker.NONE) {
            return; // 期限切れ・破棄済み・再生成後のインスタンス。
        }
        if (launched != current) {
            checkPermissionAndRead(current); // 起動時の要求は差し替え済み。現在の要求で再判定。
        } else if (!granted) {
            finish(current, RESULT_NO_VISIBLE_DATA, 0);
        } else {
            aggregate(current);
        }
    }

    private void aggregate(long id) {
        try {
            HealthConnectManager manager = activity.getSystemService(HealthConnectManager.class);
            ZoneId zone = ZoneId.systemDefault();
            TimeInstantRangeFilter filter = new TimeInstantRangeFilter.Builder()
                    .setStartTime(LocalDate.now(zone).atStartOfDay(zone).toInstant())
                    .setEndTime(Instant.now())
                    .build();
            AggregateRecordsRequest<Long> req = new AggregateRecordsRequest.Builder<Long>(filter)
                    .addAggregationType(StepsRecord.STEPS_COUNT_TOTAL)
                    .build();
            manager.aggregate(req, activity.getMainExecutor(),
                    new OutcomeReceiver<AggregateRecordsResponse<Long>, HealthConnectException>() {
                        @Override
                        public void onResult(AggregateRecordsResponse<Long> response) {
                            Long total = response.get(StepsRecord.STEPS_COUNT_TOTAL);
                            if (total == null) {
                                finish(id, RESULT_NO_VISIBLE_DATA, 0);
                            } else {
                                finish(id, RESULT_STEPS, total);
                            }
                        }

                        @Override
                        public void onError(HealthConnectException e) {
                            finish(id, e.getErrorCode() == HealthConnectException.ERROR_SECURITY
                                    ? RESULT_NO_VISIBLE_DATA : RESULT_FAILED, 0);
                        }
                    });
        } catch (SecurityException e) {
            finish(id, RESULT_NO_VISIBLE_DATA, 0);
        } catch (RuntimeException e) {
            finish(id, RESULT_FAILED, 0);
        }
    }

    // compare-and-set で確定する。旧要求の遅れた callback が新しい要求の id を消費しない。
    private void finish(long id, int code, long steps) {
        if (request.complete(id)) {
            nativeOnHealthDataResult(id, code, steps);
        }
    }
}
