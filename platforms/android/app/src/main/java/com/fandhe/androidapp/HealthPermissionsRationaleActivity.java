package com.fandhe.androidapp;

import android.app.Activity;
import android.os.Bundle;
import android.widget.TextView;

// Health Connect の許可画面から開かれる「権限の理由」画面（Issue #1100）。
// Manifest の activity-alias（VIEW_PERMISSION_USAGE + HEALTH_PERMISSIONS）の実体。
// 受け取った Intent の extras は読まず、固定文言だけを表示する（外部入力を受けない）。
public final class HealthPermissionsRationaleActivity extends Activity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        TextView view = new TextView(this);
        view.setText("今日の歩数を表示するためだけに、Health Connect の歩数を読み取ります。"
                + "データを外部へ送信したり、広告に使ったりしません。");
        view.setPadding(48, 48, 48, 48);
        setContentView(view);
    }
}
