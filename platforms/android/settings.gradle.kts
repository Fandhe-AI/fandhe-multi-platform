// Android アプリプロジェクトのルート設定（TASK-11.2-S2 / REQ-11、Issue #226。
// 親: TASK-11.2 系列 / #224）。
//
// 呼び出し文脈: scripts/android/build-aab.sh が
// `gradle -p platforms/android :app:bundleRelease` として本プロジェクトを起動する。
// Gradle wrapper は同梱しない（wrapper jar というバイナリのコミットを避け、
// scripts/android/setup-android-env.sh がチェックサム検証済みで導入する
// Gradle 本体（GRADLE_HOME、scripts/android/android-env.sh が export）を使う前提）。
//
// pluginManagement.repositories は google() / mavenCentral() / gradlePluginPortal()
// の 3 つ（いずれも公式・信頼できるリポジトリ）に限定し、野良リポジトリを追加しない
// （security.md A05・A06 対策）。dependencyResolutionManagement.repositories は
// google() / mavenCentral() の 2 つに限定する（プラグイン解決と依存解決でスコープが異なる）。

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

rootProject.name = "fandhe-android"

include(":app")
