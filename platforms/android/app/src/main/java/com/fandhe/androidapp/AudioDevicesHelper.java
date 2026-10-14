package com.fandhe.androidapp;

import android.content.Context;
import android.media.AudioDeviceInfo;
import android.media.AudioManager;

// 音声デバイス一覧画面（Issue #1332、親 #1136）の Android 実装ヘルパー。MainActivity.audioDevicesList()
// から委譲される。Rust の `device/android_audio_devices.rs` が egui スレッドからボタン押下ごとに
// JNI（`()Ljava/lang/String;`）で 1 回呼ぶ同期 getter で、Java -> Rust の native コールバックは
// 無い（no-op スタブ不要）。
//
// 戻り値は「入力区画」+ U+001D（GS）+「出力区画」。Rust 側 `from_android` と対で決める:
//   区画が空文字        その方向は取得できなかった（getDevices が例外）。Rust で Unavailable
//   区画が件数で始まる  <件数（10 進）> に、件数分の「U+001F + 名前」が続く。0 件は "0" だけ。
//                       名前が無い項目は空文字（件数には数える）
// 名前からは U+001F・U+001D を除いてから詰める。全体の失敗（RuntimeException・AudioManager が
// null）は null。
//
// 名前は AudioDeviceInfo.getProductName() だけ。getAddress()（Bluetooth の MAC を含む）と
// getId() は読まない。種別・既定の印も持たない。getDevices は API 23 以降（minSdk 24）、権限は
// 不要（RECORD_AUDIO は宣言しない）。Activity は握らず ApplicationContext だけを使い、要求 id・
// 保留状態も持たないので、Activity 再生成で破棄すべきものは無い。
// 守る範囲: 0 件と取得不可の区別。守らない範囲: 読み取り後の接続・取り外しの反映（再取得まで）。
final class AudioDevicesHelper {
    private static final char SEP = '\u001F';
    private static final char GROUP = '\u001D';

    private AudioDevicesHelper() {}

    static String list(Context context) {
        try {
            AudioManager am = (AudioManager) context.getSystemService(Context.AUDIO_SERVICE);
            if (am == null) {
                return null;
            }
            return section(am, AudioManager.GET_DEVICES_INPUTS) + GROUP
                    + section(am, AudioManager.GET_DEVICES_OUTPUTS);
        } catch (RuntimeException e) {
            return null;
        }
    }

    // 1 方向分。失敗はこの方向だけ空区画にする。
    private static String section(AudioManager am, int flag) {
        try {
            AudioDeviceInfo[] devices = am.getDevices(flag);
            StringBuilder sb = new StringBuilder(Integer.toString(devices.length));
            for (AudioDeviceInfo d : devices) {
                CharSequence name = d.getProductName();
                sb.append(SEP);
                if (name != null) {
                    sb.append(name.toString().replace(String.valueOf(SEP), "").replace(String.valueOf(GROUP), ""));
                }
            }
            return sb.toString();
        } catch (RuntimeException e) {
            return "";
        }
    }
}
