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
// 扱う範囲は「実行時に作った H.264 を小さい解像度の mp4 へ 1 回変換する」ことだけ。寸法・fps・フレーム数は
// Rust の `fandhe_device::video_transcode` の定数が SSOT で、JNI 引数で受け取る（ここに複製しない）
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
    // release() が例外を投げた MediaCodec / MediaMuxer。参照を捨てず次の要求の前に再解放し、
    // 確認できるまで RELEASE_PENDING を返す（device-bridge-contracts 類型 9）。
    private static final List<Object> unreleased = new ArrayList<>();

    private static final class Timeout extends RuntimeException {}
    private static final class Unsupported extends RuntimeException {}

    private static final class Source {
        MediaFormat format;
        final List<byte[]> data = new ArrayList<>();
        final List<long[]> meta = new ArrayList<>(); // {pts, flags}
        int bytes;
    }

    private final long deadline;
    private final int sw, sh, fps, frames, tw, th;
    private MediaMuxer muxer;
    private boolean muxerStarted;
    private boolean releaseFailed;

    private VideoTranscodeHelper(long deadline, int sw, int sh, int fps, int frames, int tw, int th) {
        this.deadline = deadline;
        this.sw = sw;
        this.sh = sh;
        this.fps = fps;
        this.frames = frames;
        this.tw = tw;
        this.th = th;
    }

    static byte[] transcode(File cacheDir, int sw, int sh, int fps, int frames, int tw, int th,
            long remainingMs) {
        if (remainingMs <= 0) {
            return code(TIMEOUT);
        }
        // 偶数の正の寸法（4:2:0）と、縮小が 2:1 で割り切れることを前提にする。外れは失敗。
        if (cacheDir == null || sw <= 0 || sh <= 0 || fps <= 0 || frames <= 0
                || sw % 2 != 0 || sh % 2 != 0 || tw != sw / 2 || th != sh / 2 || tw % 2 != 0 || th % 2 != 0) {
            return code(FAILED);
        }
        if (!inFlight.compareAndSet(false, true)) {
            return code(RELEASE_PENDING);
        }
        try {
            if (!retryLeftovers()) {
                return code(RELEASE_PENDING);
            }
            return new VideoTranscodeHelper(System.nanoTime() + remainingMs * 1_000_000L, sw, sh, fps, frames, tw, th).run(cacheDir);
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
            for (Iterator<Object> it = unreleased.iterator(); it.hasNext(); ) {
                if (tryRelease(it.next())) {
                    it.remove();
                }
            }
            return leftovers.isEmpty() && unreleased.isEmpty();
        }
    }

    private static boolean tryRelease(Object o) {
        try {
            if (o instanceof MediaCodec) {
                ((MediaCodec) o).release();
            } else {
                ((MediaMuxer) o).release();
            }
            return true;
        } catch (Exception e) {
            return false;
        }
    }

    // 解放し、失敗したら参照を静的リストへ残す。
    private boolean releaseTracked(Object o) {
        if (tryRelease(o)) {
            return true;
        }
        synchronized (leftovers) {
            unreleased.add(o);
        }
        releaseFailed = true;
        return false;
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
                releaseTracked(muxer);
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
        releaseTracked(c);
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
                if (!tryRelease(c)) {
                    synchronized (leftovers) {
                        unreleased.add(c);
                    }
                }
            }
            throw new Unsupported();
        }
    }

    private MediaFormat encoderFormat(int w, int h, int bitRate) {
        MediaFormat f = MediaFormat.createVideoFormat(AVC, w, h);
        f.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible);
        f.setInteger(MediaFormat.KEY_BIT_RATE, bitRate);
        f.setInteger(MediaFormat.KEY_FRAME_RATE, fps);
        f.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1);
        return f;
    }

    // 段 1: 元動画を符号化してメモリに溜める（一時ファイルにしない）。
    private Source encodeSource() {
        MediaCodec enc = null;
        try {
            enc = create(true, sw, sh);
            enc.configure(encoderFormat(sw, sh, 2_000_000), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
            enc.start();
            Source src = new Source();
            MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
            int fed = 0;
            boolean eos = false;
            while (!eos) {
                checkDeadline();
                if (fed < frames) {
                    int i = enc.dequeueInputBuffer(POLL_US);
                    if (i >= 0) {
                        Image img = enc.getInputImage(i);
                        if (img == null) {
                            throw new IllegalStateException();
                        }
                        drawSource(img, fed);
                        long pts = (long) fed * 1_000_000L / fps;
                        fed++;
                        enc.queueInputBuffer(i, 0, sw * sh * 3 / 2, pts,
                                fed == frames ? MediaCodec.BUFFER_FLAG_END_OF_STREAM : 0);
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
            dec = create(false, sw, sh);
            enc = create(true, tw, th);
            MediaFormat df = src.format;
            df.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible);
            dec.configure(df, null, null, 0);
            dec.start();
            enc.configure(encoderFormat(tw, th, 800_000), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
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
                                enc.queueInputBuffer(encIn, 0, tw * th * 3 / 2, lastPts, 0);
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
    private void drawSource(Image img, int index) {
        int shift = index * 16;
        Image.Plane[] planes = img.getPlanes();
        for (int p = 0; p < 3; p++) {
            int pw = p == 0 ? sw : sw / 2, ph = p == 0 ? sh : sh / 2;
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

    // 各面を縦横 2:1 で間引く。両側の pixelStride / rowStride とクロップ原点を尊重する。
    // 復号器の Image 全体寸法（例: 16 の倍数に切り上げた 1280x736）ではなく、getCropRect() の有効領域で検証する。
    private void scale(Image src, Image dst) {
        android.graphics.Rect crop = src.getCropRect();
        if (crop.width() != sw || crop.height() != sh) {
            throw new IllegalStateException();
        }
        Image.Plane[] s = src.getPlanes(), d = dst.getPlanes();
        for (int p = 0; p < 3; p++) {
            int dw = p == 0 ? tw : tw / 2, dh = p == 0 ? th : th / 2;
            // 色差面はクロップ原点も 2:1 で縮む。
            int ox = p == 0 ? crop.left : crop.left / 2, oy = p == 0 ? crop.top : crop.top / 2;
            ByteBuffer sb = s[p].getBuffer(), db = d[p].getBuffer();
            int sr = s[p].getRowStride(), sp = s[p].getPixelStride();
            int dr = d[p].getRowStride(), dp = d[p].getPixelStride();
            for (int y = 0; y < dh; y++) {
                for (int x = 0; x < dw; x++) {
                    db.put(y * dr + x * dp, sb.get((oy + 2 * y) * sr + (ox + 2 * x) * sp));
                }
            }
        }
    }

    private static byte[] code(int c) {
        return new byte[] {(byte) c};
    }
}
