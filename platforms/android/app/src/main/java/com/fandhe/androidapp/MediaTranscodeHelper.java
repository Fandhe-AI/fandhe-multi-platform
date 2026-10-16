package com.fandhe.androidapp;

import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.media.MediaMuxer;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.nio.ByteBuffer;

// 動画・音声の変換画面（Issue #314、親 #91 の b-android）の Android 実装ヘルパー。
// Rust の `device/android_media_transcode.rs` がドライバのワーカースレッドから
// `MainActivity.mediaTranscode` 経由で同期 JNI で呼ぶ。Java から Rust への native コールバックは無く、
// JNI 署名の照合先は Java の 1 か所だけ。
// 扱う範囲は「モノラル PCM 16bit を AAC-LC の m4a へ 1 回変換する」ことだけ
// （`MediaCodec` + `MediaMuxer`。動画・進捗・取り消しは扱わない）。
//
// コード表の SSOT はここで、Rust 側 `android_media_transcode.rs` が同じ値で解釈する。
//   戻り値 byte[] の先頭 1 バイト: 0=完了（残りが m4a 本体）/ 1=期限切れ / 2=失敗
//   Java は成功を判定しない。m4a の妥当性は Rust の `check_m4a` が決める。
//
// スレッド: 呼び出し元スレッドで同期実行する（main Looper は使わない）。状態を持たない static のみ。
// 期限: `remainingMs` を呼び出し開始時の `System.nanoTime()`（単調時計）に足して締切とし、
// 処理ループの各周回で確かめる。過ぎたら codec・muxer・一時ファイルを解放してから 1 を返す。
// ライフサイクル: 変換は短いため onPause / onDestroy では中断しない。
//
// 守る範囲: 期限での中断・全経路での資源解放・失敗を成功に畳まないこと。
// 守らない範囲: 端末ごとの所要時間とビットレートの違い、複数プロセス間。
// PCM・出力・例外メッセージはログに出さない。
final class MediaTranscodeHelper {
    private static final int OK = 0, TIMEOUT = 1, FAILED = 2;
    // Rust 側と同じ上限。
    private static final int MAX_PCM_BYTES = 16 * 1024 * 1024;
    private static final int MAX_M4A_BYTES = 4 * 1024 * 1024;
    private static final long POLL_US = 10_000;

    private MediaTranscodeHelper() {}

    static byte[] transcode(File cacheDir, byte[] pcm, int sampleRate, long remainingMs) {
        if (remainingMs <= 0) {
            return code(TIMEOUT);
        }
        if (pcm == null || pcm.length == 0 || (pcm.length & 1) != 0 || pcm.length > MAX_PCM_BYTES
                || sampleRate <= 0 || cacheDir == null) {
            return code(FAILED);
        }
        final long deadline = System.nanoTime() + remainingMs * 1_000_000L;
        File out = null;
        MediaCodec codec = null;
        MediaMuxer muxer = null;
        boolean muxerStarted = false;
        int result = FAILED;
        try {
            out = File.createTempFile("transcode", ".m4a", cacheDir);
            MediaFormat fmt = MediaFormat.createAudioFormat("audio/mp4a-latm", sampleRate, 1);
            fmt.setInteger(MediaFormat.KEY_AAC_PROFILE, MediaCodecInfo.CodecProfileLevel.AACObjectLC);
            fmt.setInteger(MediaFormat.KEY_BIT_RATE, 64000);
            fmt.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 16384);
            codec = MediaCodec.createEncoderByType("audio/mp4a-latm");
            codec.configure(fmt, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
            codec.start();
            muxer = new MediaMuxer(out.getAbsolutePath(), MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4);

            MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
            int off = 0;
            boolean inputDone = false, eos = false;
            int track = -1;
            while (!eos) {
                if (System.nanoTime() >= deadline) {
                    result = TIMEOUT;
                    throw new IllegalStateException();
                }
                if (!inputDone) {
                    int i = codec.dequeueInputBuffer(POLL_US);
                    if (i >= 0) {
                        ByteBuffer b = codec.getInputBuffer(i);
                        b.clear();
                        int n = Math.min(b.capacity() & ~1, pcm.length - off);
                        if (n <= 0) {
                            throw new IllegalStateException();
                        }
                        b.put(pcm, off, n);
                        long pts = (long) (off / 2) * 1_000_000L / sampleRate;
                        off += n;
                        inputDone = off >= pcm.length;
                        codec.queueInputBuffer(i, 0, n, pts,
                                inputDone ? MediaCodec.BUFFER_FLAG_END_OF_STREAM : 0);
                    }
                }
                int o = codec.dequeueOutputBuffer(info, POLL_US);
                if (o == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                    if (muxerStarted) {
                        throw new IllegalStateException();
                    }
                    track = muxer.addTrack(codec.getOutputFormat());
                    muxer.start();
                    muxerStarted = true;
                } else if (o >= 0) {
                    ByteBuffer b = codec.getOutputBuffer(o);
                    if ((info.flags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0 && info.size > 0) {
                        if (!muxerStarted) {
                            throw new IllegalStateException();
                        }
                        b.position(info.offset);
                        b.limit(info.offset + info.size);
                        muxer.writeSampleData(track, b, info);
                    }
                    eos = (info.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0;
                    codec.releaseOutputBuffer(o, false);
                }
            }
            if (!muxerStarted) {
                throw new IllegalStateException();
            }
            // moov を書き終えてからでないと読めない。stop が失敗したら失敗。
            muxer.stop();
            muxerStarted = false;
            result = OK;
        } catch (Exception e) {
            // result は TIMEOUT か FAILED のまま。詳細はログに出さない。
            if (result == OK) {
                result = FAILED;
            }
        } finally {
            if (codec != null) {
                try { codec.stop(); } catch (Exception ignored) { }
                try { codec.release(); } catch (Exception ignored) { }
            }
            if (muxer != null) {
                if (muxerStarted) {
                    try { muxer.stop(); } catch (Exception ignored) { }
                }
                try { muxer.release(); } catch (Exception ignored) { }
            }
        }
        try {
            if (result != OK) {
                return code(result);
            }
            long len = out.length();
            if (len > MAX_M4A_BYTES) {
                return code(FAILED);
            }
            byte[] ret = new byte[1 + (int) len];
            ret[0] = OK;
            try (FileInputStream in = new FileInputStream(out)) {
                int got = 0;
                while (got < len) {
                    int r = in.read(ret, 1 + got, (int) len - got);
                    if (r < 0) {
                        return code(FAILED);
                    }
                    got += r;
                }
            }
            return ret;
        } catch (IOException | RuntimeException e) {
            return code(FAILED);
        } finally {
            if (out != null) {
                //noinspection ResultOfMethodCallIgnored
                out.delete();
            }
        }
    }

    private static byte[] code(int c) {
        return new byte[] {(byte) c};
    }
}
