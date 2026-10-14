package com.fandhe.metrics.baseline

import android.app.Activity
import android.os.Bundle

/**
 * TASK-5.2（REQ-5、Issue #237）のメモリ増分計測におけるベースラインアプリ。
 *
 * PoC-4 の空ウィンドウ手法を Android 実機向けに再現する。`android.app.Activity`
 * （AndroidX の `ComponentActivity` ではない）を直接継承し、`setContentView` も
 * 呼ばない。Compose・View 階層等の UI フレームワークを一切使わないことで、
 * native-sample（Jetpack Compose）・fandhe 本体（NativeActivity + fandhe-egui-proxy）
 * それぞれの TotalPSS からアプリ起動自体のオーバーヘッドを差し引くための基準点となる。
 *
 * 呼び出し元: scripts/android/measure-idle-memory-device.sh が
 * `adb shell am start -n com.fandhe.metrics.baseline/.BaselineActivity` で起動し、
 * `adb shell dumpsys meminfo` で TotalPSS を計測する。
 */
class BaselineActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // 意図的に setContentView を呼ばない（UI フレームワーク不使用のベースライン）。
    }
}
