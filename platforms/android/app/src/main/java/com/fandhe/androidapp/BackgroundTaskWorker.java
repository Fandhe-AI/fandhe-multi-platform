// Issue #1046: WorkManager の `OneTimeWorkRequest` から起動される Worker。
// `MainActivity.backgroundTaskSchedule()` が `enqueueUniqueWork(...,
// ExistingWorkPolicy.REPLACE, ...)` で予約する唯一の Worker クラス。
//
// プロセスが冷えた状態（Activity も fandhe_egui_proxy の cdylib も動いていない）で
// 起動されうるため、`android_main` 前提の JNI 経路
// （`crate::android::android_app()`）は使えない。本クラスは Rust 側
// `fandhe_device::background_task::RECORD_KEY`/`encode_record` と同じ形式・保存先
// （`getFilesDir()` 直下）へ直接書き込む
// （crates/fandhe-egui-proxy/src/device/android_background_task.rs モジュール doc
// 「記録を書く主体」参照）。
package com.fandhe.androidapp;

import android.content.Context;
import androidx.annotation.NonNull;
import androidx.work.Worker;
import androidx.work.WorkerParameters;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import org.json.JSONException;
import org.json.JSONObject;

public final class BackgroundTaskWorker extends Worker {
    // Rust 側 `fandhe_device::background_task::RECORD_KEY`
    // （"background-task.last-run"）を `fandhe_device::storage` の非公開
    // `encode_key`（各バイトを小文字 16 進 2 桁）で符号化した固定値（SSOT は
    // Rust 側。`android_background_task.rs` の `record_file_name_matches_fs_storage_encoding`
    // テストが同じ値を固定する）。
    private static final String RECORD_FILE_NAME =
            "6261636b67726f756e642d7461736b2e6c6173742d72756e";

    // 一時ファイルの置き場所。`fandhe_device::storage::FsStorage` の `TMP_SUBDIR` と
    // 同じ名前空間分離の考え方（通常ファイルと衝突しないサブディレクトリに置く）。
    private static final String TMP_SUBDIR = ".fandhe-tmp";

    // `fandhe_device::background_task::encode_record` が書く JSON の `version` 値。
    private static final int RECORD_VERSION = 1;

    // 既存記録の時刻比較から rename までを、同じプロセス内で並行する Worker 同士で
    // 直列化する（REPLACE で置き換え前後の Worker が重なったとき、遅れて保存を終えた
    // 古い方が新しい記録を上書きしないようにする。PR #1052 codex・Bugbot 指摘、
    // `fandhe_device::background_task` 契約「実行記録は時刻の新しい方を残す」）。
    private static final Object RECORD_LOCK = new Object();

    public BackgroundTaskWorker(@NonNull Context context, @NonNull WorkerParameters params) {
        super(context, params);
    }

    // 保存に失敗したら `Result.retry()` で WorkManager の再試行（バックオフ付き）に
    // 任せる。`failure()` で終えると記録が無いまま予約も消え、画面は「予約中」のまま
    // 食い違う（PR #1052 codex 指摘）。再試行中は実際に未完了なので「予約中」と一致する。
    @NonNull
    @Override
    public Result doWork() {
        // 書き込み開始前に停止済みなら記録を残さない（モジュール doc
        // 「`RunResult::Expired` は使わない」。停止後の再試行は `WorkManager` に委ねる）。
        if (isStopped()) {
            return Result.success();
        }

        long ranAtUnixMs = System.currentTimeMillis();
        File tmpDir = new File(getApplicationContext().getFilesDir(), TMP_SUBDIR);
        File tmpFile = null;
        try {
            JSONObject json = new JSONObject();
            json.put("version", RECORD_VERSION);
            json.put("ranAtUnixMs", ranAtUnixMs);
            json.put("result", "completed");
            byte[] bytes = json.toString().getBytes(StandardCharsets.UTF_8);

            if (!tmpDir.exists() && !tmpDir.mkdirs() && !tmpDir.exists()) {
                return Result.retry();
            }
            tmpFile = File.createTempFile("record", ".tmp", tmpDir);
            try (FileOutputStream out = new FileOutputStream(tmpFile)) {
                out.write(bytes);
            }

            File target = new File(getApplicationContext().getFilesDir(), RECORD_FILE_NAME);
            synchronized (RECORD_LOCK) {
                if (target.exists()) {
                    // 読めない（I/O 失敗）なら既存の新しい記録を確認できないので失敗にする。
                    // 中身が壊れている場合だけは上書きして回復する
                    // （`write_record_if_newer` と同じ規則）。
                    // `java.nio.file.Files` は API 26 から（minSdk 24 のため使わない）。
                    byte[] existing;
                    try (java.io.FileInputStream in = new java.io.FileInputStream(target);
                            java.io.ByteArrayOutputStream buf =
                                    new java.io.ByteArrayOutputStream()) {
                        byte[] chunk = new byte[256];
                        int n;
                        while ((n = in.read(chunk)) != -1) {
                            buf.write(chunk, 0, n);
                        }
                        existing = buf.toByteArray();
                    }
                    Long existingAt = decodeRanAt(existing);
                    // Rust の `decode_record` で読める記録のときだけ比べる。読めない
                    // （壊れた・未対応の形式）記録は上書きして回復する（PR #1052 codex 指摘）。
                    if (existingAt != null && existingAt > ranAtUnixMs) {
                        return Result.success();
                    }
                }
                if (!tmpFile.renameTo(target)) {
                    return Result.retry();
                }
            }
            // rename 成功。以後の finally での削除対象から外す（既に target へ
            // 移動済みのため、同名の tmpFile はもう存在しない）。
            tmpFile = null;
            return Result.success();
        } catch (JSONException | IOException | RuntimeException e) {
            return Result.retry();
        } finally {
            // 失敗時（JSON 構築失敗・書き込み失敗・rename 失敗）、確保済みの
            // 一時ファイルを残さない（device-bridge-contracts.md「9. 失敗時の
            // リソース解放」）。
            if (tmpFile != null) {
                //noinspection ResultOfMethodCallIgnored
                tmpFile.delete();
            }
        }
    }

    // `fandhe_device::background_task::decode_record` と同じ検証（version が
    // RECORD_VERSION、ranAtUnixMs が 0 以上の整数、result が completed / expired）を
    // 通った記録の時刻を返す。通らなければ null（読めない記録として扱う）。
    private static Long decodeRanAt(byte[] bytes) {
        try {
            JSONObject json = new JSONObject(new String(bytes, StandardCharsets.UTF_8));
            // `optInt` は文字列の数値も受け入れるため、JSON の整数型であることを確かめる
            // （Rust の `decode_record` と同じ型規則。PR #1052 codex 指摘）。
            Object version = json.opt("version");
            if (!(version instanceof Integer || version instanceof Long)
                    || ((Number) version).longValue() != RECORD_VERSION) {
                return null;
            }
            if (!(json.opt("result") instanceof String)) {
                return null;
            }
            Object at = json.opt("ranAtUnixMs");
            if (!(at instanceof Integer || at instanceof Long)) {
                return null;
            }
            long ranAt = ((Number) at).longValue();
            if (ranAt < 0) {
                return null;
            }
            String result = json.optString("result", "");
            if (!result.equals("completed") && !result.equals("expired")) {
                return null;
            }
            return ranAt;
        } catch (JSONException e) {
            return null;
        }
    }
}
