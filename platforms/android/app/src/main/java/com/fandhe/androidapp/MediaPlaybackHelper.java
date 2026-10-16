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
// スレッド: `MediaPlayer` の生成・操作・解放は main Looper の Handler へ post して行い、
// JNI 呼び出し側は状態を `lock` の内側で更新して戻る（main への同期待ちは作らない）。
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
    private static final int POST_START = 0, POST_PAUSE = 1, POST_STOP = 2;
    // Rust 側と同じ上限。FFI に過大な byte[] を渡さない。
    private static final int MAX_WAV_BYTES = 16 * 1024 * 1024;

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Object lock = new Object();
    private MediaPlayer player;       // lock の内側。生成は main 上
    private long current;             // 現世代（0=なし）
    private int phase = IDLE;
    private boolean prepared;
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
            MediaPlayer mp = new MediaPlayer();
            try {
                mp.setOnPreparedListener(p -> onPrepared(gen, p));
                mp.setOnCompletionListener(p -> {
                    synchronized (lock) {
                        if (valid(gen, p)) phase = ENDED;
                    }
                });
                mp.setOnErrorListener((p, what, extra) -> {
                    synchronized (lock) {
                        if (valid(gen, p)) {
                            phase = isDecodeError(what) || isDecodeError(extra)
                                    ? FAILED_DECODE : FAILED_INTERNAL;
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
        mp.release();
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
                    phase = PLAYING;
                } catch (IllegalStateException e) {
                    phase = FAILED_INTERNAL;
                }
            } else if (phase != INTERRUPTED) {
                phase = READY; // 中断は維持する（準備完了でも Ready に戻さない）
            }
        }
    }

    int control(long gen, int op) {
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
                    postOp(gen, POST_START);
                    phase = PLAYING;
                }
            } else if (op == OP_PAUSE) {
                wantPlay = false;
                if (prepared && phase == PLAYING) {
                    postOp(gen, POST_PAUSE);
                    phase = PAUSED;
                }
            } else { // stop: 一時停止して先頭へ
                wantPlay = false;
                if (prepared) {
                    postOp(gen, POST_STOP);
                    phase = READY;
                }
            }
            return OK;
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
                        mp.start();
                    } else {
                        mp.pause();
                        if (kind == POST_STOP) mp.seekTo(0);
                    }
                } catch (IllegalStateException e) {
                    phase = FAILED_INTERNAL;
                }
            }
        });
    }

    long[] snapshot() {
        synchronized (lock) {
            if (current == 0) return new long[] {0, IDLE, -1, -1};
            long pos = -1;
            boolean live = phase == READY || phase == PLAYING || phase == PAUSED
                    || phase == ENDED || phase == INTERRUPTED;
            if (prepared && live && player != null) {
                try {
                    pos = player.getCurrentPosition();
                } catch (IllegalStateException e) {
                    pos = -1;
                }
            }
            return new long[] {current, phase, pos, prepared ? duration : -1};
        }
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
            } else if (phase == LOADING && wantPlay) {
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
        current = 0;
        phase = IDLE;
        prepared = false;
        wantPlay = false;
        duration = -1;
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
