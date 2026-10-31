package com.fandhe.androidapp;

import android.nfc.cardemulation.HostApduService;
import android.os.Bundle;

// NFC カードエミュレーション（HCE、Issue #1129）の `HostApduService`。AndroidManifest の `<service>`
// （exported=true、`BIND_NFC_SERVICE` 要求）で宣言され、bind できるのはシステムだけ。OS は
// `res/xml/apduservice.xml` に登録した AID への SELECT だけをここへ振るので、AID のバイト列とは
// 照合せずヘッダ（CLA=00, INS=A4, P1=04）だけを見る。AID の正は Rust の `AID` 定数。
// 応答は固定のバイト列で、入力を反映せず、秘密も返さない。APDU・応答の中身はログに出さない。
public final class NfcCardEmulationService extends HostApduService {
    // ASCII "FANDHE" + SW 9000
    private static final byte[] OK = {'F', 'A', 'N', 'D', 'H', 'E', (byte) 0x90, 0x00};
    // INS not supported (6D00)
    private static final byte[] NOT_SUPPORTED = {0x6D, 0x00};

    // main thread で呼ばれるため即座に返す。
    @Override
    public byte[] processCommandApdu(byte[] apdu, Bundle extras) {
        if (apdu != null && apdu.length >= 4 && apdu[0] == 0x00 && apdu[1] == (byte) 0xA4 && apdu[2] == 0x04) {
            NfcCardEmulationHelper.recordResponse();
            return OK;
        }
        return NOT_SUPPORTED;
    }

    @Override
    public void onDeactivated(int reason) {}
}
