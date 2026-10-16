package com.fandhe.androidapp;

import android.media.MediaDataSource;
import android.media.MediaPlayer;
import android.os.Handler;
import android.os.Looper;

import java.util.HashSet;
import java.util.Set;

// 音声再生画面（Issue #270、親 #86 の b-android）の Android 実装ヘルパー。MainActivity から
// 生成・委譲される（Rust の `device/android_media_playback.rs` が egui スレッドから
// `MainActivity.mediaPlaybackLoad` / `Control` / `Snapshot` / `Shutdown` を同期 JNI で呼ぶ）。
// 素材は Rust が実行時に生成する WAV で、`MediaPlayer` + `MediaDataSource`（API 23）で鳴らす。
// 動画・背景再生・シーク・ネットワーク再生は扱わない。
//
// コード表の SSOT はここで、Rust 側 `android_media_playback.rs` が同じ値で解釈する。
//   load/control の戻り: 0=受付 / 1=失敗（破棄済み・内部）/ 2=WAV 不正 / 3=世代不一致
//   control の op      : 0=play / 1=pause / 2=stop
//   snapshot           : long[4] = {世代, phase, 位置ms, 長さms}（未取得は -1）
//   phase              : 0 Idle / 1 Ready / 2 Playing / 3 Paused / 4 Ended /
//                        5 Failed(Decode) / 6 Loading / 7 Interrupted / 8 Failed(Internal)
//
// スレッド: `MediaPlayer` の生成・操作・解放・位置取得はすべて main Looper 上で行う。
// play/snapshot は post して戻り、pause/stop は main の完了を（lock 外で）待って実結果を返す。
// 状態はすべて `lock` 1 本の内側で読み書きし、post した Runnable・リスナーは実行時に
// 「破棄されていない・世代が一致・player が同一」を照合してから動く。
//
// ライフサイクル: `onPause` で再生中なら一時停止して Interrupted にし、準備中の再生待ちは
// 取り消す（準備完了でも開始しない）。`onResume` は自動再開しない。`onDestroy`（再生成を含む）
// は積み残しを除去して release する。新インスタンスは Idle・世代 0 から始まる。
//
// 守る範囲: 世代の照合・嘘の成功の禁止（onError は完了と区別）・資源の解放。
// 守らない範囲: 位置表示の遅れ、音声フォーカス（他アプリの音声と混ざる）、複数プロセス間。
// 再生内容・例外メッセージ・what/extra の生値はログに出さない。
final class MediaPlaybackHelper {
    private static final int OK = 0;
    private static final int FAILED = 1;
    private static final int BAD_WAV = 2;
    private static final int BAD_GEN = 3;

    private static final int IDLE = 0, READY = 1, PLAYING = 2, PAUSED = 3, ENDED = 4,
            FAILED_DECODE = 5, LOADING = 6, INTERRUPTED = 7, FAILED_INTERNAL = 8;
    private static final int OP_PLAY = 0, OP_PAUSE = 1;
    private static final int POST_START = 0, POST_PAUSE = 1;
    // Rust 側と同じ上限。FFI に過大な byte[] を渡さない。
    private static final int MAX_WAV_BYTES = 16 * 1024 * 1024;

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Object lock = new Object();
    private MediaPlayer player;       // lock の内側。生成は main 上
    private long current;             // 現世代（0=なし）
    private int phase = IDLE;
    private boolean prepared;
    private boolean started;          // start() 成功済みで未 pause（lock の内側）
    private long pos = -1;            // main で更新する位置キャッシュ
    private boolean wantPlay;         // 準備中に来た再生要求
    private boolean foreground = true;
    private boolean destroyed;
    // release 待ちの旧 player。onDestroy が post 済み Runnable を除去しても漏らさないため（契約表 9）。
    private final Set<MediaPlayer> pendingRelease = new HashSet<>();
    private long duration = -1;

    int load(long gen, byte[] wav) {
        synchronized (lock) {
            if (destroyed) return FAILED;
            // 失敗経路でも旧プレーヤーは解放する（trait doc）。
            releaseLocked();
            if (gen <= 0 || wav == null || wav.length == 0 || wav.length > MAX_WAV_BYTES) {
                return BAD_WAV;
            }
            current = gen;
            phase = LOADING;
            handler.post(() -> create(gen, wav));
            return OK;
        }
    }

    // main 上。MediaPlayer を作って準備を始める。
    private void create(long gen, byte[] wav) {
        synchronized (lock) {
            if (destroyed || current != gen) return;
            MediaPlayer mp = null;
            try {
                mp = new MediaPlayer();
                mp.setOnPreparedListener(p -> onPrepared(gen, p));
                mp.setOnCompletionListener(p -> {
                    synchronized (lock) {
                        if (valid(gen, p)) {
                            phase = ENDED;
                            started = false;
                        }
                    }
                });
                mp.setOnErrorListener((p, what, extra) -> {
                    synchronized (lock) {
                        if (valid(gen, p)) {
                            failLocked(isDecodeError(what) || isDecodeError(extra)
                                    ? FAILED_DECODE : FAILED_INTERNAL);
                        }
                    }
                    return true; // onCompletion を呼ばせない
                });
                mp.setDataSource(new ByteArraySource(wav));
                mp.prepareAsync();
                player = mp;
            } catch (RuntimeException e) {
                failCreate(mp);
            }
        }
    }

    // 作りかけを即解放する（契約表 9）。
    private void failCreate(MediaPlayer mp) {
        if (mp != null) mp.release();
        phase = FAILED_INTERNAL;
    }

    private static boolean isDecodeError(int code) {
        return code == MediaPlayer.MEDIA_ERROR_UNSUPPORTED
                || code == MediaPlayer.MEDIA_ERROR_MALFORMED
                || code == MediaPlayer.MEDIA_ERROR_IO;
    }

    // lock の内側で呼ぶ。
    private boolean valid(long gen, MediaPlayer mp) {
        return !destroyed && current == gen && player == mp;
    }

    private void onPrepared(long gen, MediaPlayer mp) {
        synchronized (lock) {
            if (!valid(gen, mp)) return;
            prepared = true;
            int d = mp.getDuration();
            duration = d < 0 ? -1 : d;
            if (wantPlay && foreground) {
                wantPlay = false;
                try {
                    mp.start();
                    started = true;
                    phase = PLAYING;
                } catch (IllegalStateException e) {
                    failLocked(FAILED_INTERNAL);
                }
            } else if (phase != INTERRUPTED) {
                phase = READY; // 中断は維持する（準備完了でも Ready に戻さない）
            }
        }
    }

    int control(long gen, int op) {
        MediaPlayer stopMp = null;
        boolean rewind = false;
        synchronized (lock) {
            if (destroyed) return FAILED;
            if (gen <= 0 || current != gen) return BAD_GEN;
            if (phase == FAILED_DECODE || phase == FAILED_INTERNAL || phase == IDLE) return FAILED;
            if (op == OP_PLAY) {
                if (!foreground) { // 背景での開始を防ぐ（契約表 1）
                    wantPlay = false;
                    phase = INTERRUPTED;
                } else if (!prepared) {
                    wantPlay = true;
                } else {
                    // start の成功までは Loading（開始待ち）に留め、Rust 側の期限判定を維持する。
                    postOp(gen, POST_START);
                    phase = LOADING;
                }
                return OK;
            }
            wantPlay = false;
            // pause は再生中・開始待ちのみ、stop は準備済みなら常に対象。
            if (!prepared || (op == OP_PAUSE && phase != PLAYING && phase != LOADING)) return OK;
            stopMp = player;
            rewind = op != OP_PAUSE;
            // phase は main での実処理が成功した後に確定する（期限切れで取り消した場合に実態とずらさない）。
        }
        // MediaPlayer の操作は生成した main へ統一し、完了を待って実結果を返す（契約表 2）。
        // lock は持たずに待つ（main 側が lock を取るため）。
        final MediaPlayer mp = stopMp;
        final boolean rw = rewind;
        return runOnMain(() -> {
            synchronized (lock) {
                if (!valid(gen, mp)) return BAD_GEN;
                if (phase == FAILED_DECODE || phase == FAILED_INTERNAL) return FAILED;
                if (!rw && phase != PLAYING && phase != LOADING) return OK; // 待機中に中断・終了済み
                return stopNow(mp, rw) ? OK : FAILED;
            }
        });
    }

    // main で実行して結果を返す。main 上なら直接実行する。それ以外は 3 秒待ち、未開始のまま期限が
    // 切れたら取り消して失敗（以後 Runnable は何もしない）、開始済みなら完了まで待って実結果を返す
    // （契約表 3。開始と取り消しは state の CAS で排他）。
    private int runOnMain(java.util.concurrent.Callable<Integer> task) {
        try {
            if (Looper.myLooper() == Looper.getMainLooper()) return task.call();
            final int[] out = {FAILED};
            final java.util.concurrent.atomic.AtomicInteger state =
                    new java.util.concurrent.atomic.AtomicInteger(0); // 0 未開始 / 1 開始済み / 2 取消
            final java.util.concurrent.CountDownLatch done = new java.util.concurrent.CountDownLatch(1);
            handler.post(() -> {
                if (!state.compareAndSet(0, 1)) return;
                try {
                    out[0] = task.call();
                } catch (Exception e) {
                    out[0] = FAILED;
                } finally {
                    done.countDown();
                }
            });
            if (!done.await(3, java.util.concurrent.TimeUnit.SECONDS)) {
                if (state.compareAndSet(0, 2)) return FAILED;
                done.await(); // 開始済み。短い処理なので完了を待つ
            }
            return out[0];
        } catch (Exception e) {
            return FAILED;
        }
    }

    // lock の内側・main 上で呼ぶ。開始済みなら一時停止（Prepared の pause は不正なので開始前は触らない）、
    // rewind なら先頭へ。失敗は Failed(Internal) にして false。
    private boolean stopNow(MediaPlayer mp, boolean rewind) {
        try {
            if (started) {
                mp.pause();
                started = false;
            }
            if (rewind) mp.seekTo(0);
            phase = rewind ? READY : PAUSED;
            return true;
        } catch (IllegalStateException e) {
            failLocked(FAILED_INTERNAL);
            return false;
        }
    }

    // lock の内側で呼ぶ。prepared 後なので player は非 null。
    private void postOp(long gen, int kind) {
        MediaPlayer mp = player;
        handler.post(() -> {
            synchronized (lock) {
                if (!valid(gen, mp)) return;
                try {
                    if (kind == POST_START) {
                        // 開始待ちのままのときだけ開始し、成功後に Playing を確定する。
                        if (phase == LOADING) {
                            mp.start();
                            started = true;
                            phase = PLAYING;
                        }
                    } else {
                        if (started) {
                            mp.pause();
                            started = false;
                        }
                    }
                } catch (IllegalStateException e) {
                    failLocked(FAILED_INTERNAL);
                }
            }
        });
    }

    long[] snapshot() {
        synchronized (lock) {
            if (current == 0) return new long[] {0, IDLE, -1, -1};
            boolean live = phase == READY || phase == PLAYING || phase == PAUSED
                    || phase == ENDED || phase == INTERRUPTED;
            // getCurrentPosition は main でだけ呼ぶ。ここではキャッシュを返し、更新を main へ頼む。
            if (prepared && live && player != null) refreshPos(current, player);
            return new long[] {current, phase, prepared && live ? pos : -1, prepared ? duration : -1};
        }
    }

    private void refreshPos(long gen, MediaPlayer mp) {
        handler.post(() -> {
            synchronized (lock) {
                if (!valid(gen, mp)) return;
                try {
                    pos = mp.getCurrentPosition();
                } catch (IllegalStateException e) {
                    pos = -1;
                }
            }
        });
    }

    void onResume() {
        synchronized (lock) {
            foreground = true; // 自動では再開しない
        }
    }

    void onPause() {
        synchronized (lock) {
            foreground = false;
            if (phase == PLAYING) {
                postOp(current, POST_PAUSE);
                phase = INTERRUPTED;
            } else if (phase == LOADING && (wantPlay || prepared)) {
                wantPlay = false;
                phase = INTERRUPTED;
            }
        }
    }

    void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            handler.removeCallbacksAndMessages(null);
            if (player != null) player.release(); // main 上なので直接呼ぶ
            for (MediaPlayer old : pendingRelease) old.release();
            pendingRelease.clear();
            player = null;
            current = 0;
        }
    }

    // 冪等。
    void shutdown() {
        synchronized (lock) {
            releaseLocked();
        }
    }

    // lock の内側。旧 player の release を post し、状態を Idle へ戻す。
    private void releaseLocked() {
        postRelease();
        current = 0;
        phase = IDLE;
        prepared = false;
        started = false;
        pos = -1;
        wantPlay = false;
        duration = -1;
    }

    // lock の内側。現 player の release を main へ post し、player を手放す（資源は ByteArraySource ごと解放）。
    private void postRelease() {
        MediaPlayer old = player;
        if (old != null) {
            pendingRelease.add(old);
            handler.post(() -> {
                synchronized (lock) {
                    if (pendingRelease.remove(old)) old.release();
                }
            });
        }
        player = null;
    }

    // lock の内側。失敗世代と phase は保持したまま player を解放する（契約表 9）。
    // 以後 control は失敗 phase で拒否し、snapshot は位置取得をしない。
    private void failLocked(int failedPhase) {
        phase = failedPhase;
        started = false;
        wantPlay = false;
        postRelease();
    }

    private static final class ByteArraySource extends MediaDataSource {
        private final byte[] data;

        ByteArraySource(byte[] data) {
            this.data = data;
        }

        @Override
        public int readAt(long position, byte[] buffer, int offset, int size) {
            if (position >= data.length) return -1;
            if (size == 0) return 0;
            int n = (int) Math.min(size, data.length - position);
            System.arraycopy(data, (int) position, buffer, offset, n);
            return n;
        }

        @Override
        public long getSize() {
            return data.length;
        }

        @Override
        public void close() {}
    }
}
