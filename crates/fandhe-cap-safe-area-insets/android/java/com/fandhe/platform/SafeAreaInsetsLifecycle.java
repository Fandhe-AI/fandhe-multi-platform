package com.fandhe.platform;

import androidx.activity.ComponentActivity;
import androidx.core.util.Consumer;
import androidx.core.view.WindowInsetsCompat;

// safe-area-insets が Activity のライフサイクルへ参加するための非公開 ContentProvider（Issue #1334）。
// 宣言は `fandhe-cap-safe-area-insets` の Cargo.toml（`android.components`）にあり、OS がプロセス開始時に生成する。
//
// 値そのもの（`systemBars | displayCutout` の px）は共通層 `HostInsets` が Rust の static へ送り、本機能の Rust は
// `fandhe_platform_android::window_insets_px()` を読むだけ。ただし `HostInsets` の取り付けは最初の `addListener` で起きるため、
// 値だけが要る本機能も、何もしない配り先を登録して取り付けを起こす。`HostInsets` は decorView を触るので `onHostCreated` ではなく
// `onHostStarted` で呼ぶ。配り先は `static final` の同一インスタンスで、再登録の重複排除が効く。
public final class SafeAreaInsetsLifecycle extends FeatureLifecycleProvider {
    private static final Consumer<WindowInsetsCompat> NO_OP = insets -> {};

    @Override
    protected void onHostStarted(ComponentActivity activity) {
        HostInsets.addListener(activity, NO_OP);
    }
}
