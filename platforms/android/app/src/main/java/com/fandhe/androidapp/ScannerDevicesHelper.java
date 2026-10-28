package com.fandhe.androidapp;

import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.hardware.usb.UsbConstants;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbInterface;
import android.hardware.usb.UsbManager;
import android.mtp.MtpConstants;
import android.mtp.MtpDevice;
import android.mtp.MtpObjectInfo;
import android.net.Uri;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;

import androidx.activity.ComponentActivity;
import androidx.core.content.ContextCompat;

import java.io.ByteArrayOutputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicInteger;

// 接続したスキャナー・カメラ機器（USB の PTP / MTP）からの取り込み（Issue #983、親 #192）の
// Android 実装ヘルパー。MainActivity から生成・委譲される。Rust の `device/android_scanner_devices.rs` が
// `MainActivity.requestScannerDevicesImport` / `cancelScannerDevicesImport` を JNI で呼び、
// 結果を `nativeOnScannerDevicesResult` で受けて契約（`fandhe_device::scanner_devices`）の結果へ写像する。
//
// 流れ: 探索窓の間 `getDeviceList` を読み直して Still Image の機器を探す → `hasPermission` が偽なら
// `requestPermission`（応答の extras は信じず `hasPermission` を読み直す）→ IO スレッドで
// `MtpDevice` を開き、作成日時が最新の画像 1 枚を読み、長辺 2048 画素以下の JPEG にして返す。
//
// スレッド: 状態の更新は main looper 1 本、MTP の I/O は static の単一 executor 1 本。
// 結果の確定は資源（MtpDevice・接続・レシーバー）を解放した後に `finish` で 1 回だけ行う
// （`RequestTracker.complete` が id 照合の first-wins）。期限は Rust が持ち、切れたら `cancel` が
// 呼ばれる。IO 中なら解放が済んでから CODE_CANCELLED で確定する。
// ライフサイクル: `onPause` では中断しない（許可ダイアログが pause を起こすため）。`onDestroy` は
// INTERRUPTED で確定する。再生成後の新インスタンスは空の tracker から始まり、旧結果は捨てられる。
// 機器が外れたことは broadcast ではなく `getDeviceList` の読み直しで判定する（OS の状態を正にする）。
// 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止・資源の解放。守らない範囲: 探索窓の後に現れた機器、
// 複数台のうち先頭以外、他アプリが同じ機器を同時に開いていることによる失敗（FAILED）、期限の後に
// ダイアログで与えられた許可（OS に残る）。
// 機器名・シリアル・画像・例外の文言・id はログに出さない。
final class ScannerDevicesHelper {
    // 結果コード。Rust 側 `android_scanner_devices.rs::decode` が同じ値で写像する（SSOT は本クラス）。
    static final int CODE_OK = 0;
    static final int CODE_UNSUPPORTED = 1;
    static final int CODE_NO_DEVICE = 2;
    static final int CODE_NO_IMAGE = 3;
    static final int CODE_PERMISSION_DENIED = 4;
    static final int CODE_INTERRUPTED = 5;
    static final int CODE_TOO_LARGE = 6;
    static final int CODE_UNDECODABLE = 7;
    static final int CODE_FAILED = 8;
    static final int CODE_CANCELLED = 9;

    private static final String ACTION_PERMISSION =
            "com.fandhe.androidapp.SCANNER_DEVICES_USB_PERMISSION";
    private static final long POLL_MS = 250;
    // 元ファイルの上限（macOS の kFandheMaxSourceBytes と同値）。
    private static final long MAX_SOURCE_BYTES = 64L * 1024 * 1024;
    private static final int MAX_SIDE = 2048;
    private static final int JPEG_QUALITY = 85;

    // 同じ機器を二重に開かないよう、Activity 再生成を跨いで単一の executor に直列化する。
    private static final ExecutorService IO = Executors.newSingleThreadExecutor();

    private static native void nativeOnScannerDevicesResult(
            long requestId, int code, int found, byte[] jpeg);

    // 要求 1 件の状態。main スレッドでだけ触る（abort は IO と共有）。
    private static final class Req {
        final long id;
        final int maxBytes;
        final long deadline;
        final AtomicInteger abort = new AtomicInteger(0);
        UsbDevice device;
        int found;
        boolean io;
        Runnable poller;
        BroadcastReceiver receiver;

        Req(long id, long windowMs, int maxBytes) {
            this.id = id;
            this.maxBytes = maxBytes;
            this.deadline = SystemClock.uptimeMillis() + windowMs;
        }
    }

    // IO の戻り値。
    private static final class Result {
        final int code;
        final byte[] jpeg;

        Result(int code, byte[] jpeg) {
            this.code = code;
            this.jpeg = jpeg;
        }
    }

    private final ComponentActivity activity;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final RequestTracker request = new RequestTracker();
    private Req cur;
    // onDestroy 済みなら、post 済みの begin も開始せず INTERRUPTED で確定する。
    private boolean destroyed;

    ScannerDevicesHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    // Rust のワーカーから JNI で呼ばれる。main へ post するだけ。
    void start(long id, long windowMs, int maxBytes) {
        request.track(id);
        handler.post(() -> begin(new Req(id, windowMs, maxBytes)));
    }

    // 期限切れで Rust のワーカーが同期で呼ぶ。main へ post するだけ。
    void cancel(long id) {
        handler.post(() -> {
            Req r = cur;
            if (r != null && r.id == id) {
                abortReq(r, CODE_CANCELLED);
            }
        });
    }

    // MainActivity.onDestroy から UI スレッドで呼ぶ。
    void onDestroy() {
        destroyed = true;
        Req r = cur;
        if (r != null) {
            abortReq(r, CODE_INTERRUPTED);
        }
    }

    // 完了は必ずここを通す（first-wins。差し替え済み・破棄済みの旧要求は黙って捨てる）。
    private void finish(long id, int code, int found, byte[] jpeg) {
        if (request.complete(id)) {
            nativeOnScannerDevicesResult(id, code, found, jpeg);
        }
    }

    // main スレッド。IO 前の段階は解放済みなのでその場で確定し、IO 中は解放後に IO 側が確定する。
    private void abortReq(Req r, int code) {
        stopWatchers(r);
        if (r.io) {
            r.abort.compareAndSet(0, code);
        } else {
            end(r, code, null);
        }
    }

    private void stopWatchers(Req r) {
        if (r.poller != null) {
            handler.removeCallbacks(r.poller);
            r.poller = null;
        }
        if (r.receiver != null) {
            try {
                activity.unregisterReceiver(r.receiver);
            } catch (IllegalArgumentException e) {
                // 解除済みは無視する。
            }
            r.receiver = null;
        }
    }

    // main スレッド。IO を経ない確定。
    private void end(Req r, int code, byte[] jpeg) {
        stopWatchers(r);
        if (cur == r) {
            cur = null;
        }
        finish(r.id, code, r.found, jpeg);
    }

    private void begin(Req r) {
        if (destroyed) {
            end(r, CODE_INTERRUPTED, null);
            return;
        }
        Req prev = cur;
        if (prev != null) {
            // 旧要求の結果は tracker の id 不一致で捨てられる。IO 中なら止めるだけ。
            stopWatchers(prev);
            prev.abort.compareAndSet(0, CODE_INTERRUPTED);
        }
        cur = r;
        try {
            UsbManager um = (UsbManager) activity.getSystemService(Context.USB_SERVICE);
            if (um == null || !activity.getPackageManager()
                    .hasSystemFeature(PackageManager.FEATURE_USB_HOST)) {
                end(r, CODE_UNSUPPORTED, null);
                return;
            }
            r.poller = new Runnable() {
                @Override
                public void run() {
                    if (cur != r) {
                        return;
                    }
                    try {
                        List<UsbDevice> found = stillImageDevices(um);
                        if (!found.isEmpty()) {
                            r.poller = null;
                            r.found = found.size();
                            r.device = found.get(0);
                            havePicked(r, um);
                        } else if (SystemClock.uptimeMillis() >= r.deadline) {
                            end(r, CODE_NO_DEVICE, null);
                        } else {
                            handler.postDelayed(this, POLL_MS);
                        }
                    } catch (RuntimeException e) {
                        end(r, CODE_FAILED, null);
                    }
                }
            };
            r.poller.run();
        } catch (RuntimeException e) {
            end(r, CODE_FAILED, null);
        }
    }

    // 名前の辞書順で並べ、先頭を取り込み対象にする。
    private static List<UsbDevice> stillImageDevices(UsbManager um) {
        List<UsbDevice> out = new ArrayList<>();
        for (UsbDevice d : um.getDeviceList().values()) {
            if (isStillImage(d)) {
                out.add(d);
            }
        }
        Collections.sort(out, (a, b) -> a.getDeviceName().compareTo(b.getDeviceName()));
        return out;
    }

    private static boolean isStillImage(UsbDevice d) {
        if (d.getDeviceClass() == UsbConstants.USB_CLASS_STILL_IMAGE) {
            return true;
        }
        for (int i = 0; i < d.getInterfaceCount(); i++) {
            UsbInterface itf = d.getInterface(i);
            if (itf.getInterfaceClass() == UsbConstants.USB_CLASS_STILL_IMAGE) {
                return true;
            }
        }
        return false;
    }

    private static Uri permissionData(long id) {
        return Uri.parse("fandhe-scanner-devices://permission/" + id);
    }

    private static boolean present(UsbManager um, UsbDevice d) {
        return um.getDeviceList().get(d.getDeviceName()) != null;
    }

    private void havePicked(Req r, UsbManager um) {
        if (um.hasPermission(r.device)) {
            startIo(r, um);
            return;
        }
        BroadcastReceiver rcv = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                // 要求 id を data に埋めた通知だけを処理する（期限切れ旧要求の遅延通知を排除する）。
                if (cur != r || !ACTION_PERMISSION.equals(intent.getAction())
                        || !permissionData(r.id).equals(intent.getData())) {
                    return;
                }
                stopWatchers(r);
                try {
                    // extras は信じず OS の状態を読み直す。
                    if (!present(um, r.device)) {
                        end(r, CODE_INTERRUPTED, null);
                    } else if (!um.hasPermission(r.device)) {
                        end(r, CODE_PERMISSION_DENIED, null);
                    } else {
                        startIo(r, um);
                    }
                } catch (RuntimeException e) {
                    end(r, CODE_FAILED, null);
                }
            }
        };
        // 参照は登録より先に保存する（登録直後の通知を取りこぼさない）。
        r.receiver = rcv;
        try {
            ContextCompat.registerReceiver(activity, rcv, new IntentFilter(ACTION_PERMISSION),
                    null, handler, ContextCompat.RECEIVER_NOT_EXPORTED);
            // data に要求 id を含め、要求ごとに別の PendingIntent にする（filterEquals は data を比較する）。
            Intent intent = new Intent(ACTION_PERMISSION, permissionData(r.id))
                    .setPackage(activity.getPackageName());
            PendingIntent pi = PendingIntent.getBroadcast(
                    activity, (int) r.id, intent, PendingIntent.FLAG_IMMUTABLE);
            um.requestPermission(r.device, pi);
        } catch (RuntimeException e) {
            end(r, CODE_FAILED, null);
            return;
        }
        // 機器を外すと許可の broadcast が届かない場合に備え、存在を読み直し続ける。
        r.poller = new Runnable() {
            @Override
            public void run() {
                if (cur != r) {
                    return;
                }
                try {
                    if (!present(um, r.device)) {
                        end(r, CODE_INTERRUPTED, null);
                    } else {
                        handler.postDelayed(this, POLL_MS);
                    }
                } catch (RuntimeException e) {
                    end(r, CODE_FAILED, null);
                }
            }
        };
        handler.postDelayed(r.poller, POLL_MS);
    }

    private void startIo(Req r, UsbManager um) {
        r.io = true;
        UsbDevice dev = r.device;
        IO.execute(() -> runIo(r, um, dev));
    }

    // IO スレッド。資源を解放してから結果を確定する。
    private void runIo(Req r, UsbManager um, UsbDevice dev) {
        Result res = new Result(CODE_FAILED, null);
        UsbDeviceConnection conn = null;
        MtpDevice mtp = null;
        try {
            if (r.abort.get() == 0) {
                conn = um.openDevice(dev);
                if (conn != null) {
                    mtp = new MtpDevice(dev);
                    if (mtp.open(conn)) {
                        res = readLatest(r, mtp, um, dev);
                    }
                }
            }
        } catch (SecurityException e) {
            res = new Result(CODE_PERMISSION_DENIED, null);
        } catch (RuntimeException e) {
            res = new Result(CODE_FAILED, null);
        } finally {
            if (mtp != null) {
                try {
                    mtp.close();
                } catch (RuntimeException e) {
                    // 解放の失敗は結果を変えない。
                }
            }
            if (conn != null) {
                try {
                    conn.close();
                } catch (RuntimeException e) {
                    // 同上（MtpDevice.close が閉じ済みでも二重 close は無害）。
                }
            }
        }
        // どの I/O 経路の失敗でも、機器が外れていれば切断として報告する（資源解放後に OS 一覧を読み直す）。
        // 成功も完了前に確認する。確認自体が失敗したら成功を返さない。
        if (res.code == CODE_FAILED || res.code == CODE_OK) {
            try {
                if (!present(um, dev)) {
                    res = new Result(CODE_INTERRUPTED, null);
                }
            } catch (RuntimeException e) {
                if (res.code == CODE_OK) {
                    res = new Result(CODE_FAILED, null);
                }
            }
        }
        int abort = r.abort.get();
        if (abort != 0) {
            res = new Result(abort, null);
        }
        finish(r.id, res.code, r.found, res.jpeg);
        handler.post(() -> {
            if (cur == r) {
                cur = null;
            }
        });
    }

    private static boolean isImageFormat(int f) {
        return f == MtpConstants.FORMAT_EXIF_JPEG || f == MtpConstants.FORMAT_JFIF
                || f == MtpConstants.FORMAT_PNG || f == MtpConstants.FORMAT_GIF
                || f == MtpConstants.FORMAT_BMP;
    }

    // 欠けた要素を飛ばして成功を作らない。日時が最新のものを選ぶ。
    private Result readLatest(Req r, MtpDevice mtp, UsbManager um, UsbDevice dev) {
        int[] sids = mtp.getStorageIds();
        if (sids == null || sids.length == 0) {
            // ロック中と区別できない。
            return new Result(CODE_FAILED, null);
        }
        int bestHandle = -1;
        long bestDate = -1;
        long bestSize = 0;
        for (int sid : sids) {
            int[] handles = mtp.getObjectHandles(sid, 0, 0);
            if (handles == null) {
                return new Result(CODE_FAILED, null);
            }
            for (int h : handles) {
                if (r.abort.get() != 0) {
                    return new Result(CODE_FAILED, null);
                }
                MtpObjectInfo info = mtp.getObjectInfo(h);
                if (info == null) {
                    return new Result(CODE_FAILED, null);
                }
                if (!isImageFormat(info.getFormat())) {
                    continue;
                }
                long date = info.getDateCreated();
                if (date <= 0) {
                    return new Result(CODE_FAILED, null);
                }
                if (date > bestDate || (date == bestDate && h > bestHandle)) {
                    bestDate = date;
                    bestHandle = h;
                    bestSize = info.getCompressedSizeLong();
                }
            }
        }
        if (bestHandle < 0) {
            return new Result(CODE_NO_IMAGE, null);
        }
        if (bestSize <= 0) {
            return new Result(CODE_UNDECODABLE, null);
        }
        if (bestSize > MAX_SOURCE_BYTES) {
            return new Result(CODE_TOO_LARGE, null);
        }
        byte[] raw;
        try {
            raw = mtp.getObject(bestHandle, (int) bestSize);
        } catch (OutOfMemoryError e) {
            return new Result(CODE_TOO_LARGE, null);
        }
        if (raw == null) {
            return new Result(present(um, dev) ? CODE_FAILED : CODE_INTERRUPTED, null);
        }
        return toJpeg(raw, r.maxBytes);
    }

    // 長辺 MAX_SIDE 以下の JPEG にする。復号不可は UNDECODABLE、上限超過・OOM は TOO_LARGE。
    private static Result toJpeg(byte[] raw, int maxBytes) {
        Bitmap bmp = null;
        Bitmap scaled = null;
        try {
            BitmapFactory.Options bounds = new BitmapFactory.Options();
            bounds.inJustDecodeBounds = true;
            BitmapFactory.decodeByteArray(raw, 0, raw.length, bounds);
            if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
                return new Result(CODE_UNDECODABLE, null);
            }
            int longSide = Math.max(bounds.outWidth, bounds.outHeight);
            int sample = 1;
            while (longSide / (sample * 2) >= MAX_SIDE) {
                sample *= 2;
            }
            BitmapFactory.Options opts = new BitmapFactory.Options();
            opts.inSampleSize = sample;
            bmp = BitmapFactory.decodeByteArray(raw, 0, raw.length, opts);
            if (bmp == null) {
                return new Result(CODE_UNDECODABLE, null);
            }
            scaled = bmp;
            int side = Math.max(bmp.getWidth(), bmp.getHeight());
            if (side > MAX_SIDE) {
                float k = (float) MAX_SIDE / side;
                scaled = Bitmap.createScaledBitmap(bmp,
                        Math.max(1, Math.round(bmp.getWidth() * k)),
                        Math.max(1, Math.round(bmp.getHeight() * k)), true);
            }
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            if (!scaled.compress(Bitmap.CompressFormat.JPEG, JPEG_QUALITY, out)) {
                return new Result(CODE_FAILED, null);
            }
            if (out.size() > maxBytes) {
                return new Result(CODE_TOO_LARGE, null);
            }
            return new Result(CODE_OK, out.toByteArray());
        } catch (OutOfMemoryError e) {
            return new Result(CODE_TOO_LARGE, null);
        } finally {
            if (scaled != null && scaled != bmp) {
                scaled.recycle();
            }
            if (bmp != null) {
                bmp.recycle();
            }
        }
    }
}
