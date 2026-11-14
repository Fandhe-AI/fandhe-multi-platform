package com.fandhe.platform;

import android.content.Context;

import com.google.android.gms.common.ConnectionResult;
import com.google.android.gms.common.GoogleApiAvailability;

/**
 * Google Play services の有無を判定する（決済リクエストのヘルパーから呼ぶ。書類スキャン・行動認識は機能 crate の
 * ヘルパーが同じ判定を持ち、GMS を使わない機能を選んだ利用者に依存を乗せない）。
 *
 * <p>SUCCESS だけを可とし、それ以外（無い・古い・無効）は呼び出し側で Unsupported に写す。
 * 更新ダイアログは出さない。
 */
final class GmsAvailability {
    private GmsAvailability() {}

    static boolean isAvailable(Context ctx) {
        return GoogleApiAvailability.getInstance().isGooglePlayServicesAvailable(ctx)
                == ConnectionResult.SUCCESS;
    }
}
