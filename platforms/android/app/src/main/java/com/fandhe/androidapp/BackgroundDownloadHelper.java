package com.fandhe.androidapp;

import android.app.DownloadManager;
import android.content.Context;
import android.content.SharedPreferences;
import android.database.Cursor;
import android.net.Uri;

import java.io.File;

// OS 管理のバックグラウンドダウンロード（Issue #392、親 #100）の Android 実装ヘルパー。
// MainActivity から生成・委譲される（Rust の `device/android_background_download.rs` が
// ドライバのワーカースレッド（start/cancel）と egui スレッド（status）から
// `MainActivity.backgroundDownloadStart` / `Cancel` / `Status` を同期 JNI で呼ぶ）。
//
// 戻り値のコードの SSOT はここで、Rust 側が同じ値で解釈する。
//   start : 0=OK / 1=DownloadManager が無い / 2=開始失敗
//   cancel: 0=OK / 2=失敗（ファイルの削除失敗・remove の例外）
//   status: long[4] = {kind, received, total, reason}。読み出しの例外では null
//     kind: 0=id なし / 1=id はあるが行が無い（利用者が通知等から消した）/ 2=転送中
//           （PENDING・RUNNING・PAUSED）/ 3=成功 / 4=失敗
//     total: 不明なら -1。reason: 失敗時の COLUMN_REASON（HTTP の状態コード or ERROR_*）
//     成功時の received は列の値でなく保存先ファイルの実際の長さ（無ければ 0）。
//
// 状態の SSOT は OS の DownloadManager の行と、SharedPreferences の「現在の id」1 つ。
// Java 側に保留中の要求・コールバックは無い（完了ブロードキャストも受けない）。転送の持ち主は
// OS で、Activity の再生成・プロセス終了をまたいで続くのが機能の目的なので onDestroy で
// 取り消さない。照会は常に prefs の現在の id だけなので、古い転送の結果が新しい転送を
// 消費しない。置き換えは旧 id の remove と新 id の保存を同じロックの内側で順に行う。
//
// ロックは static。再生成中に旧・新 Activity のヘルパーが並んでも直列になる。ロックを持った
// まま呼ぶのは DownloadManager だけで main looper への待ちは無い（デッドロックしない）。
//
// 保存先: `setDestinationInExternalFilesDir`（アプリ専用の外部領域。API 24 以降でも保存の権限は
// 不要）の固定名 1 ファイル。サーバー由来のファイル名は使わない。URL・パスはログに出さない。
// 通知はシステムの既定表示のまま使う（追加の権限は足さない）。
final class BackgroundDownloadHelper {
    static final int OK = 0;
    static final int UNSUPPORTED = 1;
    static final int FAILED = 2;

    private static final int KIND_NONE = 0;
    private static final int KIND_GONE = 1;
    private static final int KIND_RUNNING = 2;
    private static final int KIND_SUCCESS = 3;
    private static final int KIND_FAILED = 4;

    // `fandhe_device::background_download::FILE_NAME` と同じ値。
    private static final String FILE_NAME = "background-download";
    private static final String PREFS = "background_download";
    private static final String KEY_ID = "id";
    private static final Object LOCK = new Object();

    private final Context context;

    BackgroundDownloadHelper(Context context) {
        this.context = context;
    }

    private DownloadManager manager() {
        return (DownloadManager) context.getSystemService(Context.DOWNLOAD_SERVICE);
    }

    private SharedPreferences prefs() {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    private File file() {
        File dir = context.getExternalFilesDir(null);
        return dir == null ? null : new File(dir, FILE_NAME);
    }

    int start(String url) {
        synchronized (LOCK) {
            try {
                DownloadManager dm = manager();
                if (dm == null) return UNSUPPORTED;
                File f = file();
                if (f == null) return FAILED;
                long old = prefs().getLong(KEY_ID, -1L);
                if (old >= 0) {
                    // 旧 id を先に消してから転送を破棄する。以降の失敗経路で消えた id が残り
                    // status() が「利用者が消した（Cancelled）」と誤判定しないようにする。
                    if (!prefs().edit().remove(KEY_ID).commit()) return FAILED;
                    dm.remove(old);
                }
                // 残っていると DownloadManager が別名で保存するため、必ず消す。
                if (f.exists() && !f.delete()) return FAILED;
                DownloadManager.Request req = new DownloadManager.Request(Uri.parse(url));
                req.setDestinationInExternalFilesDir(context, null, FILE_NAME);
                long id = dm.enqueue(req);
                if (!prefs().edit().putLong(KEY_ID, id).commit()) {
                    dm.remove(id); // id を追えない転送を残さない（KEY_ID は上で消去済みか未設定）
                    return FAILED;
                }
                return OK;
            } catch (RuntimeException e) {
                return FAILED;
            }
        }
    }

    int cancel() {
        synchronized (LOCK) {
            try {
                DownloadManager dm = manager();
                long id = prefs().getLong(KEY_ID, -1L);
                if (dm != null && id >= 0) dm.remove(id);
                if (id >= 0 && !prefs().edit().remove(KEY_ID).commit()) return FAILED;
                File f = file();
                if (f != null && f.exists() && !f.delete()) return FAILED;
                return OK;
            } catch (RuntimeException e) {
                return FAILED;
            }
        }
    }

    long[] status() {
        synchronized (LOCK) {
            try {
                long id = prefs().getLong(KEY_ID, -1L);
                if (id < 0) return new long[] {KIND_NONE, 0, -1, 0};
                DownloadManager dm = manager();
                if (dm == null) return null;
                try (Cursor c = dm.query(new DownloadManager.Query().setFilterById(id))) {
                    if (c == null) return null;
                    if (!c.moveToFirst()) return new long[] {KIND_GONE, 0, -1, 0};
                    int st = c.getInt(c.getColumnIndexOrThrow(DownloadManager.COLUMN_STATUS));
                    long got = c.getLong(
                            c.getColumnIndexOrThrow(DownloadManager.COLUMN_BYTES_DOWNLOADED_SO_FAR));
                    long total = c.getLong(c.getColumnIndexOrThrow(DownloadManager.COLUMN_TOTAL_SIZE_BYTES));
                    long reason = c.getLong(c.getColumnIndexOrThrow(DownloadManager.COLUMN_REASON));
                    if (total < 0) total = -1;
                    switch (st) {
                        case DownloadManager.STATUS_SUCCESSFUL: {
                            File f = file();
                            long len = (f != null && f.exists()) ? f.length() : 0L;
                            return new long[] {KIND_SUCCESS, len, total, 0};
                        }
                        case DownloadManager.STATUS_FAILED:
                            return new long[] {KIND_FAILED, got, total, reason};
                        default: // PENDING・RUNNING・PAUSED
                            return new long[] {KIND_RUNNING, got, total, 0};
                    }
                }
            } catch (RuntimeException e) {
                return null;
            }
        }
    }
}
