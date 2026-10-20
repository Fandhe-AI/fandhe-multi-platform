package com.fandhe.androidapp;

import android.content.Context;
import android.content.pm.PackageManager;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbManager;

// USB・HID 機器一覧画面（Issue #507、親 #112）の Android 実装ヘルパー。MainActivity.usbDeviceList()
// から委譲される。Rust の `device/android_usb_device.rs` が egui スレッドからボタン押下ごとに
// JNI（`()Ljava/lang/String;`）で 1 回呼ぶ同期 getter で、Java -> Rust の native コールバックは
// 無い（no-op スタブ不要）。
//
// 戻り値は Rust 側 `from_android` と対で決める:
//   null     全体の失敗（UsbManager が null・RuntimeException）。Rust で Internal
//   ""       USB ホスト非搭載（FEATURE_USB_HOST が無い）。Rust で Unsupported
//   それ以外 <件数（10 進）> に、件数分の「U+001E + vid（10 進）+ U+001F + pid（10 進）+
//            U+001F + 製品名」が続く。0 件は "0" だけ。製品名が無い・取れない機器は空文字
//            （件数には数える）。製品名からは U+001E・U+001F を除いてから詰める。
//
// 読むのは getVendorId / getProductId / getProductName だけ。getDeviceList() のキー（/dev/bus/usb の
// パス）・getSerialNumber()・getDeviceName()・getManufacturerName() は読まない。機器は開かず
// requestPermission もしない（権限も宣言しない）。製品名の取得に接続許可が要る端末では
// SecurityException を名前なしとして吸収する。API は 12/21 以降（minSdk 24）。
// Activity は握らず ApplicationContext だけを使い、要求 id・保留状態も持たないので、
// Activity 再生成で破棄すべきものは無い。
// 守る範囲: 0 件・非対応・失敗の区別。守らない範囲: 読み取り後の抜き差しの反映（再取得まで）。
final class UsbDeviceHelper {
    private static final char REC = '\u001E';
    private static final char SEP = '\u001F';

    private UsbDeviceHelper() {}

    static String list(Context context) {
        try {
            if (!context.getPackageManager().hasSystemFeature(PackageManager.FEATURE_USB_HOST)) {
                return "";
            }
            UsbManager um = (UsbManager) context.getSystemService(Context.USB_SERVICE);
            if (um == null) {
                return null;
            }
            StringBuilder sb = new StringBuilder();
            int count = 0;
            for (UsbDevice d : um.getDeviceList().values()) {
                count++;
                String name = "";
                try {
                    String n = d.getProductName();
                    if (n != null) {
                        name = n.replace(String.valueOf(REC), "").replace(String.valueOf(SEP), "");
                    }
                } catch (RuntimeException e) {
                    // 名前なしとして数える。
                }
                sb.append(REC).append(d.getVendorId()).append(SEP).append(d.getProductId())
                        .append(SEP).append(name);
            }
            return count + sb.toString();
        } catch (RuntimeException e) {
            return null;
        }
    }
}
