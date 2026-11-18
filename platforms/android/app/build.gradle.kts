// Android アプリモジュール（TASK-11.2-S2 / REQ-11、Issue #226。
// Activity 方式は TASK-9.3-S1 / REQ-9 / M4、Issue #242 で GameActivity へ変更。
// 決定記録: docs/architecture/android-activity-method.md）。
//
// GameActivity 方式（NativeActivity から変更）。実描画は `fandhe-gallery`
// （crates/fandhe-gallery/src/lib.rs の android_main）が cdylib（`libfandhe_gallery.so`）として
// 提供し、本モジュールは最小の Java サブクラス
// （src/main/java/com/fandhe/platform/MainActivity.java、GameActivity 継承）経由で
// それを起動する薄いラッパー。GameActivity は NativeActivity と異なりサブクラス化と
// ネイティブライブラリの明示ロードが必須のため、`hasCode="false"` 前提は撤回した
// （追加コードは MainActivity.java と、共通層・機能 crate の android/java に閉じる）。
//
// sub-issue 間のスコープ境界（重要）:
//   - 開発署名鍵による署名（signingConfigs）は本ファイルで実装済み（TASK-11.2-S3 /
//     #227）。契約（環境変数インターフェース・keystore 生成手順）は
//     scripts/android/build-aab.sh と docs/operations/android-aab-signing.md を参照。
//   - CI（.github/workflows/ci.yml）への組み込みは TASK-11.2-S4（#228）へ申し送る。
//   - accesskit（accesskit_android）の有効化・動作整合は TASK-9.3-S2（#243）へ申し送る。
//     本ファイルは GameActivity への切替とビルド成立確認のみが範囲。
//
// compileSdk / buildToolsVersion / minSdk は scripts/android/versions.env の
// PLATFORM_PACKAGE（platforms;android-35）/ BUILD_TOOLS_PACKAGE（build-tools;36.0.0）/
// ANDROID_MIN_API（24）と一致させること（SSOT はそちら）。games-activity などの依存の版は
// 共通層・機能 crate の宣言（`gradle`）が正で、gen の出力から入る（選定根拠は versions.env）。
plugins {
    id("com.android.application")
}

android {
    namespace = "com.fandhe.platform"
    compileSdk = 35
    // AGP 9.3.1 が要求する最小 build-tools バージョン（36.0.0）を明記する
    // （scripts/android/versions.env の BUILD_TOOLS_PACKAGE と一致させること。
    // 更新履歴・根拠は同ファイルのコメント参照）。
    buildToolsVersion = "36.0.0"

    defaultConfig {
        applicationId = "com.fandhe.androidapp"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"

        // scripts/android/build-aab.sh が `-PfandheAndroidAbis=<カンマ区切り ABI>` で
        // 今回のクロスビルド対象 ABI を明示的に渡す。ndk.abiFilters で要求外 ABI を
        // 確実に除外することで、jniLibs.srcDirs 側（target/android/jniLibs/）に
        // 過去ビルドの残置ディレクトリが万一残っていても .aab には収載させない
        //（Cursor Bugbot 指摘: stale ABI libs packaged into AAB。build-aab.sh 側の
        // `rm -rf` によるクリアと二重の防御）。プロパティ未指定時（gradle 単独実行等）は
        // scripts/android/build-aab.sh の既定 ABI 一覧と一致させる。
        val abiCsv = (project.findProperty("fandheAndroidAbis") as String?)
            ?: "arm64-v8a,armeabi-v7a,x86_64"
        ndk {
            abiFilters += abiCsv.split(",").map { it.trim() }.filter { it.isNotEmpty() }
        }
    }

    // GameActivity サブクラス（MainActivity.java）のコンパイルに使う Java バージョン。
    // Kotlin ソースは持たないため kotlinOptions は不要。
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    // jniLibs の実体は Gradle プロジェクト配下ではなく、cargo-ndk が
    // `-o` オプションで出力するリポジトリルート直下の target/android/jniLibs/
    // （.gitignore 済みの生成物ディレクトリ、platforms/android/ 配下を汚さないための
    // 意図的な配置）を参照する。
    sourceSets {
        getByName("main") {
            // AGP 9.x では `jniLibs.srcDirs(...)` が非推奨（後継の `directories`
            // mutable set は Kotlin DSL からの直接代入 API が未整備で
            // `Unresolved reference` になることを確認したため、
            // 対象範囲外の API 移行としてここでは非推奨警告を許容する）。
            @Suppress("DEPRECATION")
            jniLibs.srcDirs("../../../target/android/jniLibs")
        }
    }

    // 開発署名鍵（TASK-11.2-S3 / #227）: FANDHE_ANDROID_KEYSTORE が非空の環境変数として
    // 渡された場合のみ signingConfigs を定義し、下の buildTypes.release へ割り当てる。
    // 未設定時は S2 までと同じ「未署名 .aab」挙動を完全に維持する（後方互換）。
    // 鍵・パスワードはこのファイルへ直書きせず、System.getenv() で都度読む
    // （security.md 準拠。Gradle -P プロパティは使わない。プロセス一覧・ログへの
    // 露出を避けるため scripts/android/build-aab.sh 側もコマンドラインへは渡さない）。
    // パスワード類の存在検査・keystore パスの絶対パス化・fail-closed な前提検査は
    // scripts/android/build-aab.sh のフェーズ 1 が担う（本ファイルは契約の受け口のみ）。
    val devKeystorePath = System.getenv("FANDHE_ANDROID_KEYSTORE")
    if (!devKeystorePath.isNullOrEmpty()) {
        val devKeystorePassword = System.getenv("FANDHE_ANDROID_KEYSTORE_PASSWORD")
        // `?:` は null のみをフォールバック対象とし空文字列を通してしまうため、
        // devKeystorePath と同じ isNullOrEmpty() 判定に揃える（GitHub Actions 等で
        // 未設定の secrets が空文字列として展開される典型パターン。S4 / #228 の
        // GitHub Secrets 経由配布で起こりうる。docs/operations/android-aab-signing.md
        // §3 の契約表と実装を一致させる）。
        val envKeyAlias = System.getenv("FANDHE_ANDROID_KEY_ALIAS")
        val devKeyAlias = if (envKeyAlias.isNullOrEmpty()) "fandhe-dev" else envKeyAlias
        // PKCS12 keystore では鍵パスワードと store パスワードが同一であることが多いため、
        // 未指定時は store パスワードを流用する（docs/operations/android-aab-signing.md 参照）。
        val envKeyPassword = System.getenv("FANDHE_ANDROID_KEY_PASSWORD")
        val devKeyPassword = if (envKeyPassword.isNullOrEmpty()) devKeystorePassword else envKeyPassword

        signingConfigs {
            create("fandheDev") {
                storeFile = file(devKeystorePath)
                storePassword = devKeystorePassword
                keyAlias = devKeyAlias
                keyPassword = devKeyPassword
            }
        }
    }

    // ABI split は行わない（`.aab` に defaultConfig.ndk.abiFilters で指定した
    // 全 ABI（既定は arm64-v8a / armeabi-v7a / x86_64 の 3 ABI。上記
    // fandheAndroidAbis プロパティで変更可）を収載し、Play Store 側の
    // デバイス別 APK 生成に委ねる。scripts/android/build-aab.sh フェーズ 3 が
    // unzip -l で「要求 ABI の収載」と「要求外 ABI の不在」の両方を検証する）。
    splits {
        abi {
            isEnable = false
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // FANDHE_ANDROID_KEYSTORE 未設定時は signingConfigs.fandheDev 自体が
            // 定義されないため、意図的に signingConfig を割り当てず未署名のままにする
            // （S2 までの後方互換。挙動の分岐は上の signingConfigs 定義箇所のコメント参照）。
            if (!devKeystorePath.isNullOrEmpty()) {
                signingConfig = signingConfigs.getByName("fandheDev")
            }
        }
    }
}

// ディープリンク（App Links、Issue #1279）。許可ホストはビルド時の環境変数
// FANDHE_ANDROID_DEEP_LINK_HOST で渡す（Rust 側 `option_env!` も同じ変数を読む。値は書かない）。
// 設定時だけ src/deeplink/AndroidManifest.xml（autoVerify の https intent-filter）を重ねる。
// 未設定なら filter を出さない（host なしの https 宣言は全 https を受けるため）。形式の正は
// `fandhe_cap_deep_link::validate_host`。ずれても Rust が拒否する側（fail-closed）に倒れる。
// 不正な形式は XML への注入を避けるためビルドを止める（値はメッセージに出さない）。
val deepLinkHost: String? = System.getenv("FANDHE_ANDROID_DEEP_LINK_HOST")?.takeIf { it.isNotEmpty() }
if (deepLinkHost != null) {
    if (!Regex("^[a-z0-9-]+(\\.[a-z0-9-]+)*$").matches(deepLinkHost)) {
        throw GradleException("FANDHE_ANDROID_DEEP_LINK_HOST の形式が不正です")
    }
    android.defaultConfig.manifestPlaceholders["fandheDeepLinkHost"] = deepLinkHost
    androidComponents {
        onVariants(selector().all()) { variant ->
            variant.sources.manifests.addStaticManifestFile("src/deeplink/AndroidManifest.xml")
        }
    }
}

// 機能 crate の Java・res・Manifest の overlay・Gradle 依存（GameActivity 等の基盤を含む）は
// `cargo fandhe gen` の出力（crates/fandhe-gallery/gen/android/）から取り込む。ここには殻に固有の設定だけを書く。
// 生成前は失敗させる（scripts/android/build-aab.sh 等は Gradle の前に gen を実行する）。
val fandheGradle = rootProject.file("../../crates/fandhe-gallery/gen/android/fandhe.gradle")
check(fandheGradle.exists()) { "crates/fandhe-gallery/gen/android/fandhe.gradle が無い。先に `cargo fandhe gen` を実行する" }
apply(from = fandheGradle)
