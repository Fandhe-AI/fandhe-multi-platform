// Android アプリモジュール（TASK-11.2-S2 / REQ-11、Issue #226。
// Activity 方式は TASK-9.3-S1 / REQ-9 / M4、Issue #242 で GameActivity へ変更。
// 決定記録: docs/architecture/android-activity-method.md）。
//
// GameActivity 方式（NativeActivity から変更）。実描画は `fandhe-gallery`
// （crates/fandhe-gallery/src/lib.rs の android_main。端末機能は fandhe-egui-proxy）が cdylib（`libfandhe_gallery.so`）として
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
// ANDROID_MIN_API（24）と一致させること（SSOT はそちら）。games-activity /
// appcompat のバージョンは同ファイルの GAMES_ACTIVITY_VERSION / APPCOMPAT_VERSION と
// 一致させること。
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

            // 共通層 fandhe-platform-android の Java（RequestTracker・FeatureLifecycleProvider）。機能 crate の
            // helper が同じパッケージから使う。複製せず crate の置き場を直接読む。
            @Suppress("DEPRECATION")
            java.srcDirs("../../../crates/fandhe-platform-android/android/java")

            // 機能 crate が持つ Java・res（宣言の `[package.metadata.fandhe.android] java` / `res`）。目印の間は
            // `node scripts/device/migrate-capability.mjs sync` が書く（#1183）。cargo-fandhe の突き合わせテストが、
            // 宣言とここの srcDirs の一致を確かめる。
            // BEGIN fandhe-cap（`node scripts/device/migrate-capability.mjs sync` が書き直す。手で編集しない）
            @Suppress("DEPRECATION")
            java.srcDirs("../../../crates/fandhe-cap-camera-capture/android/java")
            @Suppress("DEPRECATION")
            java.srcDirs("../../../crates/fandhe-cap-device-identifier/android/java")
            @Suppress("DEPRECATION")
            java.srcDirs("../../../crates/fandhe-cap-document-scan/android/java")
            @Suppress("DEPRECATION")
            java.srcDirs("../../../crates/fandhe-cap-home-widget/android/java")
            @Suppress("DEPRECATION")
            res.srcDirs("../../../crates/fandhe-cap-home-widget/android/res")
            @Suppress("DEPRECATION")
            java.srcDirs("../../../crates/fandhe-cap-nfc-card-emulation/android/java")
            @Suppress("DEPRECATION")
            res.srcDirs("../../../crates/fandhe-cap-nfc-card-emulation/android/res")
            @Suppress("DEPRECATION")
            java.srcDirs("../../../crates/fandhe-cap-pedometer/android/java")
            @Suppress("DEPRECATION")
            java.srcDirs("../../../crates/fandhe-cap-power-state/android/java")
            @Suppress("DEPRECATION")
            java.srcDirs("../../../crates/fandhe-cap-share-receive/android/java")
            // END fandhe-cap
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
// `fandhe_device::deep_link::validate_host`。ずれても Rust が拒否する側（fail-closed）に倒れる。
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
    // WebAuthSessionHelper の Custom Tabs（`CustomTabsIntent`、Issue #534）が
    // androidx.browser を直接 import するため明示依存にする
    // （バージョン選定根拠は versions.env の ANDROIDX_BROWSER_VERSION コメント参照）。
    implementation("androidx.browser:browser:${androidVersion("ANDROIDX_BROWSER_VERSION")}")
    // FoldablePostureHelper の折りたたみ姿勢（`WindowInfoTracker` の `FoldingFeature`、
    // `WindowInfoTrackerCallbackAdapter`）が androidx.window を直接 import するため明示依存にする
    // （バージョン選定根拠は versions.env の WINDOW_VERSION コメント参照）。
    implementation("androidx.window:window:${androidVersion("WINDOW_VERSION")}")
    implementation("androidx.window:window-java:${androidVersion("WINDOW_VERSION")}")
    // CarDisplayService（car-display）が Car App Library の `CarAppService`・`Screen`・`PaneTemplate` を
    // 直接 import するため明示依存にする（バージョン選定根拠は versions.env の CAR_APP_VERSION コメント参照）。
    implementation("androidx.car.app:app:${androidVersion("CAR_APP_VERSION")}")
    // Car App Library の推移依存 guava 31.1-android には OSV の勧告があるため、勧告のない版へ引き上げる
    // （選定根拠は versions.env の GUAVA_VERSION コメント参照）。
    constraints {
        implementation("com.google.guava:guava:${androidVersion("GUAVA_VERSION")}") {
            because("guava 31.1-android に GHSA-5mg8-w23w-74h3・GHSA-7g45-4rm6-3mm3・GHSA-xxph-c9ww-hj94")
        }
    }
    // Google Play services（GMS が使えない端末では各機能が Unsupported を返す）。
    // activity-recognition は遷移 API（`ActivityRecognition`）、document-scan は ML Kit Document Scanner、
    // payment-request は Google Pay API（`PaymentsClient`）を直接 import する予定のため明示依存にする
    // （バージョン選定根拠は versions.env の PLAY_SERVICES_* コメント参照）。
    // passkey-signin は Credential Manager（`CredentialManager`）を直接 import するため明示依存にする。
    // API 34 未満は credentials-play-services-auth が Play services 経由で実体を提供する
    // （バージョン選定根拠は versions.env の CREDENTIALS_VERSION コメント参照）。
    implementation("androidx.credentials:credentials:${androidVersion("CREDENTIALS_VERSION")}")
    implementation("androidx.credentials:credentials-play-services-auth:${androidVersion("CREDENTIALS_VERSION")}")
    implementation("com.google.android.gms:play-services-location:${androidVersion("PLAY_SERVICES_LOCATION_VERSION")}")
    implementation("com.google.android.gms:play-services-wallet:${androidVersion("PLAY_SERVICES_WALLET_VERSION")}")
    implementation("com.google.android.gms:play-services-mlkit-document-scanner:${androidVersion("PLAY_SERVICES_MLKIT_DOCUMENT_SCANNER_VERSION")}")
}
