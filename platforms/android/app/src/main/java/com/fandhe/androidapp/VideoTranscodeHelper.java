package com.fandhe.androidapp;

import android.media.Image;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.media.MediaMuxer;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;

// 動画の変換画面（Issue #1017、親 #340 の b-android）の Android 実装ヘルパー。
// Rust の `device/android_video_transcode.rs` がドライバのワーカースレッドから
// `MainActivity.videoTranscode` 経由で同期 JNI で呼ぶ。Java から Rust への native コールバックは無く、
// JNI 署名の照合先は Java の 1 か所だけ。
// 扱う範囲は「実行時に作った 1280x720・15 fps・30 フレームの H.264 を 640x360 の mp4 へ 1 回変換する」ことだけ
// （保存・共有・進捗・取り消しは扱わない）。流れは、符号化器で元動画を作ってメモリに溜め
// → 復号器で戻し → Java の YUV 間引きで縮小 → 符号化器 → MediaMuxer で mp4。
// 元動画の絵は `fandhe_device::video_transcode::fill_source_frame_bgra` と同じ縦縞（幅 32 px・
// フレームごとに 16 px 移動）を YUV で直接描く（巨大な BGRA を JNI で渡さないため）。
//
// コード表の SSOT はここで、Rust 側 `android_video_transcode.rs` が同じ値で解釈する。
//   戻り値 byte[] の先頭 1 バイト: 0=完了（残りが mp4 本体）/ 1=期限切れ / 2=失敗 / 3=非対応
//   / 4=前回の解放を確認できない
//   Java は成功を判定しない。mp4 の妥当性は Rust の `check_mp4_video` が決める。
//
// スレッド: 呼び出し元スレッドで同期実行する（main Looper は使わない）。
// 期限: `remainingMs` を呼び出し開始時の `System.nanoTime()`（単調時計）に足して締切とし、
// 各ループの周回で確かめる。過ぎたら codec・muxer・一時ファイルを解放してから 1 を返す。
// ライフサイクル: 変換は短いため onPause / onDestroy では中断しない。
// 非対応: 事前の機種判定は置かず、codec が作れない・`isSizeSupported` が偽なら 3。
// YUV は `Image` の pixelStride / rowStride を尊重して読み書きする。
//
// 守る範囲: 期限での中断・全経路での資源解放（消せない一時ファイルは記録し次の要求の前に再削除）・
// 失敗を成功に畳まないこと。守らない範囲: 端末ごとの所要時間とビットレートの違い、複数プロセス間。
// 映像・出力・例外メッセージはログに出さない。
final class VideoTranscodeHelper {
    private static final int OK = 0, TIMEOUT = 1, FAILED = 2, UNSUPPORTED = 3, RELEASE_PENDING = 4;
    private static final String AVC = "video/avc";
    private static final int SW = 1280, SH = 720, FPS = 15, FRAMES = 30;
    private static final int TW = 640, TH = 360;
    // 出力の上限は Rust 側と同じ（確保の前に長さを検査する）。元動画の符号化結果の上限はこの層だけで持つ。
    private static final int MAX_MP4_BYTES = 4 * 1024 * 1024;
    private static final int MAX_SOURCE_BYTES = 8 * 1024 * 1024;
    private static final long POLL_US = 10_000;
    // BGRA (R240,G160,B40) / (230,230,230) 相当の BT.601 YUV。
    private static final byte Y_ON = (byte) 162, U_ON = (byte) 64, V_ON = (byte) 172;
    private static final byte Y_OFF = (byte) 214, U_OFF = (byte) 128, V_OFF = (byte) 128;

    // 前回の呼び出しが Java 内でまだ走っているか（Rust の期限切れ後もワーカーが残りうる）。
    private static final AtomicBoolean inFlight = new AtomicBoolean(false);
    // 消せなかった一時ファイル。次の要求の前に消し直す。
    private static final List<File> leftovers = new ArrayList<>();

    private static final class Timeout extends RuntimeException {}
    private static final class Unsupported extends RuntimeException {}

    private static final class Source {
        MediaFormat format;
        final List<byte[]> data = new ArrayList<>();
        final List<long[]> meta = new ArrayList<>(); // {pts, flags}
        int bytes;
    }

    private final long deadline;
    private MediaMuxer muxer;
    private boolean muxerStarted;
    private boolean releaseFailed;

    private VideoTranscodeHelper(long deadline) {
        this.deadline = deadline;
    }

    static byte[] transcode(File cacheDir, long remainingMs) {
        if (remainingMs <= 0) {
            return code(TIMEOUT);
        }
        if (cacheDir == null) {
            return code(FAILED);
        }
        if (!inFlight.compareAndSet(false, true)) {
            return code(RELEASE_PENDING);
        }
        try {
            if (!retryLeftovers()) {
                return code(RELEASE_PENDING);
            }
            return new VideoTranscodeHelper(System.nanoTime() + remainingMs * 1_000_000L).run(cacheDir);
        } finally {
            inFlight.set(false);
        }
    }

    private static boolean retryLeftovers() {
        synchronized (leftovers) {
            for (Iterator<File> it = leftovers.iterator(); it.hasNext(); ) {
                File f = it.next();
                if (!f.exists() || f.delete()) {
                    it.remove();
                }
            }
            return leftovers.isEmpty();
        }
    }

    private byte[] run(File cacheDir) {
        File out = null;
        int result = FAILED;
        byte[] ret = null;
        try {
            out = File.createTempFile("vtranscode", ".mp4", cacheDir);
            Source src = encodeSource();
            convert(src, out);
            long len = out.length();
            if (len > MAX_MP4_BYTES) {
                throw new IllegalStateException();
            }
            ret = new byte[1 + (int) len];
            ret[0] = OK;
            try (FileInputStream in = new FileInputStream(out)) {
                int got = 0;
                while (got < len) {
                    int r = in.read(ret, 1 + got, (int) len - got);
                    if (r < 0) {
                        throw new IOException();
                    }
                    got += r;
                }
            }
            result = OK;
        } catch (Timeout e) {
            result = TIMEOUT;
        } catch (Unsupported e) {
            result = UNSUPPORTED;
        } catch (Exception e) {
            // 詳細はログに出さない。
            result = FAILED;
        } finally {
            if (muxer != null) {
                if (muxerStarted) {
                    try { muxer.stop(); } catch (Exception ignored) { }
                }
                try { muxer.release(); } catch (Exception e) { releaseFailed = true; }
            }
            if (out != null && !out.delete() && out.exists()) {
                synchronized (leftovers) {
                    leftovers.add(out);
                }
            }
        }
        if (releaseFailed && result == OK) {
            result = FAILED;
        }
        return result == OK ? ret : code(result);
    }

    private void checkDeadline() {
        if (System.nanoTime() >= deadline) {
            throw new Timeout();
        }
    }

    private void release(MediaCodec c) {
        if (c == null) {
            return;
        }
        try { c.stop(); } catch (Exception ignored) { }
        try { c.release(); } catch (Exception e) { releaseFailed = true; }
    }

    // codec を作り、OS に寸法の対応を問う。作れない・偽なら非対応。
    private static MediaCodec create(boolean encoder, int w, int h) {
        MediaCodec c = null;
        try {
            c = encoder ? MediaCodec.createEncoderByType(AVC) : MediaCodec.createDecoderByType(AVC);
            MediaCodecInfo.VideoCapabilities vc =
                    c.getCodecInfo().getCapabilitiesForType(AVC).getVideoCapabilities();
            if (vc == null || !vc.isSizeSupported(w, h)) {
                throw new Unsupported();
            }
            return c;
        } catch (IOException | IllegalArgumentException | Unsupported e) {
            if (c != null) {
                try { c.release(); } catch (Exception ignored) { }
            }
            throw new Unsupported();
        }
    }

    private static MediaFormat encoderFormat(int w, int h, int bitRate) {
        MediaFormat f = MediaFormat.createVideoFormat(AVC, w, h);
        f.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible);
        f.setInteger(MediaFormat.KEY_BIT_RATE, bitRate);
        f.setInteger(MediaFormat.KEY_FRAME_RATE, FPS);
        f.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1);
        return f;
    }

    // 段 1: 元動画を符号化してメモリに溜める（一時ファイルにしない）。
    private Source encodeSource() {
        MediaCodec enc = null;
        try {
            enc = create(true, SW, SH);
            enc.configure(encoderFormat(SW, SH, 2_000_000), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
            enc.start();
            Source src = new Source();
            MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
            int fed = 0;
            boolean eos = false;
            while (!eos) {
                checkDeadline();
                if (fed < FRAMES) {
                    int i = enc.dequeueInputBuffer(POLL_US);
                    if (i >= 0) {
                        Image img = enc.getInputImage(i);
                        if (img == null) {
                            throw new IllegalStateException();
                        }
                        drawSource(img, fed);
                        long pts = (long) fed * 1_000_000L / FPS;
                        fed++;
                        enc.queueInputBuffer(i, 0, SW * SH * 3 / 2, pts,
                                fed == FRAMES ? MediaCodec.BUFFER_FLAG_END_OF_STREAM : 0);
                    }
                }
                int o = enc.dequeueOutputBuffer(info, POLL_US);
                if (o == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                    src.format = enc.getOutputFormat();
                } else if (o >= 0) {
                    if ((info.flags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0 && info.size > 0) {
                        src.bytes += info.size;
                        if (src.bytes > MAX_SOURCE_BYTES) {
                            throw new IllegalStateException();
                        }
                        ByteBuffer b = enc.getOutputBuffer(o);
                        b.position(info.offset);
                        b.limit(info.offset + info.size);
                        byte[] d = new byte[info.size];
                        b.get(d);
                        src.data.add(d);
                        // EOS は復号側で空バッファとして別に送るので、ここでは外す。
                        src.meta.add(new long[] {info.presentationTimeUs,
                                info.flags & ~MediaCodec.BUFFER_FLAG_END_OF_STREAM});
                    }
                    eos = (info.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0;
                    enc.releaseOutputBuffer(o, false);
                }
            }
            if (src.format == null || src.data.isEmpty()) {
                throw new IllegalStateException();
            }
            return src;
        } finally {
            release(enc);
        }
    }

    // 段 2: 復号 → 縮小 → 符号化 → MediaMuxer。
    private void convert(Source src, File out) throws IOException {
        MediaCodec dec = null, enc = null;
        try {
            dec = create(false, SW, SH);
            enc = create(true, TW, TH);
            MediaFormat df = src.format;
            df.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible);
            dec.configure(df, null, null, 0);
            dec.start();
            enc.configure(encoderFormat(TW, TH, 800_000), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
            enc.start();
            muxer = new MediaMuxer(out.getAbsolutePath(), MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4);

            MediaCodec.BufferInfo di = new MediaCodec.BufferInfo(), ei = new MediaCodec.BufferInfo();
            int next = 0, track = -1, encIn = -1;
            long lastPts = 0;
            boolean decInDone = false, decOutDone = false, encEosQueued = false, encEos = false;
            while (!encEos) {
                checkDeadline();
                if (!decInDone) {
                    int i = dec.dequeueInputBuffer(0);
                    if (i >= 0) {
                        ByteBuffer b = dec.getInputBuffer(i);
                        b.clear();
                        if (next < src.data.size()) {
                            byte[] d = src.data.get(next);
                            if (d.length > b.capacity()) {
                                throw new IllegalStateException();
                            }
                            b.put(d);
                            long[] m = src.meta.get(next);
                            dec.queueInputBuffer(i, 0, d.length, m[0], (int) m[1]);
                            next++;
                        } else {
                            dec.queueInputBuffer(i, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM);
                            decInDone = true;
                        }
                    }
                }
                if (!decOutDone) {
                    if (encIn < 0) {
                        encIn = enc.dequeueInputBuffer(0);
                    }
                    if (encIn >= 0) {
                        int o = dec.dequeueOutputBuffer(di, 0);
                        if (o >= 0) {
                            if (di.size > 0) {
                                Image in = dec.getOutputImage(o);
                                try {
                                    Image dst = enc.getInputImage(encIn);
                                    if (in == null || dst == null) {
                                        throw new IllegalStateException();
                                    }
                                    scale(in, dst);
                                } finally {
                                    if (in != null) {
                                        in.close();
                                    }
                                }
                                lastPts = di.presentationTimeUs;
                                enc.queueInputBuffer(encIn, 0, TW * TH * 3 / 2, lastPts, 0);
                                encIn = -1;
                            }
                            decOutDone = (di.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0;
                            dec.releaseOutputBuffer(o, false);
                        }
                    }
                }
                if (decOutDone && !encEosQueued) {
                    if (encIn < 0) {
                        encIn = enc.dequeueInputBuffer(0);
                    }
                    if (encIn >= 0) {
                        enc.queueInputBuffer(encIn, 0, 0, lastPts, MediaCodec.BUFFER_FLAG_END_OF_STREAM);
                        encIn = -1;
                        encEosQueued = true;
                    }
                }
                int o = enc.dequeueOutputBuffer(ei, POLL_US);
                if (o == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                    if (muxerStarted) {
                        throw new IllegalStateException();
                    }
                    track = muxer.addTrack(enc.getOutputFormat());
                    muxer.start();
                    muxerStarted = true;
                } else if (o >= 0) {
                    ByteBuffer b = enc.getOutputBuffer(o);
                    if ((ei.flags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0 && ei.size > 0) {
                        if (!muxerStarted) {
                            throw new IllegalStateException();
                        }
                        b.position(ei.offset);
                        b.limit(ei.offset + ei.size);
                        muxer.writeSampleData(track, b, ei);
                    }
                    encEos = (ei.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0;
                    enc.releaseOutputBuffer(o, false);
                }
            }
            if (!muxerStarted) {
                throw new IllegalStateException();
            }
            // moov を書き終えてからでないと読めない。stop が失敗したら失敗。
            muxer.stop();
            muxerStarted = false;
        } finally {
            release(dec);
            release(enc);
        }
    }

    // 元動画の index 枚目を YUV で描く（縦縞。Rust の fill_source_frame_bgra と同じ位置）。
    private static void drawSource(Image img, int index) {
        int shift = index * 16;
        Image.Plane[] planes = img.getPlanes();
        for (int p = 0; p < 3; p++) {
            int pw = p == 0 ? SW : SW / 2, ph = p == 0 ? SH : SH / 2;
            byte on = p == 0 ? Y_ON : p == 1 ? U_ON : V_ON;
            byte off = p == 0 ? Y_OFF : p == 1 ? U_OFF : V_OFF;
            byte[] row = new byte[pw];
            for (int x = 0; x < pw; x++) {
                int lx = p == 0 ? x : 2 * x;
                row[x] = ((lx + shift) / 32) % 2 == 0 ? on : off;
            }
            ByteBuffer buf = planes[p].getBuffer();
            int rs = planes[p].getRowStride(), ps = planes[p].getPixelStride();
            for (int y = 0; y < ph; y++) {
                if (ps == 1) {
                    buf.position(y * rs);
                    buf.put(row, 0, pw);
                } else {
                    for (int x = 0; x < pw; x++) {
                        buf.put(y * rs + x * ps, row[x]);
                    }
                }
            }
        }
    }

    // 各面を縦横 2:1 で間引く。両側の pixelStride / rowStride を尊重する。
    private static void scale(Image src, Image dst) {
        if (src.getWidth() != SW || src.getHeight() != SH) {
            throw new IllegalStateException();
        }
        Image.Plane[] s = src.getPlanes(), d = dst.getPlanes();
        for (int p = 0; p < 3; p++) {
            int dw = p == 0 ? TW : TW / 2, dh = p == 0 ? TH : TH / 2;
            ByteBuffer sb = s[p].getBuffer(), db = d[p].getBuffer();
            int sr = s[p].getRowStride(), sp = s[p].getPixelStride();
            int dr = d[p].getRowStride(), dp = d[p].getPixelStride();
            for (int y = 0; y < dh; y++) {
                for (int x = 0; x < dw; x++) {
                    db.put(y * dr + x * dp, sb.get(2 * y * sr + 2 * x * sp));
                }
            }
        }
    }

    private static byte[] code(int c) {
        return new byte[] {(byte) c};
    }
}
