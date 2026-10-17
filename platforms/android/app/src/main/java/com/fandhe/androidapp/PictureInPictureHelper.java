package com.fandhe.androidapp;

import android.app.Activity;
import android.app.PictureInPictureParams;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Build;
import android.util.Rational;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

// ピクチャ・イン・ピクチャ（Issue #366、親 #97）の Android 実装ヘルパー。
// MainActivity から生成・委譲され、Rust の `device/android_picture_in_picture.rs` が egui スレッドから
// `MainActivity.pipPrepare` / `pipStart` / `pipStop` / `pipSnapshot` / `pipShutdown` を同期 JNI で呼ぶ
// pull 型（Rust 側ドライバが snapshot をポーリングするので native コールバックは無い）。
//
// コード表の SSOT はここで、Rust 側が同じ値で写像する。
//   戻り: 0 OK / 1 FAILED / 2 UNSUPPORTED / 3 NOT_POSSIBLE / 4 BAD_GEN
//   phase: 0 Idle / 1 Ready / 2 Starting / 3 Active / 4 Stopping / 5 Exited
//          / 6 Failed(NotPossible) / 7 Failed(Internal)
//
// Activity ごと小窓になる。winit は onPause で描画を止めるので、小窓に入ると復帰まで Rust 側の
// drive() が走らない。そのため enterPictureInPictureMode が true を返した時点で Active にし、
// ドライバが start 直後の同フレームで読めるようにする（しないと復帰後の最初のフレームで期限切れ判定が
// 先に走り、正常終了が失敗になる）。true は OS が「小窓に入った」と報告した値で、嘘の成功ではない。
// onPause は中断として扱わない（小窓に入るときにも onPause が来る）。
//
// 状態は `lock` 1 本で守る。UI スレッド待ち（runOnUiThread + latch）は lock の外で行い、
// UI 側は実行時に destroyed・世代・phase を再確認する（開始と取り消しの排他）。
// 守らない範囲: 小窓の位置・大きさ、小窓中の画面静止（winit の制約）、複数プロセス。
final class PictureInPictureHelper {
    static final int RESULT_OK = 0;
    static final int RESULT_FAILED = 1;
    static final int RESULT_UNSUPPORTED = 2;
    static final int RESULT_NOT_POSSIBLE = 3;
    static final int RESULT_BAD_GEN = 4;

    static final int PHASE_IDLE = 0;
    static final int PHASE_READY = 1;
    static final int PHASE_STARTING = 2;
    static final int PHASE_ACTIVE = 3;
    static final int PHASE_STOPPING = 4;
    static final int PHASE_EXITED = 5;
    static final int PHASE_FAILED_NOT_POSSIBLE = 6;
    static final int PHASE_FAILED_INTERNAL = 7;

    private static final long UI_TIMEOUT_MS = 1000;
    private static final int PENDING = 0;
    private static final int STARTED = 1;
    private static final int ABANDONED = 2;

    private static final class Call {
        final AtomicInteger state = new AtomicInteger(PENDING);
        final CountDownLatch finished = new CountDownLatch(1);
        volatile int code = RESULT_FAILED;
    }

    private final Activity activity;
    private final Object lock = new Object();
    private boolean destroyed;
    private long current;
    private int phase = PHASE_IDLE;
    private PictureInPictureParams params;

    PictureInPictureHelper(Activity activity) {
        this.activity = activity;
    }

    // egui スレッドから。UI スレッド不要。
    int prepare(long gen, int w, int h) {
        synchronized (lock) {
            if (destroyed) {
                return RESULT_FAILED;
            }
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O
                    || !activity.getPackageManager()
                            .hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE)) {
                return RESULT_UNSUPPORTED;
            }
            if (w <= 0 || h <= 0) {
                return RESULT_FAILED;
            }
            try {
                params = new PictureInPictureParams.Builder()
                        .setAspectRatio(new Rational(w, h))
                        .build();
            } catch (RuntimeException e) {
                return RESULT_FAILED;
            }
            current = gen;
            phase = PHASE_READY;
            return RESULT_OK;
        }
    }

    int start(final long gen) {
        synchronized (lock) {
            if (destroyed) {
                return RESULT_FAILED;
            }
            if (gen != current) {
                return RESULT_BAD_GEN;
            }
            if (phase != PHASE_READY && phase != PHASE_EXITED) {
                return RESULT_FAILED;
            }
            phase = PHASE_STARTING;
        }
        final Call call = new Call();
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (call.state.compareAndSet(PENDING, STARTED)) {
                    try {
                        call.code = enter(gen);
                    } finally {
                        call.finished.countDown();
                    }
                }
            }
        });
        return await(call, gen);
    }

    // UI スレッド。実行時に要求がまだ現在のものか確認してから入る。
    private int enter(long gen) {
        synchronized (lock) {
            if (destroyed) {
                return RESULT_FAILED;
            }
            if (gen != current) {
                return RESULT_BAD_GEN;
            }
            if (phase != PHASE_STARTING) {
                // 取り消し済み（stop が Exited を確定した）。OS の状態は変えない。
                return RESULT_OK;
            }
            try {
                if (activity.enterPictureInPictureMode(params)) {
                    phase = PHASE_ACTIVE;
                    return RESULT_OK;
                }
                phase = PHASE_FAILED_NOT_POSSIBLE;
                return RESULT_NOT_POSSIBLE;
            } catch (RuntimeException e) {
                phase = PHASE_FAILED_INTERNAL;
                return RESULT_FAILED;
            }
        }
    }

    int stop(final long gen) {
        synchronized (lock) {
            if (gen != current) {
                return RESULT_BAD_GEN;
            }
            if (phase == PHASE_STARTING) {
                phase = PHASE_EXITED; // UI 側の再確認で enter がスキップされる
                return RESULT_OK;
            }
            if (phase != PHASE_ACTIVE) {
                return RESULT_OK;
            }
            phase = PHASE_STOPPING;
        }
        final Call call = new Call();
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (call.state.compareAndSet(PENDING, STARTED)) {
                    try {
                        call.code = leave(gen);
                    } finally {
                        call.finished.countDown();
                    }
                }
            }
        });
        return await(call, gen);
    }

    // UI スレッド。singleTask なので自身を前面に戻すと小窓が解ける。
    // 届く onNewIntent は ACTION_VIEW でないため URL 中継は無視する。
    private int leave(long gen) {
        synchronized (lock) {
            if (destroyed || gen != current || phase != PHASE_STOPPING) {
                return RESULT_OK;
            }
            try {
                if (activity.isInPictureInPictureMode()) {
                    activity.startActivity(new Intent(activity, MainActivity.class)
                            .addFlags(Intent.FLAG_ACTIVITY_REORDER_TO_FRONT));
                } else {
                    phase = PHASE_EXITED;
                }
                return RESULT_OK;
            } catch (RuntimeException e) {
                phase = PHASE_FAILED_INTERNAL;
                return RESULT_FAILED;
            }
        }
    }

    // 1 秒以内に UI 側が始まらなければ放棄。開始済みなら完了まで待って実結果を返す。
    private int await(Call call, long gen) {
        boolean interrupted = false;
        try {
            try {
                if (call.finished.await(UI_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
                    return call.code;
                }
            } catch (InterruptedException e) {
                interrupted = true;
            }
            if (call.state.compareAndSet(PENDING, ABANDONED)) {
                synchronized (lock) {
                    if (gen == current && (phase == PHASE_STARTING || phase == PHASE_STOPPING)) {
                        phase = PHASE_FAILED_INTERNAL;
                    }
                }
                return RESULT_FAILED;
            }
            while (true) {
                try {
                    call.finished.await();
                    return call.code;
                } catch (InterruptedException e) {
                    interrupted = true;
                }
            }
        } finally {
            if (interrupted) {
                Thread.currentThread().interrupt();
            }
        }
    }

    long[] snapshot() {
        synchronized (lock) {
            if (current == 0) {
                return new long[] {0, PHASE_IDLE};
            }
            return new long[] {current, phase};
        }
    }

    // 冪等。小窓そのものは閉じない（OS が片付ける。閉じるのはドライバの stop）。
    void shutdown() {
        synchronized (lock) {
            current = 0;
            phase = PHASE_IDLE;
            params = null;
        }
    }

    // MainActivity.onPictureInPictureModeChanged（UI スレッド）から。
    void onPictureInPictureModeChanged(boolean inPip) {
        synchronized (lock) {
            if (destroyed || current == 0) {
                return;
            }
            if (inPip) {
                if (phase == PHASE_STARTING || phase == PHASE_ACTIVE) {
                    phase = PHASE_ACTIVE;
                }
            } else if (phase == PHASE_STARTING || phase == PHASE_ACTIVE || phase == PHASE_STOPPING) {
                // 利用者が小窓を閉じた・戻したも成功の遷移。
                phase = PHASE_EXITED;
            }
        }
    }

    void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            current = 0;
            phase = PHASE_IDLE;
            params = null;
        }
    }
}
