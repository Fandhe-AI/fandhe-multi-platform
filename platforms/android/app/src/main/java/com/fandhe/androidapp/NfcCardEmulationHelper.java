package com.fandhe.androidapp;

import android.content.ComponentName;
import android.content.Context;
import android.content.pm.PackageManager;
import android.nfc.NfcAdapter;
import android.nfc.cardemulation.CardEmulation;
import android.os.SystemClock;

// NFC カードエミュレーション（HCE、nfc-card-emulation、Issue #1129）の状態置き場と読み出し。
// 応答回数と最後の応答時刻はプロセス内 static に持つ（SharedPreferences は使わない。Rust 側 trait doc の
// 契約「回数はこのプロセス内」に揃える）。`NfcCardEmulationService` が `recordResponse` で書き、
// Rust の `android_nfc_card_emulation.rs` が `MainActivity.nfcCardEmulationStatus` 経由で `status` を読む。
// 書き込みと読み出しは同じ LOCK の内側で行い、回数と時刻の組を分裂させない。
final class NfcCardEmulationHelper {
    // `status` の戻り値 long[5] の配置（SSOT。Rust の `decode_status` はこの配置で解釈する）:
    //   [0] state: 0 = OK / 1 = 非対応（HCE 機能なし、または NfcAdapter が null）
    //   [1] nfcEnabled: 0 か 1
    //   [2] defaultForAid: 0 か 1
    //   [3] responses: 0 以上
    //   [4] sinceLastMs: responses == 0 のときは 0（Rust は無視して None にする）
    private static final Object LOCK = new Object();
    private static long responses = 0;
    private static long lastAt = 0;

    private NfcCardEmulationHelper() {}

    // main thread（processCommandApdu）から呼ばれる。溢れたら加算しない。
    static void recordResponse() {
        synchronized (LOCK) {
            if (responses < Long.MAX_VALUE) {
                responses++;
            }
            lastAt = SystemClock.elapsedRealtime();
        }
    }

    // ワーカースレッドから呼ばれる。OS の例外は握りつぶさず呼び出し元（Rust）へ返す。
    static long[] status(Context ctx, String aid) {
        long[] out = new long[5];
        if (!ctx.getPackageManager().hasSystemFeature(PackageManager.FEATURE_NFC_HOST_CARD_EMULATION)) {
            out[0] = 1;
            return out;
        }
        NfcAdapter adapter = NfcAdapter.getDefaultAdapter(ctx);
        if (adapter == null) {
            out[0] = 1;
            return out;
        }
        out[1] = adapter.isEnabled() ? 1 : 0;
        CardEmulation emu = CardEmulation.getInstance(adapter);
        out[2] = emu.isDefaultServiceForAid(new ComponentName(ctx, NfcCardEmulationService.class), aid) ? 1 : 0;
        synchronized (LOCK) {
            out[3] = responses;
            out[4] = responses == 0 ? 0 : SystemClock.elapsedRealtime() - lastAt;
        }
        return out;
    }
}
