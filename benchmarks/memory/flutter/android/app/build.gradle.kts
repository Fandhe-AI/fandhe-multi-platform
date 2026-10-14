// fandhe `sample_screen()` との性能比較用の計測専用フィクスチャ（#647 S2 / Issue #657）。
// platforms/android の配布用プロジェクトとは分離しており、ストア提出パイプラインの
// 対象外。compileSdk / minSdk / targetSdk / NDK は Flutter テンプレートの
// `flutter.*` 既定値をそのまま使う（scripts/android/versions.env の SSOT とは独立、
// README「前提」参照）。
plugins {
    id("com.android.application")
    id("kotlin-android")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "com.fandhe.metrics.flutter_sample"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = JavaVersion.VERSION_17.toString()
    }

    defaultConfig {
        // 計測 Activity 起動契約（S3 の起動コマンド）に対応した固定 ID。変更しない。
        applicationId = "com.fandhe.metrics.flutter_sample"
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    buildTypes {
        release {
            // 計測専用フィクスチャのため adb install 可能化を優先し debug 署名のまま。
            // ストア提出用鍵は使わない（README「逸脱事項」参照）。
            signingConfig = signingConfigs.getByName("debug")
        }
    }
}

flutter {
    source = "../.."
}
