package com.fandhe.androidapp;

import android.media.AudioManager;
import android.media.ToneGenerator;

/**
 * システムの効果音（ビープ）の Android 実装（Issue #940、親 #183）。
 *
 * <p>呼び出し元: Rust の {@code device/android_system_sound.rs} が egui スレッドから
 * {@code MainActivity.systemSoundBeep()} を JNI で同期に呼ぶ。結果コード（{@code BEEP_*}）の
 * SSOT は本クラスで、Rust 側の {@code decode_beep} が同じ値を写す。
 *
 * <p>{@code ToneGenerator} は Looper を要さないため UI スレッドへ post せず、期限後に遅れて
 * 適用される経路も持たない。要求 id・保留状態も持たない（同期の一発呼び出し。世代の照合は
 * Rust の driver が担う）。Activity が再生成されると旧 helper は {@link #onDestroy()} で
 * 片付き、次の押下で新しい helper が作り直す。
 *
 * <p>守る範囲: 1 プロセス内の順序、{@code false}・例外を成功に畳まないこと、破棄後に資源を
 * 作らないこと。守らない範囲: 実際に聞こえたか（消音・音量 0）、連打時の置き換え、背景中の押下。
 */
final class SystemSoundHelper {
    /** {@code startTone} が true（OS に渡した）。 */
    static final int BEEP_REQUESTED = 0;
    /** {@code startTone} が false（OS は鳴らさなかった）。 */
    static final int BEEP_NOT_PLAYED = 1;
    /** 生成・呼び出しの例外、または破棄後。 */
    static final int BEEP_FAILED = 2;

    private final Object lock = new Object();
    private ToneGenerator tone;
    private boolean destroyed;

    /** ビープを 1 回要求する。egui スレッドから呼ばれ、{@link #onDestroy()} と {@code lock} で排他する。 */
    int beep() {
        synchronized (lock) {
            if (destroyed) {
                return BEEP_FAILED;
            }
            try {
                if (tone == null) {
                    // 生成に失敗したら tone は null のまま（失敗を保持せず次回作り直す）。
                    tone = new ToneGenerator(AudioManager.STREAM_NOTIFICATION, ToneGenerator.MAX_VOLUME);
                }
                // 長さは渡さない。TONE_PROP_BEEP の定義（35ms）で自ら止まる。
                return tone.startTone(ToneGenerator.TONE_PROP_BEEP) ? BEEP_REQUESTED : BEEP_NOT_PLAYED;
            } catch (RuntimeException e) {
                releaseLocked();
                return BEEP_FAILED;
            }
        }
    }

    /** Activity 破棄時に generator を解放し、以後の生成を拒否する。 */
    void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            releaseLocked();
        }
    }

    private void releaseLocked() {
        if (tone != null) {
            try {
                tone.release();
            } catch (RuntimeException ignored) {
                // 解放失敗は握りつぶす（保持し続けない）。
            }
            tone = null;
        }
    }
}
