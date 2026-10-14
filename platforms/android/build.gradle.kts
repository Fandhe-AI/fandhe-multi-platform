// ルートビルドスクリプト（TASK-11.2-S2 / REQ-11、Issue #226）。
//
// AGP（Android Gradle Plugin）プラグイン宣言のみを担う。実際の適用は
// app/build.gradle.kts（`:app` モジュール）側で行う。
//
// バージョンは scripts/android/versions.env の AGP_VERSION と一致させること
// （SSOT はそちら。選定根拠・一次情報 URL は同ファイルのコメント参照）。
plugins {
    id("com.android.application") version "9.3.1" apply false
}
