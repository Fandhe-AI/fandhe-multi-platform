package com.fandhe.androidapp;

import android.os.Build;
import android.os.Bundle;
import android.telecom.Connection;
import android.telecom.ConnectionRequest;
import android.telecom.ConnectionService;
import android.telecom.DisconnectCause;
import android.telecom.PhoneAccountHandle;
import android.telecom.TelecomManager;

import androidx.annotation.RequiresApi;

// 通話 UI 統合（Issue #278）の self-managed ConnectionService。`CallUiHelper` が
// `TelecomManager.addNewIncomingCall` で出した着信を Telecom 経由で受け、`CallConnection` を作る。
// self-managed では着信 UI を OS が出さないため、`onShowIncomingCallUi` で `CallUiHelper` が着信通知を出す。
// bind できるのは `BIND_TELECOM_CONNECTION_SERVICE` を持つ Telecom だけ（マニフェスト参照）。
//
// 状態は `CallUiHelper` の static（main looper 専用）に集約する。Telecom のコールバックは main で来る。
// 取り消し済み・旧世代の着信が遅れて届いたら、通知を出さず失敗の Connection で閉じる（契約表 3）。
// 1 つの Connection に対する確定は first-wins（`CallUiHelper.finish` の id 照合と connection 照合）。
@RequiresApi(Build.VERSION_CODES.O)
public final class CallUiConnectionService extends ConnectionService {

    // addNewIncomingCall の extras が直下に来るか EXTRA_INCOMING_CALL_EXTRAS の入れ子で来るかは
    // 実機で未確認のため、両方読む。
    private static long readId(ConnectionRequest request) {
        Bundle extras = request == null ? null : request.getExtras();
        if (extras == null) {
            return RequestTracker.NONE;
        }
        if (extras.containsKey(CallUiHelper.EXTRA_CALL_ID)) {
            return extras.getLong(CallUiHelper.EXTRA_CALL_ID, RequestTracker.NONE);
        }
        Bundle nested = extras.getBundle(TelecomManager.EXTRA_INCOMING_CALL_EXTRAS);
        return nested == null
                ? RequestTracker.NONE
                : nested.getLong(CallUiHelper.EXTRA_CALL_ID, RequestTracker.NONE);
    }

    @Override
    public Connection onCreateIncomingConnection(
            PhoneAccountHandle account, ConnectionRequest request) {
        long id = readId(request);
        if (id == RequestTracker.NONE
                || id != CallUiHelper.currentId
                || CallUiHelper.connection != null) {
            return Connection.createFailedConnection(new DisconnectCause(DisconnectCause.CANCELED));
        }
        CallConnection c = new CallConnection(id);
        c.setConnectionProperties(Connection.PROPERTY_SELF_MANAGED);
        c.setCallerDisplayName(
                getString(R.string.call_ui_caller_name), TelecomManager.PRESENTATION_ALLOWED);
        c.setAudioModeIsVoip(true);
        c.setRinging();
        CallUiHelper.connection = c;
        return c;
    }

    @Override
    public void onCreateIncomingConnectionFailed(
            PhoneAccountHandle account, ConnectionRequest request) {
        CallUiHelper.finish(readId(request), CallUiHelper.EVENT_FAILED);
    }

    /** 疑似着信 1 件ぶんの Connection。確定は `CallUiHelper` へ集める。 */
    static final class CallConnection extends Connection {
        private final long id;

        CallConnection(long id) {
            this.id = id;
        }

        private boolean current() {
            return id == CallUiHelper.currentId && CallUiHelper.connection == this;
        }

        @Override
        public void onShowIncomingCallUi() {
            if (current()) {
                CallUiHelper.postIncoming(id);
            }
        }

        @Override
        public void onAnswer() {
            if (current()) {
                CallUiHelper.answered(id);
            }
        }

        @Override
        public void onAnswer(int videoState) {
            onAnswer();
        }

        @Override
        public void onReject() {
            if (current()) {
                CallUiHelper.finish(id, CallUiHelper.EVENT_DECLINED);
            }
        }

        // OS 側（他通話の調停・周辺機器）からの終了。
        @Override
        public void onDisconnect() {
            if (current()) {
                CallUiHelper.finish(id, CallUiHelper.EVENT_REMOTE_ENDED);
            }
        }

        @Override
        public void onAbort() {
            onDisconnect();
        }

        // 通知の応答・拒否ボタン（CallUiActionReceiver）から。
        void doAnswer() {
            onAnswer();
        }

        void doReject() {
            onReject();
        }
    }
}
