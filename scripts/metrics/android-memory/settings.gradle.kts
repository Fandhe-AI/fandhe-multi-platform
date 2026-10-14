// TASK-5.2（REQ-5 / REQ-6 実施後、Issue #237）メモリ増分実機計測専用の Gradle プロジェクト。
//
// 位置づけ: 配布用アプリ（platforms/android、NativeActivity + fandhe-egui-proxy）と
// **意図的に分離**した検証専用プロジェクト。ここで作る 2 アプリ（baseline / native-sample）は
// ストア配布経路（scripts/android/build-aab.sh・release 署名）には一切接続されない
// （security.md A05 対策。配布用の platforms/android/settings.gradle.kts へは
// モジュールとして追加しない）。
//
// 呼び出し文脈: scripts/android/measure-idle-memory-device.sh が
// `gradle -p scripts/metrics/android-memory :baseline:assembleDebug` /
// `:native-sample:assembleDebug` としてビルドし、debug 署名の APK を
// 実機計測（TotalPSS 比較）に用いる。
//
// リポジトリ定義は platforms/android/settings.gradle.kts と同方針
// （google() / mavenCentral() / gradlePluginPortal() に限定。security.md A05/A06 対策）。

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "fandhe-memory-metrics"

include(":baseline")
include(":native-sample")
