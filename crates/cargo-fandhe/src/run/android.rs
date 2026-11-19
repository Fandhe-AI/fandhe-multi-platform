//! `cargo fandhe run|build --os android` の手順（`run/mod.rs` の [`prepare`](super::prepare) から呼ばれる、Issue #1423）。
//!
//! 流れは gen（Android の分だけ）→ 殻の生成 → `cargo ndk`（`.so` を殻の `jniLibs` へ）→ `gradle` →
//! （run は）`adb install`・`am start -W`・`pidof` で起動の確認（`am start` は失敗でも 0 を返すため）。
//! 殻は `<target_dir>/fandhe/<package>/android/` に置き、gen もその下の `gen/` へ出す（アプリの `gen/` には触れない。
//! 他 OS の生成物を消さないため）。殻の `app/src` は毎回作り直してから書く（前回の ABI の `.so` や外した設定の
//! ファイルが APK に混ざらないように）。Gradle の `build/`・`.gradle/` は消さない。
//! アプリの `eject/android/shell/` は殻を書いたあと cargo-ndk の前に重ねる（同じパスは置き換え、殻に無いファイルは足す）。
//! `app/src` の外に足したファイルは、重ねた一覧を殻のルートの `.fandhe-overlay` に控え、次回の最初に消す。
//!
//! 殻の版（AGP・SDK・minSdk 等）は `platforms/android` と `scripts/android/versions.env` に揃える（`tests/run.rs` が突き合わせる）。
//! MainActivity は `GameActivity` の継承と `System.loadLibrary(FandheLibrary.NAME)` だけ
//! （`docs/architecture/consumer-app-structure.md` §3）。Gradle wrapper は同梱しない（wrapper jar のバイナリを避ける。
//! `platforms/android` と同じく PATH の `gradle` を使う）。署名の値は環境変数のまま子プロセスへ継承し、引数にも殻にも載せない。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{output_of, Action, App, Env, Op, Options, Os, Step};
use crate::native::{esc_xml, shell_overlay_dir, MAIN_ACTIVITY, SHELL_APPLICATION_ATTRIBUTES};

pub(crate) const AGP_VERSION: &str = "9.3.1";
pub(crate) const COMPILE_SDK: u32 = 35;
pub(crate) const TARGET_SDK: u32 = 35;
pub(crate) const MIN_SDK: u32 = 24;
pub(crate) const BUILD_TOOLS: &str = "36.0.0";
/// 署名に要る環境変数（`--release` は全部そろっていなければ止まる。値は読まず、非空かだけ見る）。
pub(crate) const SIGNING_VARS: [&str; 3] = [
    "FANDHE_ANDROID_KEYSTORE",
    "FANDHE_ANDROID_KEYSTORE_PASSWORD",
    "FANDHE_ANDROID_KEY_ALIAS",
];
/// `build` が作る ABI と Rust の triple（`platforms/android` の abiFilters と同じ 3 つ）。
pub(crate) const BUILD_ABIS: [(&str, &str); 3] = [
    ("arm64-v8a", "aarch64-linux-android"),
    ("armeabi-v7a", "armv7-linux-androideabi"),
    ("x86_64", "x86_64-linux-android"),
];

/// 端末の ABI（`ro.product.cpu.abi`）から cargo-ndk の ABI 名と triple を引く。
fn device_target(abi: &str) -> Option<(&'static str, &'static str)> {
    BUILD_ABIS
        .into_iter()
        .chain([("x86", "i686-linux-android")])
        .find(|(a, _)| *a == abi)
}

/// Android の環境のスナップショット。値そのもの（パスワード等）は持たない。
#[derive(Clone, Debug, Default)]
pub struct AndroidEnv {
    /// `ANDROID_HOME`（または `ANDROID_SDK_ROOT`）が実在するディレクトリを指している。
    pub sdk: bool,
    /// NDK がある（`ANDROID_NDK_HOME` か `<SDK>/ndk/*`）。
    pub ndk: bool,
    /// adb が `device` 状態で認識している端末のシリアル（`run` のときだけ調べる）。
    pub devices: Vec<String>,
    /// 環境変数 `ANDROID_SERIAL`。
    pub serial_env: Option<String>,
    /// 使う端末の `ro.product.cpu.abi`。
    pub device_abi: Option<String>,
    /// `SIGNING_VARS` のうち、非空で設定されているもの。
    pub signing: BTreeSet<String>,
    /// `FANDHE_ANDROID_KEYSTORE` を絶対パスにしたもの（Gradle はカレントが違うため。パスは秘密ではない）。
    pub keystore: Option<PathBuf>,
    /// `keystore` が実在する通常ファイルか。
    pub keystore_file: bool,
}

impl AndroidEnv {
    /// 使う端末。`ANDROID_SERIAL` があればそれ、無ければ 1 台だけのとき。
    fn chosen(&self) -> Result<&str, String> {
        match (&self.serial_env, self.devices.as_slice()) {
            (Some(s), d) if d.contains(s) => Ok(s),
            (Some(_), _) => Err("環境変数 ANDROID_SERIAL の端末が adb に見つかりません。`adb devices` で確認してください".into()),
            (None, []) => Err("adb につながった端末がありません。端末を接続するかエミュレータを起動してください（自動では起動しません）".into()),
            (None, [one]) => Ok(one),
            (None, _) => Err("adb につながった端末が複数あります。環境変数 ANDROID_SERIAL に使う端末のシリアルを設定してください".into()),
        }
    }
}

/// `adb devices` の出力から、`device` 状態（`offline`・`unauthorized` を除く）の端末のシリアルを取り出す。
pub fn parse_adb_devices(out: &str) -> Vec<String> {
    out.lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|l| {
            let mut c = l.split_whitespace();
            let (serial, state) = (c.next()?, c.next()?);
            (state == "device").then(|| serial.to_string())
        })
        .collect()
}

/// 実環境を観測する。`commands` は PATH 上にあったコマンド名。
pub(super) fn detect(commands: &BTreeSet<String>, action: Action) -> AndroidEnv {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    let sdk_dir = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .into_iter()
        .filter_map(var)
        .map(PathBuf::from)
        .find(|p| p.is_dir());
    // `<SDK>/ndk/` 直下のディレクトリ（NDK の版ごと）だけを数える。
    let ndk = var("ANDROID_NDK_HOME").is_some_and(|p| Path::new(&p).is_dir())
        || sdk_dir.as_ref().is_some_and(|s| {
            std::fs::read_dir(s.join("ndk")).is_ok_and(|mut d| {
                d.any(|e| e.is_ok_and(|e| e.file_type().is_ok_and(|t| t.is_dir())))
            })
        });
    let keystore = var("FANDHE_ANDROID_KEYSTORE").and_then(|p| std::path::absolute(p).ok());
    let mut a = AndroidEnv {
        sdk: sdk_dir.is_some(),
        ndk,
        serial_env: var("ANDROID_SERIAL").and_then(|s| s.into_string().ok()),
        signing: SIGNING_VARS
            .into_iter()
            .filter(|k| var(k).is_some())
            .map(String::from)
            .collect(),
        keystore_file: keystore.as_ref().is_some_and(|p| p.is_file()),
        keystore,
        ..AndroidEnv::default()
    };
    if action == Action::Run && commands.contains("adb") {
        a.devices = output_of("adb", &["devices"])
            .map(|o| parse_adb_devices(&o))
            .unwrap_or_default();
        if let Ok(serial) = a.chosen() {
            a.device_abi = output_of(
                "adb",
                &["-s", serial, "shell", "getprop", "ro.product.cpu.abi"],
            )
            .map(|o| o.trim().to_string());
        }
    }
    a
}

/// Gradle の起動コマンド。Windows の gradle は `gradle.bat`（`Command` は `.bat` を補わない）。
fn gradle_cmd(host: Option<Os>) -> &'static str {
    if host == Some(Os::Windows) {
        "gradle.bat"
    } else {
        "gradle"
    }
}

/// 前提を全件検査し、問題がなければ手順を返す。`dir` は殻（Gradle プロジェクト）の置き場。
pub(super) fn prepare(
    app: &App,
    opts: &Options,
    env: &Env,
    dir: &Path,
) -> Result<Vec<Op>, Vec<String>> {
    let a = &env.android;
    let run = opts.action == Action::Run;
    let mut problems = Vec::new();
    if app.cdylib.is_none() {
        problems.push("lib の crate-type に cdylib がありません。`[lib]` の `crate-type` に \"cdylib\" を足してください".to_string());
    }
    if app.application_id.is_none() {
        problems.push("applicationId がありません。`[package.metadata.fandhe-app.android]` の `application-id` に `com.example.app` の形で書いてください".to_string());
    }
    if !a.sdk {
        problems.push("Android SDK が見つかりません。環境変数 ANDROID_HOME（または ANDROID_SDK_ROOT）を SDK のディレクトリに設定してください".to_string());
    }
    if !a.ndk {
        problems.push("Android NDK が見つかりません。環境変数 ANDROID_NDK_HOME を設定するか、SDK の `ndk/` に NDK を入れてください".to_string());
    }
    let mut tools = vec![
        (
            "cargo-ndk",
            "`cargo install cargo-ndk --locked` を実行してください",
        ),
        (
            gradle_cmd(env.host),
            "Gradle を入れて PATH に通してください",
        ),
        ("java", "JDK（17 以上）を入れて PATH に通してください"),
    ];
    if run {
        tools.push((
            "adb",
            "Android SDK の platform-tools を PATH に通してください",
        ));
    }
    for (cmd, fix) in tools {
        if !env.commands.contains(cmd) {
            problems.push(format!("{cmd} がありません。{fix}"));
        }
    }

    // run は端末の ABI だけ、build は 3 ABI。
    let mut targets: Vec<(&str, &str)> = Vec::new();
    if run {
        match a.chosen() {
            Ok(_) => {
                match a.device_abi.as_deref().map(|b| (b, device_target(b))) {
                    Some((_, Some(t))) => targets.push(t),
                    Some((b, None)) => problems.push(format!("端末の ABI {b} には対応していません")),
                    None => problems.push("端末の ABI を取得できません。`adb devices` で端末が `device` 状態か確認してください".to_string()),
                }
            }
            Err(e) => problems.push(e),
        }
    } else {
        targets.extend(BUILD_ABIS);
    }
    if let Some(t) = &env.installed_targets {
        for (_, triple) in &targets {
            if !t.contains(*triple) {
                problems.push(format!(
                    "Rust の target {triple} がありません。`rustup target add {triple}` を実行してください"
                ));
            }
        }
    }
    if opts.release {
        let missing: Vec<&str> = SIGNING_VARS
            .into_iter()
            .filter(|k| !a.signing.contains(*k))
            .collect();
        if a.signing.contains(SIGNING_VARS[0]) && !a.keystore_file {
            problems.push(format!(
                "環境変数 {} が指すファイルがありません。keystore のパスを確認してください",
                SIGNING_VARS[0]
            ));
        }
        if !missing.is_empty() {
            problems.push(format!(
                "--release は署名が必要です。環境変数 {} を設定してください（無署名では成功させません）",
                missing.join("・")
            ));
        }
    }
    if app.android_gen.is_none() {
        problems.push(
            "Android の gen 出力がありません（load_app を --os android で呼んでください）"
                .to_string(),
        );
    }
    let (Some(id), Some(files), true) =
        (&app.application_id, &app.android_gen, problems.is_empty())
    else {
        return Err(problems);
    };
    Ok(plan(app, opts, env, dir, id, files, &targets))
}

fn plan(
    app: &App,
    opts: &Options,
    env: &Env,
    dir: &Path,
    id: &str,
    files: &BTreeMap<String, String>,
    targets: &[(&str, &str)],
) -> Vec<Op> {
    // gen の出力は殻の下に置く（アプリの `gen/` は書き換えない）。
    let gen_dir = dir.join("gen");
    let within = app.target_dir.clone();
    let jni = dir.join("app/src/main/jniLibs");
    // 殻のソース（`app/src`。jniLibs を含む）を作り直してから書く。Gradle の `build/`・`.gradle/` は残す。
    let mut ops = vec![
        // `app/src` の外に重ねたファイル（`google-services.json` 等）は Reset で消えないため、前回の分を控えから消す。
        // 先頭に置くのは、控えが gen/ 配下を指していても、後の Gen が書き直して生成物を残すため。
        Op::Prune {
            dir: dir.to_path_buf(),
            within: within.clone(),
        },
        Op::Gen {
            dir: gen_dir.clone(),
            within: within.clone(),
            files: files.clone(),
        },
        Op::Reset {
            dir: dir.join("app/src"),
            within: within.clone(),
        },
    ];
    for (rel, body) in shell_files(app, id) {
        ops.push(Op::Write {
            path: dir.join(rel),
            within: within.clone(),
            body,
        });
    }
    // 殻のあと・cargo-ndk の前に、`eject/android/shell/` を殻へ重ねる（同じパスは置き換え、殻に無いものは足す）。
    if !app.shell_overlay.is_empty() {
        ops.push(Op::Overlay {
            from: shell_overlay_dir(&app.dir),
            to: dir.to_path_buf(),
            within: within.clone(),
            files: app.shell_overlay.clone(),
        });
    }

    let mut ndk: Vec<String> = vec!["ndk".into()];
    for (abi, _) in targets {
        ndk.extend(["-t".to_string(), abi.to_string()]);
    }
    ndk.extend([
        "--platform".into(),
        MIN_SDK.to_string(),
        "-o".into(),
        jni.display().to_string(),
        "build".into(),
        "--lib".into(),
    ]);
    if opts.release {
        ndk.push("--release".into());
    }
    ndk.extend(["--manifest-path".into(), app.manifest.display().to_string()]);
    ops.push(Op::Spawn(Step {
        stage: "cargo-ndk",
        program: env.cargo.clone(),
        args: ndk,
        env: Vec::new(),
    }));

    // build --release はストア向けの .aab、それ以外は端末へ入れられる .apk。
    let aab = opts.release && opts.action == Action::Build;
    let (task, artifact) = match (opts.release, aab) {
        (false, _) => (
            ":app:assembleDebug",
            "app/build/outputs/apk/debug/app-debug.apk",
        ),
        (true, true) => (
            ":app:bundleRelease",
            "app/build/outputs/bundle/release/app-release.aab",
        ),
        (true, false) => (
            ":app:assembleRelease",
            "app/build/outputs/apk/release/app-release.apk",
        ),
    };
    // keystore は Gradle のカレントに依らないよう絶対パスで渡す（値は秘密ではないパス。パスワードは継承のまま）。
    let keystore_env = env
        .android
        .keystore
        .iter()
        .filter(|_| opts.release)
        .map(|p| {
            (
                "FANDHE_ANDROID_KEYSTORE".to_string(),
                p.clone().into_os_string(),
            )
        })
        .collect();
    ops.push(Op::Spawn(Step {
        stage: "gradle",
        program: gradle_cmd(env.host).into(),
        args: vec![
            "-p".into(),
            dir.display().to_string(),
            task.into(),
            "--console=plain".into(),
        ],
        env: keystore_env,
    }));
    let artifact = dir.join(artifact);
    ops.push(Op::Say(format!("Android の成果物: {}", artifact.display())));
    if let (Action::Run, Ok(serial)) = (opts.action, env.android.chosen()) {
        let adb = |stage, args: &[&str]| {
            let mut a = vec!["-s".to_string(), serial.to_string()];
            a.extend(args.iter().map(|s| s.to_string()));
            Op::Spawn(Step {
                stage,
                program: "adb".into(),
                args: a,
                env: Vec::new(),
            })
        };
        let apk = artifact.display().to_string();
        ops.push(adb("adb-install", &["install", "-r", &apk]));
        ops.push(adb(
            "adb-start",
            &[
                "shell",
                "am",
                "start",
                "-W",
                "-n",
                &format!("{id}/{MAIN_ACTIVITY}"),
            ],
        ));
        // `am start` は起動に失敗しても 0 を返すため、プロセスがあることで確かめる（無ければ pidof が非 0）。
        ops.push(adb("adb-check", &["shell", "pidof", id]));
    }
    ops
}

/// Kotlin の文字列リテラルの中身（`\`・`"`・`$` をエスケープ）。
fn kt(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
}

/// 殻の Manifest。`<application>` は殻の既定に `application-attributes` をキー単位で重ねる（既定の置き換え・追加）。
/// MainActivity の宣言・LAUNCHER・権限・バックアップ規則は gen の Manifest を overlay で重ねる。
fn shell_manifest(app: &App) -> String {
    let mut attrs: BTreeMap<&str, &str> = SHELL_APPLICATION_ATTRIBUTES.iter().copied().collect();
    attrs.extend(
        app.application_attributes
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str())),
    );
    let attrs: String = attrs
        .iter()
        .map(|(k, v)| format!("\n        android:{k}=\"{}\"", esc_xml(v)))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- cargo fandhe run|build が生成。MainActivity の宣言・LAUNCHER・権限は gen の Manifest を overlay で重ねる -->\n<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n    <application{attrs} />\n</manifest>\n"
    )
}

/// 殻のファイル（殻のルートからの相対パス → 内容）。
/// `app/build.gradle.kts` は gen の出力を相対パスで参照する（殻・gen はどちらも殻のルート配下。eject で持ち出しても
/// target の場所に依らない）。
fn shell_files(app: &App, id: &str) -> Vec<(&'static str, String)> {
    vec![
        ("settings.gradle.kts", SETTINGS.to_string()),
        (
            "build.gradle.kts",
            format!("// cargo fandhe run|build が生成。編集しない\nplugins {{\n    id(\"com.android.application\") version \"{AGP_VERSION}\" apply false\n}}\n"),
        ),
        ("gradle.properties", GRADLE_PROPERTIES.to_string()),
        (
            "app/build.gradle.kts",
            format!(
                r#"// cargo fandhe run|build が生成。編集しない
plugins {{
    id("com.android.application")
}}

android {{
    namespace = "com.fandhe.platform"
    compileSdk = {COMPILE_SDK}
    buildToolsVersion = "{BUILD_TOOLS}"

    defaultConfig {{
        applicationId = "{id}"
        minSdk = {MIN_SDK}
        targetSdk = {TARGET_SDK}
        versionCode = 1
        versionName = "{ver}"
    }}

    compileOptions {{
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }}

    // 署名は環境変数から読む（値は殻にも引数にも書かない）。--release の前提検査は cargo-fandhe が行う。
    val keystorePath = System.getenv("FANDHE_ANDROID_KEYSTORE")
    if (!keystorePath.isNullOrEmpty()) {{
        val storePassword = System.getenv("FANDHE_ANDROID_KEYSTORE_PASSWORD")
        val envKeyPassword = System.getenv("FANDHE_ANDROID_KEY_PASSWORD")
        signingConfigs {{
            create("fandhe") {{
                storeFile = file(keystorePath)
                this.storePassword = storePassword
                keyAlias = System.getenv("FANDHE_ANDROID_KEY_ALIAS")
                keyPassword = if (envKeyPassword.isNullOrEmpty()) storePassword else envKeyPassword
            }}
        }}
    }}

    buildTypes {{
        release {{
            isMinifyEnabled = false
            if (!keystorePath.isNullOrEmpty()) {{
                signingConfig = signingConfigs.getByName("fandhe")
            }}
        }}
    }}
}}

// 機能 crate の Java・res・Manifest の overlay・Gradle 依存（GameActivity 等）は `cargo fandhe gen` の出力から取り込む。
apply(from = "{GEN_GRADLE}")
"#,
                id = kt(id),
                ver = kt(&app.version),
            ),
        ),
        ("app/src/main/AndroidManifest.xml", shell_manifest(app)),
        (
            "app/src/main/java/com/fandhe/platform/MainActivity.java",
            "// cargo fandhe run|build が生成。編集しない\npackage com.fandhe.platform;\n\nimport com.google.androidgamesdk.GameActivity;\n\npublic class MainActivity extends GameActivity {\n    static {\n        System.loadLibrary(FandheLibrary.NAME);\n    }\n}\n".to_string(),
        ),
        (
            "app/src/main/res/values/themes.xml",
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- GameActivity は AppCompatActivity のため Theme.AppCompat 系が要る -->\n<resources>\n    <style name=\"FandheAppTheme\" parent=\"Theme.AppCompat.NoActionBar\" />\n</resources>\n".to_string(),
        ),
        (
            "app/src/main/res/values/strings.xml",
            format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n    <string name=\"fandhe_app_name\">{}</string>\n</resources>\n", esc_xml(&app.name)),
        ),
    ]
}

/// 殻の `app/` から見た gen の Gradle 設定（殻のルートの `gen/`、`plan` の `Op::Gen` の出力先）。
const GEN_GRADLE: &str = "../gen/android/fandhe.gradle";

/// `platforms/android/settings.gradle.kts` と同じ解決先（google・mavenCentral・gradlePluginPortal だけ）。
const SETTINGS: &str = r#"// cargo fandhe run|build が生成。編集しない
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

rootProject.name = "fandhe-app"

include(":app")
"#;

const GRADLE_PROPERTIES: &str = "# cargo fandhe run|build が生成。編集しない\nandroid.useAndroidX=true\norg.gradle.jvmargs=-Xmx2048m\n";
