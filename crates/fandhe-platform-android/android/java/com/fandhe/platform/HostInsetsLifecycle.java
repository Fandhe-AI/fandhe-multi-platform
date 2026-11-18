package com.fandhe.platform;

import androidx.activity.ComponentActivity;
import androidx.core.util.Consumer;
import androidx.core.view.WindowInsetsCompat;

// 描画の余白用の window insets を、端末機能の有無によらず Rust へ届けるための非公開 ContentProvider（Issue #1200）。
// 宣言は `fandhe-platform-android` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
//
// `HostInsets` の取り付けは最初の `addListener` で起きる。機能を 1 つも選ばないアプリ（入口 `#[fandhe::main]` だけのアプリ）でも
// 画面の余白（`fandhe_platform_android::window_insets_px()`）が取れるよう、何もしない配り先を登録して取り付けを起こす。
// `HostInsets` は decorView を触るので `onHostCreated` ではなく `onHostStarted` で呼ぶ。配り先は `static final` の同一インスタンスで、
// 再登録の重複排除が効く。
public final class HostInsetsLifecycle extends FeatureLifecycleProvider {
    private static final Consumer<WindowInsetsCompat> NO_OP = insets -> {};

    @Override
    protected void onHostStarted(ComponentActivity activity) {
        HostInsets.addListener(activity, NO_OP);
    }
}
