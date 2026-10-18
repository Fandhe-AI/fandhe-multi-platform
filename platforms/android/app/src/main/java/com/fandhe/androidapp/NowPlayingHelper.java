package com.fandhe.androidapp;

import android.app.Activity;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.media.MediaMetadata;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

// ロック画面・OS のメディア操作（Issue #383、親 #98）の Android 実装ヘルパー。MainActivity から
// 生成・委譲される。Rust の `device/android_now_playing.rs` が egui スレッドから
// `MainActivity.nowPlayingPublish` / `nowPlayingClear` を JNI で同期呼び出しし、OS からの
// 再生・一時停止は `nativeOnNowPlayingCommand` で Rust の `SharedOsCommand` へ返す。
//
// コード表（SSOT はここ。Rust 側 `decode_result` / `decode_command` が同じ値で写像する）:
//   publish の戻り: 0 OK / 1 FAILED / 2 TIMEOUT
//   操作: 0 PLAY / 1 PAUSE（メディアキーのトグルは MediaSession の既定処理が PlaybackState を見て
//   PLAY / PAUSE に振り分けるため、Toggle のコードは持たない）
//
// スレッド: MediaSession・通知は UI スレッドで扱う。publish は `ScreenWakeLockHelper.set` と同じく
// `runOnUiThread` へ渡して最長 APPLY_TIMEOUT_MS だけ待つ。未開始のまま期限を過ぎたら放棄して
// TIMEOUT（UI 側は何も変えない）、開始済みなら完了を待って実結果を返す。状態は `lock` 1 本の内側。
//
// ライフサイクル: 前景だけで扱う。onPause で session を非アクティブにして通知を下げ、onResume で
// 希望状態（`desired`）から出し直す。onDestroy（Activity 再生成を含む）で session を解放し、
// 旧インスタンスの操作は `destroyed` の関門で止める。操作は「現在の session・前景・desired あり」
// のときだけ Rust へ渡す。
//
// 守る範囲: 1 プロセス内の操作順、取り下げ後の古い操作で再開しないこと、嘘の成功の禁止、
// session と通知の解放。守らない範囲: 通知を消した後の再表示、アプリの通知を切った端末での
// シェード表示（session は有効でヘッドセットは効く）、API 24〜32 の通知ボタン、背景中の操作、
// 複数プロセス。曲名・例外メッセージはログに出さない。
final class NowPlayingHelper {
    static final int RESULT_OK = 0;
    static final int RESULT_FAILED = 1;
    static final int RESULT_TIMEOUT = 2;

    private static final int CMD_PLAY = 0;
    private static final int CMD_PAUSE = 1;

    private static final long APPLY_TIMEOUT_MS = 1000;
    private static final int PENDING = 0;
    private static final int STARTED = 1;
    private static final int ABANDONED = 2;

    private static final String TAG = "fandhe_now_playing";
    private static final String CHANNEL_ID = "fandhe_now_playing";
    private static final int NOTIFICATION_ID = 1;

    private static native void nativeOnNowPlayingCommand(int code);

    private static final class Call {
        final AtomicInteger state = new AtomicInteger(PENDING);
        final CountDownLatch finished = new CountDownLatch(1);
        volatile int code = RESULT_FAILED;
    }

    // publish の内容。onResume で出し直すために持つ。
    private static final class Desired {
        final String title;
        final boolean playing;
        final long positionMs;
        final long durationMs;

        Desired(String title, boolean playing, long positionMs, long durationMs) {
            this.title = title;
            this.playing = playing;
            this.positionMs = positionMs;
            this.durationMs = durationMs;
        }
    }

    // session ごとに作る。自分が現在の session のときだけ操作を通す（古い session の遅延を止める）。
    private final class Callback extends MediaSession.Callback {
        MediaSession mine;

        @Override
        public void onPlay() {
            deliver(this, CMD_PLAY);
        }

        @Override
        public void onPause() {
            deliver(this, CMD_PAUSE);
        }
    }

    private final Activity activity;
    private final Object lock = new Object();
    private boolean destroyed;
    private boolean foreground = true;
    private MediaSession session;
    private Desired desired;
    // desired / session が変わるたびに進める。タイムアウト後の後片付けが後続の変更を壊さないための照合用。
    private long generation;

    NowPlayingHelper(Activity activity) {
        this.activity = activity;
    }

    private void deliver(Callback cb, int code) {
        synchronized (lock) {
            if (destroyed || !foreground || desired == null || session == null
                    || cb.mine != session) {
                return;
            }
        }
        // Rust の Mutex と入れ子にしないよう `lock` の外で呼ぶ。
        nativeOnNowPlayingCommand(code);
    }

    // egui スレッドから呼ぶ。
    int publish(String title, boolean playing, long positionMs, long durationMs) {
        if (title == null || title.isEmpty()) {
            return RESULT_FAILED;
        }
        final Desired d = new Desired(title, playing, positionMs, durationMs);
        final Call call = new Call();
        final long startGen;
        synchronized (lock) {
            startGen = generation;
        }
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (call.state.compareAndSet(PENDING, STARTED)) {
                    try {
                        synchronized (lock) {
                            call.code = applyPublish(d);
                        }
                    } finally {
                        call.finished.countDown();
                    }
                }
            }
        });
        int code = await(call);
        if (code == RESULT_TIMEOUT) {
            // 放棄した publish が失敗として確定する。Rust は commands を disarm し同じ状態で再試行しない
            // ため、以前の session と表示を残さない。世代が進んでいなければ（後続の publish・clear が
            // 何も変えていなければ）その場で取り下げる。進んでいれば後続の結果を壊さない。
            synchronized (lock) {
                if (generation == startGen) {
                    release();
                }
            }
        }
        return code;
    }

    // egui スレッドから呼ぶ（冪等）。Rust 側は先に操作の受け口を閉じている。UI キューを経由せず、
    // 呼び出しスレッドで `lock` の内側から直接解放して戻る（MediaSession.release と通知の cancel は
    // Binder 呼び出しでスレッドを選ばない）。UI スレッドが塞がっていても、戻った時点で取り下げ済み。
    void clear() {
        synchronized (lock) {
            release();
        }
    }

    private static int await(Call call) {
        boolean interrupted = false;
        try {
            try {
                if (call.finished.await(APPLY_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
                    return call.code;
                }
            } catch (InterruptedException e) {
                interrupted = true;
            }
            if (call.state.compareAndSet(PENDING, ABANDONED)) {
                return RESULT_TIMEOUT;
            }
            // 開始済みは取り消せない。終了まで待ち実結果を返す（失敗報告と実状態を食い違わせない）。
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

    // `lock` を保持した UI スレッドから呼ぶ。
    private int applyPublish(Desired d) {
        if (destroyed || activity.isDestroyed()) {
            return RESULT_FAILED;
        }
        try {
            if (session == null) {
                Callback cb = new Callback();
                MediaSession s = new MediaSession(activity, TAG);
                cb.mine = s;
                session = s; // setCallback 失敗時も catch の release で解放できるよう先に保持する
                s.setCallback(cb, new Handler(Looper.getMainLooper()));
            }
            desired = d;
            generation++;
            show(d);
            return RESULT_OK;
        } catch (RuntimeException e) {
            release();
            return RESULT_FAILED;
        }
    }

    // `lock` を保持した UI スレッドから呼ぶ。session があることが前提。例外は呼び出し側で処理する。
    private void show(Desired d) {
        MediaMetadata.Builder meta = new MediaMetadata.Builder()
                .putString(MediaMetadata.METADATA_KEY_TITLE, d.title);
        if (d.durationMs >= 0) {
            meta.putLong(MediaMetadata.METADATA_KEY_DURATION, d.durationMs);
        }
        session.setMetadata(meta.build());
        session.setPlaybackState(new PlaybackState.Builder()
                .setActions(PlaybackState.ACTION_PLAY | PlaybackState.ACTION_PAUSE
                        | PlaybackState.ACTION_PLAY_PAUSE)
                .setState(d.playing ? PlaybackState.STATE_PLAYING : PlaybackState.STATE_PAUSED,
                        d.positionMs >= 0 ? d.positionMs : PlaybackState.PLAYBACK_POSITION_UNKNOWN,
                        d.playing ? 1f : 0f)
                .build());
        if (!foreground) {
            return;
        }
        session.setActive(true);
        postNotification(d);
    }

    // 通知の失敗では session を落とさない（ヘッドセット操作は通知なしでも効く。表示は守る範囲外）。
    private void postNotification(Desired d) {
        try {
            postNotificationUnchecked(d);
        } catch (RuntimeException e) {
            // 握りつぶす。
        }
    }

    private void postNotificationUnchecked(Desired d) {
        NotificationManager nm = activity.getSystemService(NotificationManager.class);
        if (nm == null) {
            return;
        }
        Notification.Builder b;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NotificationChannel channel = new NotificationChannel(
                    CHANNEL_ID,
                    activity.getString(R.string.now_playing_channel_name),
                    NotificationManager.IMPORTANCE_LOW);
            channel.setShowBadge(false);
            nm.createNotificationChannel(channel);
            b = new Notification.Builder(activity, CHANNEL_ID);
        } else {
            b = new Notification.Builder(activity);
        }
        // contentIntent は付けない（exported な入口を増やさない）。
        b.setSmallIcon(android.R.drawable.ic_media_play)
                .setContentTitle(d.title)
                .setOngoing(d.playing)
                .setStyle(new Notification.MediaStyle().setMediaSession(session.getSessionToken()));
        nm.notify(TAG, NOTIFICATION_ID, b.build());
    }

    private void cancelNotification() {
        try {
            NotificationManager nm = activity.getSystemService(NotificationManager.class);
            if (nm != null) {
                nm.cancel(TAG, NOTIFICATION_ID);
            }
        } catch (RuntimeException e) {
            // 取り下げの失敗は握りつぶす（残りは OS が通知ごと片付ける）。
        }
    }

    // `lock` を保持した UI スレッドから呼ぶ。冪等。
    private void release() {
        desired = null;
        generation++;
        MediaSession s = session;
        session = null;
        cancelNotification();
        if (s != null) {
            // 途中の例外で release を飛ばさない。
            try {
                s.setCallback(null);
            } catch (RuntimeException e) {
                // 続行
            }
            try {
                s.setActive(false);
            } catch (RuntimeException e) {
                // 続行
            }
            try {
                s.release();
            } catch (RuntimeException e) {
                // 参照は捨てた。
            }
        }
    }

    // `MainActivity.onPause`（UI スレッド）から呼ぶ。
    void onPause() {
        synchronized (lock) {
            foreground = false;
            if (session != null) {
                try {
                    session.setActive(false);
                } catch (RuntimeException e) {
                    release();
                    return;
                }
                cancelNotification();
            }
        }
    }

    // `MainActivity.onResume`（UI スレッド）から呼ぶ。
    void onResume() {
        synchronized (lock) {
            foreground = true;
            if (session != null && desired != null && !destroyed) {
                try {
                    show(desired);
                } catch (RuntimeException e) {
                    release();
                }
            }
        }
    }

    // `MainActivity.onDestroy`（UI スレッド）から呼ぶ。以後に届く旧要求・旧操作は何もしない。
    void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            release();
        }
    }
}
