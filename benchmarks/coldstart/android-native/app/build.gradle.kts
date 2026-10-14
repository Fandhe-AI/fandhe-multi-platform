// Android アプリモジュール（TASK-5.1 / REQ-5、Issue #236）。
//
// `fandhe_core::sample_screen()`（テキスト・テキスト入力・ボタン 2 種・リストの
// 6 要素構成、`crates/fandhe-core/src/ui/sample.rs`）と同一機能を Jetpack Compose で
// 再現し、`scripts/android/measure-cold-start.sh` が fandhe 実装
// （`platforms/android/app`、NativeActivity）と同一条件（release ビルドタイプ・
// `adb shell am start -W -S`）でコールドスタートを比較するための計測専用フィクスチャ。
//
// compileSdk / buildToolsVersion / minSdk は `platforms/android/app/build.gradle.kts`
// および `scripts/android/versions.env` と一致させ、計測条件を対称にする。
// AGP 9.0 以降は Kotlin サポートが AGP 本体へ組み込まれたため
// `org.jetbrains.kotlin.android` プラグインの適用は不要（適用すると
// 「重複適用」としてビルドエラーになる。https://kotl.in/gradle/agp-built-in-kotlin
// 参照、本開発環境で実際に発生を確認した）。Compose コンパイラの統合のみ
// `org.jetbrains.kotlin.plugin.compose` で行う。
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    // namespace/applicationId は `com.fandhe.coldstart.nativeapp` とする
    // （`native` は Java 予約キーワードのため AGP の namespace 検証で拒否される。
    // 本環境で確認済み。MainActivity.kt 冒頭コメントも参照）。
    namespace = "com.fandhe.coldstart.nativeapp"
    compileSdk = 35
    buildToolsVersion = "36.0.0"

    defaultConfig {
        applicationId = "com.fandhe.coldstart.nativeapp"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    // AGP 9.0 以降の組み込み Kotlin サポートは compileOptions の JavaVersion から
    // Kotlin の jvmTarget を導出するため、`kotlinOptions { jvmTarget = ... }`
    // （旧 org.jetbrains.kotlin.android プラグイン由来の DSL）は不要
    // （適用すると Unresolved reference になることを本環境で確認した）。
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildTypes {
        release {
            // fandhe 側（platforms/android/app）と同じく release ビルドタイプで
            // debuggable=false による起動オーバーヘッドを排除する（計測の公平性、
            // scripts/android/measure-cold-start.sh フェーズ 3 のコメント参照）。
            // 未署名 APK は `adb install` が INSTALL_PARSE_FAILED_NO_CERTIFICATES で
            // 拒否するため実機計測に使えない（Review 指摘対応）。AGP 既定の
            // debug signingConfig（`~/.android/debug.keystore`、開発用固定鍵）で
            // 署名することで、`debuggable=false` は維持したまま `adb install`
            // 可能にする（本アプリは計測専用フィクスチャでありストア配布物では
            // ないため、開発署名で十分。fandhe 側は既存の開発署名鍵契約
            // （FANDHE_ANDROID_KEYSTORE、docs/operations/android-aab-signing.md）を
            // 再利用する）。
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    buildFeatures {
        compose = true
    }

    // 計測専用フィクスチャであり、ストア配布パイプライン（platforms/）とは無関係の
    // ため ABI split・NDK 設定は不要（Kotlin/Compose のみで完結する純ネイティブ実装）。
}

dependencies {
    // Compose BOM は scripts/android/versions.env の COMPOSE_BOM_VERSION（TASK-5.2 /
    // Issue #237 で導入した Compose 比較アプリ用 SSOT）と一致させる（Bugbot 指摘対応）。
    val composeBom = platform("androidx.compose:compose-bom:2026.06.01")
    implementation(composeBom)

    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
}
