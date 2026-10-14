// ルートビルドスクリプト（TASK-5.2 / REQ-5、Issue #237）。
//
// AGP・Compose コンパイラプラグインの宣言のみを担う。実際の適用は
// baseline/build.gradle.kts・native-sample/build.gradle.kts（各モジュール）側で行う
// （platforms/android/build.gradle.kts と同じ構成方針）。AGP 9.0 以降は Kotlin サポートが
// 組み込みのため（built-in Kotlin support）、`org.jetbrains.kotlin.android` は宣言しない
// （https://kotl.in/gradle/agp-built-in-kotlin）。
//
// バージョンは scripts/android/versions.env の AGP_VERSION / KOTLIN_VERSION と
// 一致させること（SSOT はそちら。選定根拠・一次情報 URL は同ファイルのコメント参照）。
plugins {
    id("com.android.application") version "9.3.1" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.10" apply false
}
