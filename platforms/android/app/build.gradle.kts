// Android アプリモジュール（TASK-11.2-S2 / REQ-11、Issue #226。
// Activity 方式は TASK-9.3-S1 / REQ-9 / M4、Issue #242 で GameActivity へ変更。
// 決定記録: docs/architecture/android-activity-method.md）。
//
// GameActivity 方式（NativeActivity から変更）。実描画は `fandhe-egui-proxy`
// （crates/fandhe-egui-proxy/src/android.rs）が cdylib（`libfandhe_egui_proxy.so`）として
// 提供し、本モジュールは最小の Java サブクラス
// （src/main/java/com/fandhe/androidapp/MainActivity.java、GameActivity 継承）経由で
// それを起動する薄いラッパー。GameActivity は NativeActivity と異なりサブクラス化と
// ネイティブライブラリの明示ロードが必須のため、`hasCode="false"` 前提は撤回した
// （追加コードは MainActivity.java と補助クラス RequestTracker.java に閉じる）。
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
// ANDROID_MIN_API（24）と一致させること（SSOT はそちら）。games-activity /
// appcompat のバージョンは同ファイルの GAMES_ACTIVITY_VERSION / APPCOMPAT_VERSION と
// 一致させること。
plugins {
    id("com.android.application")
}

android {
    namespace = "com.fandhe.androidapp"
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

// AndroidX 依存のバージョン SSOT（scripts/android/versions.env）を直接読む。
// KEY="value" 形式の行をパースして参照する（ハードコード複製によるドリフト防止、
// Issue #579）。キーが見つからない場合は error() でビルドを止める（fail-closed）。
// AGP のバージョン（platforms/android/build.gradle.kts の plugins ブロック）は
// Gradle の plugins DSL 制約で動的参照できないため、注釈による手動同期のまま。
val androidVersionsEnv: Map<String, String> =
    rootProject.file("../../scripts/android/versions.env")
        .readLines()
        .mapNotNull { line ->
            Regex("""^([A-Z0-9_]+)="([^"]*)"""").find(line.trim())
                ?.let { it.groupValues[1] to it.groupValues[2] }
        }
        .toMap()

fun androidVersion(key: String): String =
    androidVersionsEnv[key]
        ?: error("scripts/android/versions.env に $key が定義されていません（SSOT 参照失敗）")

dependencies {
    // GameActivity 本体（TASK-9.3-S1 / REQ-9 / M4、Issue #242）。
    // "buildFeatures { prefab = true }" は意図的に有効化しない
    // （android-activity README「Don't compile and link the upstream GameActivity
    // 'prefab' (C++ glue) layer」参照。android-activity crate 側が独自のネイティブ
    // グルーを持つため、prefab 経由の C++ 側 glue と二重化すると非互換になる）。
    implementation("androidx.games:games-activity:${androidVersion("GAMES_ACTIVITY_VERSION")}")
    // GameActivity は AppCompatActivity のサブクラスとして実装されているため
    // （android-activity README・GameActivity 導入ガイド参照）、AAR の POM に
    // 含まれない appcompat を明示的に追加する（dl.google.com の
    // games-activity-4.4.0.pom を直接確認、dependencies セクションなし）。
    implementation("androidx.appcompat:appcompat:${androidVersion("APPCOMPAT_VERSION")}")
    // MainActivity の WindowInsets リスナー（ViewCompat / WindowInsetsCompat /
    // Insets、Issue #579）が androidx.core を直接 import するため明示依存にする
    // （バージョン選定根拠は versions.env の ANDROIDX_CORE_VERSION コメント参照）。
    implementation("androidx.core:core:${androidVersion("ANDROIDX_CORE_VERSION")}")
    // MainActivity のカメラ Intent 中継（Phase 8、Issue #750）が
    // androidx.activity.result.{ActivityResultLauncher,contract.ActivityResultContracts} を
    // 直接 import するため明示依存にする（バージョン選定根拠は versions.env の
    // ANDROIDX_ACTIVITY_VERSION コメント参照）。
    implementation("androidx.activity:activity:${androidVersion("ANDROIDX_ACTIVITY_VERSION")}")
    // MainActivity の生体認証中継（`BiometricPrompt`/`BiometricManager`、Phase 11、
    // Issue #908）が androidx.biometric を直接 import するため明示依存にする
    // （バージョン選定根拠は versions.env の BIOMETRIC_VERSION コメント参照）。
    implementation("androidx.biometric:biometric:${androidVersion("BIOMETRIC_VERSION")}")
    // MainActivity の資格情報中継（`EncryptedSharedPreferences`/`MasterKey`、Phase 11、
    // Issue #924）が androidx.security.crypto を直接 import するため明示依存にする
    // （バージョン選定根拠は versions.env の SECURITY_CRYPTO_VERSION コメント参照）。
    implementation("androidx.security:security-crypto:${androidVersion("SECURITY_CRYPTO_VERSION")}")
    // MainActivity のバックグラウンド処理中継（`WorkManager`/`OneTimeWorkRequest`、
    // Phase 11、Issue #1046）が androidx.work を直接 import するため明示依存にする
    // （バージョン選定根拠は versions.env の WORK_RUNTIME_VERSION コメント参照）。
    implementation("androidx.work:work-runtime:${androidVersion("WORK_RUNTIME_VERSION")}")
}
