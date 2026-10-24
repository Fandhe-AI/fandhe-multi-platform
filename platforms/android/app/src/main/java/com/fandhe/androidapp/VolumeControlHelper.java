package com.fandhe.androidapp;

import android.app.Activity;
import android.content.Context;
import android.media.AudioManager;

// システム音量・消音状態の取得と変更（Issue #787、親 #166）の Android 実装ヘルパー。
// MainActivity から生成・委譲され、Rust の `device/android_volume_control.rs` が egui スレッドから
// `MainActivity.volumeControlRead` / `volumeControlAdjust` を JNI で同期呼び出しする。
//
// 結果コードの SSOT はこのクラスで、Rust 側は同じ値を写す。未知値は Rust 側で `Internal` になる。
// 対象はメディア音量（STREAM_MUSIC）だけ。着信音・通知音量・おやすみモードは扱わない。
// flags は 0 で、OS の音量 UI は出さない。音量固定の判定は Rust 側が変更前の読み取りで行う。
//
// AudioManager は呼び出しのたびに ApplicationContext から取る（Activity を握らず、
// フィールド初期化の時点で Context が未 attach でも壊れない）。binder の同期呼び出しだけなので
// UI スレッドへは post しない。要求 id も保留状態も持たないため、Activity 再生成で破棄する
// 保留中の要求はなく、古い結果が新しい要求を消費することもない（照合は Rust の RequestSlot）。
// 守らない範囲: 他アプリによる音量変更への追従（Rust 側が 1 秒ごとに読み直す）、
// STREAM_MUSIC の最小音量が 0 でない端末での表示上の差。例外の詳細はログへ出さない。
final class VolumeControlHelper {
    static final int ADJUST_OK = 0;
    static final int ADJUST_PERMISSION_DENIED = 1;
    static final int ADJUST_FAILED = 2;
    static final int ADJUST_INVALID = 3;

    private final Activity activity;

    VolumeControlHelper(Activity activity) {
        this.activity = activity;
    }

    private AudioManager audioManager() {
        Context app = activity.getApplicationContext();
        if (app == null) {
            return null;
        }
        Object svc = app.getSystemService(Context.AUDIO_SERVICE);
        return svc instanceof AudioManager ? (AudioManager) svc : null;
    }

    // {index, maxIndex, muted(0/1), fixed(0/1)}。読めないときは null（成功値へ畳まない）。
    int[] read() {
        try {
            AudioManager am = audioManager();
            if (am == null) {
                return null;
            }
            return new int[] {
                am.getStreamVolume(AudioManager.STREAM_MUSIC),
                am.getStreamMaxVolume(AudioManager.STREAM_MUSIC),
                am.isStreamMute(AudioManager.STREAM_MUSIC) ? 1 : 0,
                am.isVolumeFixed() ? 1 : 0,
            };
        } catch (RuntimeException e) {
            return null;
        }
    }

    // direction は 1（上げる）か -1（下げる）。
    int adjust(int direction) {
        int adj;
        if (direction == 1) {
            adj = AudioManager.ADJUST_RAISE;
        } else if (direction == -1) {
            adj = AudioManager.ADJUST_LOWER;
        } else {
            return ADJUST_INVALID;
        }
        try {
            AudioManager am = audioManager();
            if (am == null) {
                return ADJUST_FAILED;
            }
            am.adjustStreamVolume(AudioManager.STREAM_MUSIC, adj, 0);
            return ADJUST_OK;
        } catch (SecurityException e) {
            return ADJUST_PERMISSION_DENIED;
        } catch (RuntimeException e) {
            return ADJUST_FAILED;
        }
    }
}
