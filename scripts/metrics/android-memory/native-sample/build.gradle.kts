// native-sample アプリ（TASK-5.2 / REQ-5、Issue #237）。
//
// `crates/fandhe-core/src/ui/sample.rs` の `sample_screen()`（見出しテキスト・
// テキスト入力・ボタン 2 種・リストの計 6 要素）を、Android 側の宣言的 UI フレームワークで
// あり PoC-4 が対応関係として用いた「宣言的 UI」の系譜にある Jetpack Compose で
// 再現した比較アプリ。fandhe（NativeActivity + fandhe-egui-proxy）のアイドル時メモリ増分を、
// 同一機能のネイティブ SDK 実装と比較するための基準として使う。
//
// **検証専用**。配布経路（platforms/android・release 署名）とは分離済み（settings.gradle.kts
// 参照）。debug 署名のみで、INTERNET 等の権限は要求しない（security.md A05 対策）。
// Compose の依存バージョンは androidx.compose:compose-bom（scripts/android/versions.env の
// COMPOSE_BOM_VERSION が SSOT）で一元管理し、個別ライブラリのバージョンは直書きしない
// （security.md A06 対策）。
// AGP 9.0 以降は Kotlin サポートが組み込みのため（built-in Kotlin support）、
// `org.jetbrains.kotlin.android` プラグインの適用は不要かつエラーになる
// （https://kotl.in/gradle/agp-built-in-kotlin）。Compose コンパイラプラグインのみ適用する。
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.fandhe.metrics.nativesample"
    compileSdk = 35
    buildToolsVersion = "36.0.0"

    defaultConfig {
        applicationId = "com.fandhe.metrics.nativesample"
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

    buildFeatures {
        compose = true
    }
}

dependencies {
    // scripts/android/versions.env の COMPOSE_BOM_VERSION と一致させること（SSOT はそちら）。
    val composeBom = platform("androidx.compose:compose-bom:2026.06.01")
    implementation(composeBom)
    // androidx.activity は Compose BOM の管理対象外（BOM は androidx.compose グループのみ）。
    // 1.10 系以降は androidx.navigationevent（compileSdk 36 以上を要求）へ推移的に依存するため、
    // 本プロジェクトの compileSdk（35、scripts/android/versions.env の PLATFORM_PACKAGE と
    // 一致させる方針。distribution 側の platforms/android と同一 SDK 前提を崩さないため
    // compileSdk 単体の引き上げは行わない）と両立する最終バージョン 1.9.3 に固定する
    // （ACTIVITY_COMPOSE_VERSION、scripts/android/versions.env 参照）。
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
}
