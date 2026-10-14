// baseline アプリ（TASK-5.2 / REQ-5、Issue #237）。
//
// PoC-4（docs/spec/03-poc/performance/README.md）のベースライン手法（空ウィンドウのみ）を
// Android 実機向けに再現する。UI フレームワーク（Compose・View 階層）を一切使わず、
// 空の `Activity` のみを起動する最小構成とし、fandhe（native-sample）・fandhe 本体の
// TotalPSS 増分計測時に差し引くベースラインとして使う。
//
// **検証専用**。配布経路（platforms/android・release 署名）とは分離済み（settings.gradle.kts
// 参照）。debug 署名のみで、INTERNET 等の権限は要求しない（security.md A05 対策）。
// AGP 9.0 以降は Kotlin サポートが組み込みのため（built-in Kotlin support）、
// `org.jetbrains.kotlin.android` プラグインの適用は不要かつエラーになる
// （https://kotl.in/gradle/agp-built-in-kotlin）。
plugins {
    id("com.android.application")
}

android {
    namespace = "com.fandhe.metrics.baseline"
    // scripts/android/versions.env の PLATFORM_PACKAGE / BUILD_TOOLS_PACKAGE /
    // ANDROID_MIN_API と一致させる（platforms/android/app/build.gradle.kts と同じ方針）。
    compileSdk = 35
    buildToolsVersion = "36.0.0"

    defaultConfig {
        applicationId = "com.fandhe.metrics.baseline"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }
}
