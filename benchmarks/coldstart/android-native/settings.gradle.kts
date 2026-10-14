// TASK-5.1（REQ-5 / M4、Issue #236）: Android コールドスタート計測用ネイティブ SDK
// 比較アプリ（Jetpack Compose）の独立 Gradle プロジェクト設定。
//
// `platforms/android/`（fandhe 実装、TASK-11.2-S2）とは独立したプロジェクトとして
// `benchmarks/` 配下に隔離する（親issue 計画 §2-2 参照）。計測フィクスチャであり
// ストア提出パイプライン（platforms/）には含まれない。
//
// pluginManagement.repositories / dependencyResolutionManagement.repositories を
// google() / mavenCentral() / gradlePluginPortal() に限定する方針は
// `platforms/android/settings.gradle.kts` と同一（security.md A05・A06 対策）。
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

rootProject.name = "fandhe-coldstart-android-native"

include(":app")
