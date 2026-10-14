// ルートビルドスクリプト（TASK-5.1 / REQ-5、Issue #236）。
//
// AGP バージョンは `scripts/android/versions.env` の AGP_VERSION と一致させる
// （fandhe 実装（platforms/android）と同一 AGP で計測条件を揃えるため）。
// Kotlin / Compose コンパイラプラグインのバージョンも同ファイルの
// KOTLIN_VERSION（TASK-5.2 / Issue #237 で導入した Compose 比較アプリ用 SSOT）に
// 揃える（Bugbot 指摘対応。二重管理を避けるため本比較アプリ（TASK-5.1）でも
// 同一 SSOT を参照する）。
//
// 選定根拠（取得日 2026-08-01）:
//   - AGP 9.0 以降は Kotlin サポートが AGP 本体へ組み込まれたため、
//     `org.jetbrains.kotlin.android` プラグインは適用しない（適用すると
//     "The 'org.jetbrains.kotlin.android' plugin is no longer required for Kotlin
//     support since AGP 9.0" でビルドエラーになることを本環境で確認した。
//     https://kotl.in/gradle/agp-built-in-kotlin 参照）。
//   - `org.jetbrains.kotlin.plugin.compose`（2.4.10、scripts/android/versions.env の
//     KOTLIN_VERSION と同一）のみ Compose コンパイラ統合のために適用する。
//     AGP 9.3.1 との組み合わせは本開発環境（Linux、JDK 21.0.12+8）で
//     `gradle assembleRelease` の成立を確認済み。
plugins {
    id("com.android.application") version "9.3.1" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.10" apply false
}
