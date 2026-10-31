package com.fandhe.androidapp;

import android.content.Intent;
import android.content.pm.ApplicationInfo;

import androidx.annotation.NonNull;
import androidx.car.app.CarAppService;
import androidx.car.app.Screen;
import androidx.car.app.Session;
import androidx.car.app.validation.HostValidator;

// 車載ディスプレイ（Issue #1117、親 #152）の `CarAppService`。AndroidManifest の exported な
// `<service>` で宣言され、Android Auto のホストが bind する。exported のため接続元は
// `createHostValidator` で絞る（debuggable だけ全ホストを許し、それ以外は同梱の許可リスト）。
// 文言の受け渡しは MainActivity とは独立したプロセス内 static（`CarDisplayScreen`）で行う。
// 切り離し時は Manifest の service・meta-data 2 つ・res/xml/automotive_app_desc.xml・
// strings.xml の car_display_*・このクラスと CarDisplayScreen・MainActivity の受け口 2 本を削除する。
public final class CarDisplayService extends CarAppService {
    @NonNull
    @Override
    public HostValidator createHostValidator() {
        if ((getApplicationInfo().flags & ApplicationInfo.FLAG_DEBUGGABLE) != 0) {
            return HostValidator.ALLOW_ALL_HOSTS_VALIDATOR;
        }
        return new HostValidator.Builder(getApplicationContext())
                .addAllowedHosts(androidx.car.app.R.array.hosts_allowlist_sample)
                .build();
    }

    @NonNull
    @Override
    public Session onCreateSession() {
        return new CarSession();
    }

    private static final class CarSession extends Session {
        @NonNull
        @Override
        public Screen onCreateScreen(@NonNull Intent intent) {
            return new CarDisplayScreen(getCarContext());
        }
    }
}
