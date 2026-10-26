package com.fandhe.androidapp;

import android.annotation.SuppressLint;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.media.AudioAttributes;
import android.media.AudioFocusRequest;
import android.media.AudioManager;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;

import androidx.activity.ComponentActivity;

// 音声の割り込み・セッション種別（Issue #842、親 #172）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_audio_session.rs` が `MainActivity.audioSessionStart` / `Stop` / `Read` を JNI で呼び、
// 出来事を `nativeOnAudioSessionEvent` で受ける）。
// フォーカスは AudioFocusRequest（API 26+）で要求し、フォーカス変化と ACTION_AUDIO_BECOMING_NOISY を受ける。
// 選べる種別は StopOthers（AUDIOFOCUS_GAIN）と DuckOthers（GAIN_TRANSIENT_MAY_DUCK）だけ。MixWithOthers は
// 「フォーカスを要求しない」ことにあたり要求の API が無いので、Rust 側が Java を呼ぶ前に拒否する。
//
// コードの SSOT はここで、Rust 側 `android_audio_session.rs` が同じ値で写像する。
//   kind: 0 = StopOthers / 1 = DuckOthers
//   start の戻り値: 1 = GRANTED / 0 = FAILED / 2 = DELAYED（遅延獲得を要求しないので来ない想定）/
//                   -1 = API 26 未満 / -2 = 例外・破棄済み・未知の kind /
//                   -3 = 失敗の後始末（フォーカス・レシーバーの解除）も失敗＝元に戻せていない
//   stop の戻り値: 0 = 解除済み（現在の id でなければ何もせず 0）/ 1 = 解除の失敗
//                  （onDestroy の解除が失敗していた id の stop も 1）
//   native の code: focusChange の生値（1 / -1 / -2 / -3）または 1000（BECOMING_NOISY）
//   read の戻り値: -1 = 失敗。bit 0-7 = AudioManager.getMode()（範囲外は 0xFF = 不明）、bit 8 = isMusicActive()
//
// スレッドとロック: 状態（current / request / receiver / destroyed）は `lock` の内側だけで読み書きする。
// リスナーとレシーバーは main looper で動き、native 呼び出しはロックの外で行う（古い id は生成時に捕捉した id と
// current の照合で捨てる。id は Rust 側で単調増加）。ロック取得順は Java -> Rust の一方向。
// read は binder の getter で Looper の制約が無く、egui スレッドから同期で呼べる。
//
// ライフサイクル: onPause / onStop では止めない（契約: 背景への遷移では止めない）。onDestroy（Activity 再生成を
// 含む）はフォーカスとレシーバーを解除し、停止を失敗として送らない。新インスタンスは購読を持たない状態から始まる。
// 新しい権限は不要。id・例外の内容はログへ出さない。
final class AudioSessionHelper {
    static final int START_GRANTED = 1;
    static final int START_FAILED = 0;
    static final int START_DELAYED = 2;
    static final int START_UNSUPPORTED = -1;
    static final int START_ERROR = -2;
    static final int START_UNRESTORED = -3;

    static final int KIND_STOP_OTHERS = 0;
    static final int KIND_DUCK_OTHERS = 1;

    static final int EVENT_BECOMING_NOISY = 1000;

    private static final long NONE = 0L;
    private static final int MODE_UNKNOWN = 0xFF;

    private static native void nativeOnAudioSessionEvent(long id, int code);

    private final ComponentActivity activity;
    private final Object lock = new Object();
    private final Handler handler = new Handler(Looper.getMainLooper());
    private long current = NONE;
    private AudioFocusRequest request;
    private BroadcastReceiver receiver;
    private boolean destroyed;
    // onDestroy の解除に失敗した購読の id。その id の stop は解除失敗として返す。
    private long lostId = NONE;

    AudioSessionHelper(ComponentActivity activity) {
        this.activity = activity;
    }

    private AudioManager audioManager() {
        return activity.getApplicationContext().getSystemService(AudioManager.class);
    }

    @SuppressLint("UnspecifiedRegisterReceiverFlag")
    int start(final long id, int kind) {
        synchronized (lock) {
            if (destroyed) {
                return START_ERROR;
            }
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
                return START_UNSUPPORTED;
            }
            int gain;
            if (kind == KIND_STOP_OTHERS) {
                gain = AudioManager.AUDIOFOCUS_GAIN;
            } else if (kind == KIND_DUCK_OTHERS) {
                gain = AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK;
            } else {
                return START_ERROR;
            }
            // 古い購読を残さないための片付け（通常は購読が 1 本なので起きない）。
            if (!releaseLocked()) {
                return START_UNRESTORED;
            }
            AudioManager am = audioManager();
            if (am == null) {
                return START_ERROR;
            }
            BroadcastReceiver r = new BroadcastReceiver() {
                @Override
                public void onReceive(Context context, Intent intent) {
                    if (intent != null
                            && AudioManager.ACTION_AUDIO_BECOMING_NOISY.equals(intent.getAction())) {
                        dispatch(id, EVENT_BECOMING_NOISY);
                    }
                }
            };
            int result;
            try {
                // システムだけが送る保護されたブロードキャストなのでフラグは付けない。
                activity.getApplicationContext().registerReceiver(
                        r, new IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY), null, handler);
                receiver = r;
                AudioFocusRequest req = new AudioFocusRequest.Builder(gain)
                        .setAudioAttributes(new AudioAttributes.Builder()
                                .setUsage(AudioAttributes.USAGE_MEDIA)
                                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                                .build())
                        .setOnAudioFocusChangeListener(
                                new AudioManager.OnAudioFocusChangeListener() {
                                    @Override
                                    public void onAudioFocusChange(int focusChange) {
                                        dispatch(id, focusChange);
                                    }
                                }, handler)
                        .build();
                request = req;
                result = am.requestAudioFocus(req);
            } catch (RuntimeException e) {
                result = START_ERROR;
            }
            if (result == AudioManager.AUDIOFOCUS_REQUEST_GRANTED) {
                current = id;
                return START_GRANTED;
            }
            // GRANTED 以外は何も保持しない。abandon は DELAYED の取り消しを兼ねる（結果は無視）。
            if (!releaseLocked()) {
                return START_UNRESTORED;
            }
            if (result == AudioManager.AUDIOFOCUS_REQUEST_FAILED) {
                return START_FAILED;
            }
            return result == AudioManager.AUDIOFOCUS_REQUEST_DELAYED ? START_DELAYED : START_ERROR;
        }
    }

    // 現在の id と一致するときだけ解除する（再生成後の新インスタンスへ届く古い id の stop は何もしない）。
    int stop(long id) {
        synchronized (lock) {
            if (lostId != NONE && id == lostId) {
                lostId = NONE;
                return 1;
            }
            if (current == NONE || id != current) {
                return 0;
            }
            return releaseLocked() ? 0 : 1;
        }
    }

    void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            long id = current;
            if (!releaseLocked() && id != NONE) {
                lostId = id;
            }
        }
    }

    int read() {
        try {
            AudioManager am = audioManager();
            if (am == null) {
                return -1;
            }
            int mode = am.getMode();
            if (mode < 0 || mode >= MODE_UNKNOWN) {
                mode = MODE_UNKNOWN;
            }
            return mode | (am.isMusicActive() ? 1 << 8 : 0);
        } catch (RuntimeException e) {
            return -1;
        }
    }

    // 生成時に捕捉した id が現在の購読のときだけ Rust へ届ける。native はロックの外で呼ぶ。
    private void dispatch(long id, int code) {
        synchronized (lock) {
            if (current == NONE || current != id) {
                return;
            }
        }
        nativeOnAudioSessionEvent(id, code);
    }

    // lock を持った状態で呼ぶ。フォーカス解除とレシーバー解除を独立に試し、状態を空にする。
    // 両方成功（または保持なし）のとき true。
    private boolean releaseLocked() {
        AudioFocusRequest req = request;
        BroadcastReceiver r = receiver;
        request = null;
        receiver = null;
        current = NONE;
        boolean ok = true;
        if (req != null) {
            try {
                AudioManager am = audioManager();
                ok = am != null
                        && am.abandonAudioFocusRequest(req) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED;
            } catch (RuntimeException e) {
                ok = false;
            }
        }
        if (r != null) {
            try {
                activity.getApplicationContext().unregisterReceiver(r);
            } catch (RuntimeException e) {
                ok = false;
            }
        }
        return ok;
    }
}
