//! `cargo fandhe gen` の生成物のテスト（Issue #1176）。
//!
//! 1. カタログアプリ（fandhe-gallery）の生成結果が、全機能の宣言とアプリ設定どおりで、殻（platforms/）に
//!    手書きの宣言が残っていないことを確かめる（各 OS の設定は生成物だけを使う、#1194）。
//! 2. 一時 workspace で実際の cargo を通し、宣言・入力の不正の拒否と到達判定（他 member の feature の
//!    混入なし・対象 OS に限った依存）を確かめる。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use cargo_fandhe::declaration::{Component, IntentData, IntentFilter, MetaData};
use cargo_fandhe::{check_workspace, generate, generate_web};
use serde_json::Value;

fn repo(rel: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel),
    )
    .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// gallery の生成結果（全テストで 1 回だけ生成する）。
fn gallery() -> &'static BTreeMap<String, String> {
    static G: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
    G.get_or_init(|| {
        let m = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fandhe-gallery/Cargo.toml");
        generate(Some(&m)).unwrap_or_else(|e| panic!("{e:?}")).files
    })
}

// ---- XML の読み取り（テスト用の簡易版）----

type Attrs = BTreeMap<String, String>;
type Tag = (String, Attrs);

fn strip_comments(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<!--") {
        out += &rest[..i];
        rest = &rest[i..];
        rest = &rest[rest.find("-->").expect("コメントが閉じていない") + 3..];
    }
    out + rest
}

/// タグを出現順に (名前, 属性) へ分ける。閉じタグは名前が `/` で始まり、自己閉じは閉じタグを補う。
fn tags(xml: &str) -> Vec<Tag> {
    let xml = strip_comments(xml);
    let mut v = vec![];
    for chunk in xml.split('<').skip(1) {
        let Some((body, _)) = chunk.split_once('>') else {
            continue;
        };
        if body.starts_with('?') {
            continue;
        }
        let selfclose = body.ends_with('/');
        let body = body.trim_end_matches('/');
        let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
        let mut attrs = Attrs::new();
        let mut rest = &body[name_end..];
        while let Some((k, r)) = rest.split_once("=\"") {
            let (val, r) = r.split_once('"').expect("属性値が閉じていない");
            attrs.insert(
                k.trim().trim_start_matches("android:").to_string(),
                val.to_string(),
            );
            rest = r;
        }
        let name = body[..name_end].to_string();
        v.push((name.clone(), attrs));
        if selfclose {
            v.push((format!("/{name}"), Attrs::new()));
        }
    }
    v
}

fn parse_filter(it: &mut std::slice::Iter<Tag>) -> IntentFilter {
    let mut f = IntentFilter {
        actions: vec![],
        categories: vec![],
        data: vec![],
    };
    for (n, a) in it.by_ref() {
        match n.as_str() {
            "/intent-filter" => break,
            "action" => f.actions.push(a["name"].clone()),
            "category" => f.categories.push(a["name"].clone()),
            "data" => f.data.push(IntentData {
                scheme: a.get("scheme").cloned(),
                host: a.get("host").cloned(),
                mime_type: a.get("mimeType").cloned(),
                path_prefix: a.get("pathPrefix").cloned(),
            }),
            other if other.starts_with('/') => {}
            other => panic!("想定外のタグ {other}"),
        }
    }
    f
}

/// Manifest から宣言と同じ `kind`・`name` の要素を、宣言と同じ型で取り出す。
fn manifest_component(t: &[Tag], c: &Component) -> Component {
    let kind = c.kind.tag().to_string();
    let start = t
        .iter()
        .position(|(n, a)| *n == kind && a.get("name") == Some(&c.name))
        .unwrap_or_else(|| panic!("Manifest に {} がない", c.name));
    let mut attrs = t[start].1.clone();
    let exported = attrs.remove("exported").expect("exported がない") == "true";
    attrs.remove("name");
    let target_activity = attrs.remove("targetActivity");
    let mut out = Component {
        kind: c.kind.clone(),
        name: c.name.clone(),
        exported,
        target_activity,
        attributes: attrs,
        intent_filters: vec![],
        meta_data: vec![],
    };
    let close = format!("/{kind}");
    let mut it = t[start + 1..].iter();
    while let Some((n, a)) = it.next() {
        if *n == close {
            break;
        }
        match n.as_str() {
            "intent-filter" => out.intent_filters.push(parse_filter(&mut it)),
            "meta-data" => out.meta_data.push(MetaData {
                name: a["name"].clone(),
                resource: a.get("resource").cloned(),
                value: a.get("value").cloned(),
            }),
            _ => {}
        }
    }
    out
}

/// MainActivity の intent-filter を出現順に返す。
fn main_activity_filters(t: &[Tag]) -> Vec<IntentFilter> {
    let Some(main) = t
        .iter()
        .position(|(n, a)| n == "activity" && a["name"].ends_with(".MainActivity"))
    else {
        return vec![];
    };
    let mut it = t[main + 1..].iter();
    let mut out = vec![];
    while let Some((n, _)) = it.next() {
        match n.as_str() {
            "/activity" => break,
            "intent-filter" => out.push(parse_filter(&mut it)),
            _ => {}
        }
    }
    out
}

// ---- 1. gallery の生成結果と現行設定の比較 ----

/// カタログアプリの殻（platforms/android）は application の属性だけを持ち、機能の宣言を手で書かない（#1194）。
/// Java・res・依存は gen の `fandhe.gradle` を取り込むだけで、build.gradle.kts に機能ごとの行を書かない。
#[test]
fn android_shell_has_no_hand_written_declarations() {
    let shell = tags(&repo("platforms/android/app/src/main/AndroidManifest.xml"));
    let kinds: BTreeSet<&str> = shell
        .iter()
        .map(|(k, _)| k.trim_start_matches('/'))
        .collect();
    assert_eq!(kinds, BTreeSet::from(["manifest", "application"]));
    let kts = repo("platforms/android/app/build.gradle.kts");
    assert!(
        kts.contains("gen/android/fandhe.gradle") && kts.contains("apply(from = fandheGradle)")
    );
    for l in kts.lines().filter(|l| !l.trim_start().starts_with("//")) {
        assert!(!l.contains("implementation("), "{l}");
        assert!(!l.contains("srcDirs(") || l.contains("jniLibs"), "{l}");
    }
}

#[test]
fn android_manifest_overlay_covers_all_declarations() {
    let gen = tags(&gallery()["android/AndroidManifest.xml"]);
    let m = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let decls = check_workspace(Some(&m)).unwrap();
    // カタログは全機能を載せるので、全宣言の権限・feature・コンポーネントが生成側にある。
    let mut n = 0;
    for a in decls.iter().filter_map(|d| d.android.as_ref()) {
        for p in &a.permissions {
            assert!(
                gen.iter()
                    .any(|(k, m)| k == "uses-permission" && m["name"] == p.name),
                "{}",
                p.name
            );
        }
        for f in &a.features {
            assert!(
                gen.iter().any(|(k, m)| k == "uses-feature"
                    && m["name"] == f.name
                    && m["required"] == f.required.to_string()),
                "{}",
                f.name
            );
        }
        for c in &a.components {
            assert_eq!(manifest_component(&gen, c).name, c.name);
            n += 1;
        }
    }
    assert!(n > 5, "{n}");

    // MainActivity の SEND の filter（share-receive）と `fandhe` scheme の VIEW（url-scheme と web-auth-session が
    // 同じ filter を宣言し、統合規則で 1 つになる、#1333）、MAIN/LAUNCHER（殻の値として gen が出す）の 3 つ。
    let filters = main_activity_filters(&gen);
    let has = |action: &str| {
        filters
            .iter()
            .filter(|f| f.actions.iter().any(|a| a == action))
            .count()
    };
    assert_eq!(has("android.intent.action.SEND"), 1);
    assert_eq!(has("android.intent.action.MAIN"), 1);
    assert_eq!(
        filters
            .iter()
            .filter(|f| f.data.iter().any(|d| d.scheme.as_deref() == Some("fandhe")))
            .count(),
        1
    );
    assert_eq!(filters.len(), 3);

    // MainActivity の lib_name はアプリの cdylib 名。
    let (_, lib) = gen
        .iter()
        .find(|(n, a)| n == "meta-data" && a["name"] == "android.app.lib_name")
        .unwrap();
    assert_eq!(lib["value"], "fandhe_gallery");
    assert!(
        gallery()["android/java/com/fandhe/platform/FandheLibrary.java"]
            .contains("NAME = \"fandhe_gallery\"")
    );
}

/// `application-meta-data`（`<application>` 直下の `<meta-data>`）を宣言する機能は、生成側と現行 Manifest の
/// 宣言どおりの属性で出る（第 15 束 #1335 で car-display・payment-request、#1194 で managed-app-config が使い始めた）。
#[test]
fn application_meta_data_matches_declarations() {
    let gen = tags(&gallery()["android/AndroidManifest.xml"]);
    let find = |t: &[Tag], name: &str| -> Option<Attrs> {
        t.iter()
            .find(|(k, a)| k == "meta-data" && a.get("name").map(String::as_str) == Some(name))
            .map(|(_, a)| a.clone())
    };
    let m = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let mut n = 0;
    for d in check_workspace(Some(&m)).unwrap() {
        for md in d.android.iter().flat_map(|a| &a.application_meta_data) {
            let g =
                find(&gen, &md.name).unwrap_or_else(|| panic!("{} が生成されていない", md.name));
            assert_eq!(g.get("value"), md.value.as_ref(), "{}", md.name);
            assert_eq!(g.get("resource"), md.resource.as_ref(), "{}", md.name);
            n += 1;
        }
    }
    assert_eq!(
        n, 4,
        "car-display の 2 件・payment-request の 1 件・managed-app-config の 1 件"
    );
}

/// `fandhe.gradle` の `java.srcDirs += [...]` などの引用符内を取り出す。
fn gradle_dirs(gradle: &str, kind: &str) -> BTreeSet<String> {
    let line = gradle
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("{kind}.srcDirs +=")))
        .unwrap_or_else(|| panic!("{kind}.srcDirs がない"));
    line.split('\'')
        .skip(1)
        .step_by(2)
        // crate の絶対パスを `crates/...` からの相対へ揃える。
        .map(|p| p[p.rfind("crates/fandhe-").expect("crate のパス")..].to_string())
        .collect()
}

/// Gradle 依存の版の選定根拠は `scripts/android/versions.env` に残す。宣言（`gradle`）が正で、ここで両者の一致を確かめる。
const GRADLE_VERSION_KEYS: &[(&str, &str)] = &[
    ("androidx.games:games-activity", "GAMES_ACTIVITY_VERSION"),
    ("androidx.appcompat:appcompat", "APPCOMPAT_VERSION"),
    ("androidx.core:core", "ANDROIDX_CORE_VERSION"),
    ("androidx.activity:activity", "ANDROIDX_ACTIVITY_VERSION"),
    ("androidx.biometric:biometric", "BIOMETRIC_VERSION"),
    (
        "androidx.security:security-crypto",
        "SECURITY_CRYPTO_VERSION",
    ),
    ("androidx.work:work-runtime", "WORK_RUNTIME_VERSION"),
    ("androidx.browser:browser", "ANDROIDX_BROWSER_VERSION"),
    ("androidx.window:window", "WINDOW_VERSION"),
    ("androidx.window:window-java", "WINDOW_VERSION"),
    ("androidx.car.app:app", "CAR_APP_VERSION"),
    ("com.google.guava:guava", "GUAVA_VERSION"),
    ("androidx.credentials:credentials", "CREDENTIALS_VERSION"),
    (
        "androidx.credentials:credentials-play-services-auth",
        "CREDENTIALS_VERSION",
    ),
    (
        "com.google.android.gms:play-services-location",
        "PLAY_SERVICES_LOCATION_VERSION",
    ),
    (
        "com.google.android.gms:play-services-wallet",
        "PLAY_SERVICES_WALLET_VERSION",
    ),
    (
        "com.google.android.gms:play-services-mlkit-document-scanner",
        "PLAY_SERVICES_MLKIT_DOCUMENT_SCANNER_VERSION",
    ),
];

#[test]
fn gradle_srcdirs_and_dependency_versions() {
    let gen = &gallery()["android/fandhe.gradle"];
    // カタログは全機能を載せるので、Java・res を持つ crate（共通層を含む）がすべて入る。
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for kind in ["java", "res"] {
        let mut want: BTreeSet<String> = fs::read_dir(&crates)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with("fandhe-cap-") || n == "fandhe-platform-android")
            .filter(|n| crates.join(n).join("android").join(kind).is_dir())
            .map(|n| format!("crates/{n}/android/{kind}"))
            .collect();
        if kind == "res" {
            // gen 自身の res（バックアップ規則）。
            want.insert("crates/fandhe-gallery/gen/android/res".to_string());
        }
        assert_eq!(gradle_dirs(gen, kind), want, "{kind}");
    }
    let env = repo("scripts/android/versions.env");
    let deps: Vec<&str> = gen
        .lines()
        .filter_map(|l| l.trim().strip_prefix("implementation '"))
        .map(|l| l.trim_end_matches('\''))
        .collect();
    assert!(!deps.is_empty());
    for g in deps {
        let (ga, ver) = g.rsplit_once(':').unwrap();
        let (_, key) = GRADLE_VERSION_KEYS
            .iter()
            .find(|(k, _)| *k == ga)
            .unwrap_or_else(|| {
                panic!("{ga} の版の選定根拠（versions.env の鍵）が GRADLE_VERSION_KEYS にない")
            });
        assert!(
            env.lines().any(|l| l == format!("{key}=\"{ver}\"")),
            "{key}={ver} が versions.env にない"
        );
    }
}

/// `<key>K</key>` の次の `<string>` の中身。
fn plist_string<'a>(plist: &'a str, key: &str) -> &'a str {
    let after = plist
        .split(&format!("<key>{key}</key>"))
        .nth(1)
        .unwrap_or_else(|| panic!("{key} がない"));
    after
        .split("<string>")
        .nth(1)
        .unwrap()
        .split("</string>")
        .next()
        .unwrap()
}

fn plist_keys(plist: &str) -> BTreeSet<String> {
    strip_comments(plist)
        .split("<key>")
        .skip(1)
        .map(|s| s.split("</key>").next().unwrap().to_string())
        .collect()
}

/// `<key>K</key>` の次の `<array>` の要素（`<string>`・`<integer>`）を連結して返す。
fn plist_array(plist: &str, key: &str) -> String {
    let body = plist.split(&format!("<key>{key}</key>")).nth(1).unwrap();
    let body = body
        .split("<array>")
        .nth(1)
        .unwrap()
        .split("</array>")
        .next()
        .unwrap();
    body.split_whitespace().collect()
}

/// iOS の殻の確定値（Xcode を通さない組み立てでもそのまま使える）とアプリ全体の値（#1194）。
#[test]
fn ios_shell_values_and_app_keys() {
    let gen = &gallery()["ios/Info.plist"];
    assert!(!gen.contains("$("), "Xcode 変数が残っている");
    // 最低対応版は .cargo/config.toml の IPHONEOS_DEPLOYMENT_TARGET とも揃える。
    assert_eq!(plist_string(gen, "MinimumOSVersion"), "17.0");
    assert!(repo(".cargo/config.toml").contains(&format!(
        "IPHONEOS_DEPLOYMENT_TARGET = {{ value = \"{}\"",
        plist_string(gen, "MinimumOSVersion")
    )));
    assert_eq!(
        plist_array(gen, "UIDeviceFamily"),
        "<integer>1</integer><integer>2</integer>"
    );
    assert!(gen.contains("<key>UILaunchScreen</key>"));
    assert_eq!(plist_string(gen, "CFBundleIdentifier"), "com.fandhe.iosapp");
    assert_eq!(
        plist_string(gen, "CFBundleExecutable"),
        "fandhe-gallery-desktop"
    );
    assert!(!gen.contains("CFBundleSupportedPlatforms"));
    // アプリ全体の値（gallery の `fandhe-app.ios`）。
    assert_eq!(plist_string(gen, "CFBundleDisplayName"), "Fandhe");
    assert_eq!(
        plist_array(gen, "CFBundleLocalizations"),
        "<string>ja</string><string>en</string>"
    );
    let ats = gen
        .split("<key>NSAppTransportSecurity</key>")
        .nth(1)
        .unwrap();
    assert!(ats
        .trim_start()
        .starts_with("<dict>\n\t\t<key>NSAllowsLocalNetworking</key>\n\t\t<true/>"));
}

#[test]
fn ios_scene_manifest_names_the_shim_delegate() {
    // 無いと実機で起動直後に終了する（#1292）。選んだ機能に関係なく出す。
    let gen = &gallery()["ios/Info.plist"];
    let scene = gen
        .split("<key>UIApplicationSceneManifest</key>")
        .nth(1)
        .unwrap();
    for k in [
        "UIApplicationSupportsMultipleScenes",
        "UIWindowSceneSessionRoleApplication",
        "UIWindowSceneSessionRoleExternalDisplayNonInteractive",
    ] {
        assert!(scene.contains(&format!("<key>{k}</key>")), "{k}");
    }
    // 名指す delegate は .m が実装している。
    let m = repo("crates/fandhe-platform-apple/ios/fandhe_ios_scene.m");
    assert!(m.contains("@implementation FandheSceneDelegate"));
    assert!(gen.contains("<string>FandheSceneDelegate</string>"));
}

#[test]
fn usage_descriptions_come_from_the_app_config() {
    // 生成側に出た説明文は、gallery の `fandhe-app.<os>.usage-descriptions` の文言そのもの。
    // 選んだ機能が要求するキーだけが出る（欠けは gen がエラーにする）。
    let toml = repo("crates/fandhe-gallery/Cargo.toml");
    for os in ["ios", "macos"] {
        let gen = &gallery()[&format!("{os}/Info.plist")];
        let keys: Vec<_> = plist_keys(gen)
            .into_iter()
            .filter(|k| k.ends_with("UsageDescription"))
            .collect();
        assert!(
            !keys.is_empty(),
            "{os}: UsageDescription が生成されていない"
        );
        let section = toml
            .split(&format!(
                "[package.metadata.fandhe-app.{os}.usage-descriptions]"
            ))
            .nth(1)
            .unwrap();
        for key in keys {
            let line = format!("{key} = \"{}\"", plist_string(gen, &key));
            assert!(section.lines().any(|l| l == line), "{os}: {key}");
        }
    }
}

/// macOS は `fandhe-app.macos.bundle-id` があるので、本体の Info.plist を確定値で出す。
/// gallery の `build.rs` がこれを素の実行体へ埋め込み、`scripts/apple/bundle-macos-url-scheme.sh` が .app へ入れる。
#[test]
fn macos_main_plist_is_concrete_and_widget_extension_keeps_xcode_variables() {
    let plist = &gallery()["macos/Info.plist"];
    assert!(!plist.contains("$("), "{plist}");
    assert_eq!(
        plist_string(plist, "CFBundleIdentifier"),
        "com.fandhe.egui-proxy-desktop"
    );
    assert_eq!(
        plist_string(plist, "CFBundleExecutable"),
        "fandhe-gallery-desktop"
    );
    // サービスメニュー（share-receive）の NSPortName は実行体名へ置き換わり、CFBundleName と揃う。
    assert_eq!(plist_string(plist, "NSPortName"), "fandhe-gallery-desktop");
    assert_eq!(
        plist_string(plist, "CFBundleName"),
        "fandhe-gallery-desktop"
    );
    assert_eq!(plist_string(plist, "NSMessage"), "receiveSharedText");
    assert!(plist.contains("<key>NSAllowsLocalNetworking</key>"));

    // Widget 拡張は Xcode（platforms/apple/project.yml）が組むので、署名時の変数を残す。
    let dir = "macos/extensions/FandheTextWidget";
    let gi = &gallery()[&format!("{dir}/Info.plist")];
    assert_eq!(
        plist_string(gi, "FandheAppGroupIdentifier"),
        "$(TeamIdentifierPrefix)com.fandhe.widget"
    );
    assert_eq!(
        plist_string(gi, "NSExtensionPointIdentifier"),
        "com.apple.widgetkit-extension"
    );
    let ge = &gallery()[&format!("{dir}/FandheTextWidget.entitlements")];
    assert_eq!(
        plist_keys(ge),
        BTreeSet::from([
            "com.apple.security.app-sandbox".to_string(),
            "com.apple.security.application-groups".to_string()
        ])
    );
    assert_eq!(
        plist_string(ge, "com.apple.security.application-groups"),
        "$(TeamIdentifierPrefix)com.fandhe.widget"
    );
    // project.yml の拡張は生成物を指す。
    let yml = repo("platforms/apple/project.yml");
    assert!(yml
        .contains("../../crates/fandhe-gallery/gen/macos/extensions/FandheTextWidget/Info.plist"));
    assert!(yml
        .contains("../../crates/fandhe-gallery/gen/ios/extensions/FandheLiveActivity/Info.plist"));
    assert!(yml.contains("../../crates/fandhe-gallery/gen/ios/App.entitlements"));
}

#[test]
fn gallery_output_is_deterministic_and_marked() {
    let m = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fandhe-gallery/Cargo.toml");
    assert_eq!(generate(Some(&m)).unwrap().files, *gallery());
    assert!(gallery().contains_key(".fandhe-gen"));
}

// ---- 2. 一時 workspace を実際の cargo に通す ----

fn write_crate(root: &Path, dir: &str, toml: &str) {
    let d = root.join("c").join(dir);
    fs::create_dir_all(d.join("src")).unwrap();
    fs::write(d.join("src/lib.rs"), "").unwrap();
    // アプリ（root）は bin が 1 つ要る（iOS の実行体）。他の crate には無害。
    fs::write(d.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(d.join("Cargo.toml"), toml).unwrap();
}

/// `(ディレクトリ, Cargo.toml 全文)` から workspace を作り、`c/<app>/Cargo.toml` を返す。
fn make_ws(tag: &str, app: &str, crates: &[(&str, String)]) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gen-{tag}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"c/*\"]\n",
    )
    .unwrap();
    for (dir, toml) in crates {
        write_crate(&root, dir, toml);
    }
    root.join("c").join(app).join("Cargo.toml")
}

fn pkg(name: &str, rest: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n{rest}")
}

/// アプリの crate。殻に要る cdylib の lib target と bundle id を持つ（`rest` は追加の TOML）。
fn app_pkg(rest: &str) -> String {
    pkg(
        "app",
        &format!(
            "[lib]\ncrate-type = [\"cdylib\", \"rlib\"]\n\n\
             [package.metadata.fandhe-app.ios]\nbundle-id = \"com.example.app\"\n\n{rest}"
        ),
    )
}

/// Android の権限と iOS の説明文キーを持つ機能 crate。
fn cap(id: &str) -> (String, String) {
    (
        format!("fandhe-cap-{id}"),
        pkg(
            &format!("fandhe-cap-{id}"),
            &format!(
                "[package.metadata.fandhe]\nid = \"{id}\"\n\
                 [package.metadata.fandhe.android]\npermissions = [{{ name = \"android.permission.CAMERA\" }}]\n\
                 [package.metadata.fandhe.ios]\nusage-descriptions = [\"NSCameraUsageDescription\"]\n"
            ),
        ),
    )
}

fn errs(r: Result<cargo_fandhe::Generated, Vec<String>>) -> String {
    r.expect_err("拒否されるはず").join("\n")
}

#[test]
fn missing_usage_description_is_an_error() {
    let (dir, toml) = cap("cam");
    let m = make_ws(
        "nodesc",
        "app",
        &[
            (&dir, toml),
            (
                "app",
                app_pkg("[dependencies]\nfandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n"),
            ),
        ],
    );
    let e = errs(generate(Some(&m)));
    assert!(
        e.contains("ios") && e.contains("cam") && e.contains("NSCameraUsageDescription"),
        "{e}"
    );
    // 説明文を書けば通り、文言がそのまま出る。
    let app = app_pkg(
        "[dependencies]\nfandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n\
         [package.metadata.fandhe-app.ios.usage-descriptions]\nNSCameraUsageDescription = \"写真を撮ります\"\n",
    );
    let (dir, toml) = cap("cam");
    let m = make_ws("withdesc", "app", &[(&dir, toml), ("app", app)]);
    let g = generate(Some(&m)).unwrap();
    assert!(g.files["ios/Info.plist"].contains("<string>写真を撮ります</string>"));
    assert!(g.files["android/AndroidManifest.xml"].contains("android.permission.CAMERA"));
    // cdylib のあるアプリには Android を飛ばした通知を出さない（Issue #1399）。
    assert!(g.notes.is_empty(), "{:?}", g.notes);
}

#[test]
fn invalid_declaration_in_dependency_is_an_error() {
    let bad = pkg(
        "fandhe-cap-bad",
        "[package.metadata.fandhe]\nid = \"Bad_ID\"\n",
    );
    let app = app_pkg("[dependencies]\nfandhe-cap-bad = { path = \"../fandhe-cap-bad\" }\n");
    let m = make_ws("baddecl", "app", &[("fandhe-cap-bad", bad), ("app", app)]);
    let e = errs(generate(Some(&m)));
    assert!(e.contains("fandhe-cap-bad"), "{e}");
}

#[test]
fn features_enabled_by_another_member_do_not_leak_into_the_app() {
    let (dir, toml) = cap("cam");
    let app = app_pkg(
        "[features]\nextra = [\"dep:fandhe-cap-cam\"]\n[dependencies]\n\
         fandhe-cap-cam = { path = \"../fandhe-cap-cam\", optional = true }\n",
    );
    // 別 member が app の optional feature を有効にする。workspace 全体の統合結果にはこの辺が入る。
    let other = pkg(
        "other",
        "[dependencies]\napp = { path = \"../app\", features = [\"extra\"] }\n",
    );
    let m = make_ws(
        "leak",
        "app",
        &[(&dir, toml.clone()), ("app", app.clone()), ("other", other)],
    );
    let g = generate(Some(&m)).unwrap();
    assert!(!g.files["android/AndroidManifest.xml"].contains("android.permission.CAMERA"));
    assert!(!g.files["ios/Info.plist"].contains("NSCameraUsageDescription"));

    // 対照: app 自身の default が有効にすれば出る（説明文は書く）。
    let app = app.replace("[features]\n", "[features]\ndefault = [\"extra\"]\n")
        + "[package.metadata.fandhe-app.ios.usage-descriptions]\nNSCameraUsageDescription = \"x\"\n";
    let m = make_ws("leak-ctl", "app", &[(&dir, toml), ("app", app)]);
    assert!(
        generate(Some(&m)).unwrap().files["android/AndroidManifest.xml"]
            .contains("android.permission.CAMERA")
    );
}

#[test]
fn target_specific_dependency_reaches_only_its_os() {
    let (dir, toml) = cap("cam");
    // Android にだけ依存する。iOS には届かないので、iOS の説明文が無くてもエラーにならない。
    let app = app_pkg(
        "[target.'cfg(target_os = \"android\")'.dependencies]\n\
         fandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n",
    );
    let m = make_ws("target", "app", &[(&dir, toml), ("app", app)]);
    let g = generate(Some(&m)).unwrap();
    assert!(g.files["android/AndroidManifest.xml"].contains("android.permission.CAMERA"));
    assert!(!g.files["ios/Info.plist"].contains("NSCameraUsageDescription"));
}

// ---- 3. Web manifest（#1238）----

/// `share_target` を宣言する機能 crate。
fn web_cap() -> (String, String) {
    (
        "fandhe-cap-sharer".into(),
        pkg(
            "fandhe-cap-sharer",
            "[package.metadata.fandhe]\nid = \"sharer\"\n\
             [package.metadata.fandhe.web.manifest.share_target]\naction = \"./\"\nmethod = \"GET\"\n",
        ),
    )
}

/// Web アプリ（基礎 manifest `web.json` つき）の workspace を作り、その Cargo.toml を返す。
fn web_ws(tag: &str, deps: &str, base: &str, web_cfg: &str) -> PathBuf {
    let (dir, toml) = web_cap();
    let app = pkg("app", &format!("{deps}{web_cfg}"));
    let m = make_ws(tag, "app", &[(&dir, toml), ("app", app)]);
    fs::write(m.parent().unwrap().join("web.json"), base).unwrap();
    m
}

const WEB_CFG: &str = "[package.metadata.fandhe-app.web]\nmanifest = \"web.json\"\n";
const SHARER_DEP: &str =
    "[dependencies]\nfandhe-cap-sharer = { path = \"../fandhe-cap-sharer\" }\n";

fn web_manifest(m: &Path) -> Value {
    serde_json::from_str(&generate_web(Some(m)).unwrap().files["web/manifest.webmanifest"]).unwrap()
}

#[test]
fn web_manifest_has_share_target_only_when_the_feature_is_selected() {
    let base = r#"{ "name": "demo", "start_url": "./" }"#;
    // 選んだアプリ: 基礎のキーに share_target が足される。
    let m = web_ws("web-with", SHARER_DEP, base, WEB_CFG);
    let v = web_manifest(&m);
    assert_eq!(v["name"], "demo");
    assert_eq!(v["share_target"]["method"], "GET");
    // 選ばないアプリ: share_target が無く、基礎のキーは残る。
    let m = web_ws("web-without", "", base, WEB_CFG);
    let v = web_manifest(&m);
    assert!(v.get("share_target").is_none(), "{v}");
    assert_eq!(v["name"], "demo");
}

#[test]
fn web_manifest_rejects_bad_inputs() {
    let base = r#"{ "name": "demo" }"#;
    // 基礎が機能のメンバーを持つ。
    let m = web_ws("web-dup", SHARER_DEP, r#"{ "share_target": {} }"#, WEB_CFG);
    assert!(errs(generate_web(Some(&m))).contains("share_target"));
    // fandhe-app.web が無い。
    let m = web_ws("web-nocfg", SHARER_DEP, base, "");
    assert!(errs(generate_web(Some(&m))).contains("fandhe-app.web"));
    // パスが crate の外・絶対。
    for bad in ["../x.json", "/etc/passwd"] {
        let cfg = format!("[package.metadata.fandhe-app.web]\nmanifest = \"{bad}\"\n");
        let m = web_ws("web-path", SHARER_DEP, base, &cfg);
        assert!(errs(generate_web(Some(&m))).contains("相対パス"), "{bad}");
    }
    // 基礎が JSON でない・オブジェクトでない。
    for bad in ["not json", "[]"] {
        let m = web_ws("web-json", SHARER_DEP, bad, WEB_CFG);
        assert!(errs(generate_web(Some(&m))).contains("JSON"), "{bad}");
    }
    // 他の機能が許可リスト外のメンバーを宣言する。
    let bad = pkg(
        "fandhe-cap-sharer",
        "[package.metadata.fandhe]\nid = \"sharer\"\n\
         [package.metadata.fandhe.web.manifest.start_url]\nx = \"y\"\n",
    );
    let app = pkg("app", &format!("{SHARER_DEP}{WEB_CFG}"));
    let m = make_ws(
        "web-allow",
        "app",
        &[("fandhe-cap-sharer", bad), ("app", app)],
    );
    fs::write(m.parent().unwrap().join("web.json"), base).unwrap();
    assert!(errs(generate_web(Some(&m))).contains("start_url"));
}

#[test]
fn web_dir_is_not_generated_without_web_config() {
    // 全 OS の gen は fandhe-app.web が無いアプリに web/ を出さない（gallery の決定性テストも同じ性質に依存）。
    assert!(!gallery().keys().any(|k| k.starts_with("web/")));
}

/// fandhe-web-dom は share-receive を選んでいるので、従来の静的 manifest と同じ内容が配信される。
#[test]
fn web_dom_demo_manifest_keeps_share_target() {
    let m = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fandhe-web-dom/Cargo.toml");
    let want: Value = serde_json::json!({
        "name": "fandhe-web-dom サンプル画面",
        "short_name": "fandhe",
        "start_url": "./",
        "scope": "./",
        "display": "standalone",
        "icons": [
            { "src": "icon-192.png", "sizes": "192x192", "type": "image/png" },
            { "src": "icon-512.png", "sizes": "512x512", "type": "image/png" }
        ],
        "file_handlers": [
            { "action": "./?screen=file_association_show", "accept": { "text/plain": [".txt"] } }
        ],
        "shortcuts": [
            {
                "name": "デモを開く",
                "url": "./?screen=url_scheme_show&url=web%2Bfandhe%3A%2F%2Fshortcut%2Fopen-demo"
            }
        ],
        "share_target": {
            "action": "./",
            "method": "GET",
            "params": { "text": "share_text", "url": "share_url" }
        }
    });
    assert_eq!(web_manifest(&m), want);
}

#[test]
fn new_android_fields_reach_only_the_selecting_app() {
    let cap = pkg(
        "fandhe-cap-x",
        "[package.metadata.fandhe]\nid = \"x\"\n\
         [package.metadata.fandhe.android]\n\
         permissions = [{ name = \"android.permission.NEARBY_WIFI_DEVICES\", flags = [\"neverForLocation\"] }]\n\
         main-activity-attributes = { supportsPictureInPicture = \"true\" }\n\
         application-meta-data = [{ name = \"com.example.flag\", value = \"a&b\" }]\n\
         queries = [{ actions = [\"com.example.QUERY_ACTION\"] }]\n\
         [[package.metadata.fandhe.android.components]]\n\
         kind = \"activity-alias\"\nname = \"com.example.AAlias\"\nexported = true\n\
         target-activity = \"com.example.Zed\"\n\
         [[package.metadata.fandhe.android.components]]\n\
         kind = \"activity\"\nname = \"com.example.Zed\"\nexported = false\n",
    );
    let sel = app_pkg("[dependencies]\nfandhe-cap-x = { path = \"../fandhe-cap-x\" }\n");
    let m = make_ws(
        "newfields",
        "app",
        &[("fandhe-cap-x", cap.clone()), ("app", sel)],
    );
    let x = generate(Some(&m)).unwrap().files["android/AndroidManifest.xml"].clone();
    assert!(
        x.contains("android:usesPermissionFlags=\"neverForLocation\""),
        "{x}"
    );
    assert_eq!(x.matches("android:launchMode").count(), 1);
    assert!(
        x.contains("android:supportsPictureInPicture=\"true\""),
        "{x}"
    );
    assert!(x.contains("android:value=\"a&amp;b\""), "{x}");
    // alias は辞書順で前でも対象の後。
    let (zed, alias) = (
        x.find("com.example.Zed").unwrap(),
        x.find("<activity-alias").unwrap(),
    );
    assert!(zed < alias, "{x}");
    assert!(
        x.contains("android:targetActivity=\"com.example.Zed\""),
        "{x}"
    );
    // <queries> は <application> の前。
    let (q, app) = (
        x.find("<queries>").unwrap(),
        x.find("<application>").unwrap(),
    );
    assert!(q < app, "{x}");
    assert!(
        x.contains("android:name=\"com.example.QUERY_ACTION\""),
        "{x}"
    );
    // meta-data は alias の後（application 直下）。
    assert!(x.find("com.example.flag").unwrap() > alias, "{x}");

    // 選ばないアプリには 1 つも出ない。
    let none = app_pkg("");
    let m = make_ws(
        "newfields-none",
        "app",
        &[("fandhe-cap-x", cap), ("app", none)],
    );
    let x = generate(Some(&m)).unwrap().files["android/AndroidManifest.xml"].clone();
    for n in [
        "NEARBY_WIFI",
        "usesPermissionFlags",
        "supportsPictureInPicture",
        "com.example",
        "activity-alias",
    ] {
        assert!(!x.contains(n), "{n}: {x}");
    }
}

/// `<exclude domain path>` の集合（節の名前 → (domain, path)）。`<full-backup-content>` は節名 `legacy`。
fn excludes(xml: &str) -> BTreeSet<(String, String, String)> {
    let mut section = "legacy".to_string();
    let mut out = BTreeSet::new();
    for (name, a) in tags(xml) {
        match name.as_str() {
            "cloud-backup" | "device-transfer" => section = name,
            "exclude" => {
                out.insert((section.clone(), a["domain"].clone(), a["path"].clone()));
            }
            _ => {}
        }
    }
    out
}

#[test]
fn gallery_backup_exclusions_are_generated() {
    let g = gallery();
    let gen_ = &g["android/res/xml/fandhe_data_extraction_rules.xml"];
    assert!(!excludes(gen_).is_empty());
    let legacy = excludes(&g["android/res/xml/fandhe_backup_rules.xml"]);
    assert_eq!(legacy.len(), 1, "{legacy:?}");
    let m = &g["android/AndroidManifest.xml"];
    assert!(m.contains("android:dataExtractionRules=\"@xml/fandhe_data_extraction_rules\""));
    assert!(m.contains("android:fullBackupContent=\"@xml/fandhe_backup_rules\""));
    assert!(g["android/fandhe.gradle"].contains("android/res"));
}

#[test]
fn secure_credential_exclusion_matches_java_file_name() {
    // Java の保存ファイル名（拡張子なし）と宣言の path の複製を読み合わせる。
    let java = repo(
        "crates/fandhe-cap-secure-credential/android/java/com/fandhe/platform/SecureCredentialHelper.java",
    );
    let name = java
        .lines()
        .find(|l| l.contains("SECURE_CREDENTIAL_FILE ="))
        .and_then(|l| l.split('"').nth(1))
        .expect("SECURE_CREDENTIAL_FILE");
    let m = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let d = check_workspace(Some(&m))
        .unwrap()
        .into_iter()
        .find(|d| d.id == "secure-credential")
        .unwrap();
    let paths: Vec<_> = d
        .android
        .unwrap()
        .backup_exclusions
        .into_iter()
        .map(|e| e.path)
        .collect();
    assert_eq!(paths, [format!("{name}.xml")]);
}

#[test]
fn backup_exclusions_reach_only_the_selecting_app_and_union_with_app_config() {
    let cap = pkg(
        "fandhe-cap-bk",
        "[package.metadata.fandhe]\nid = \"bk\"\n[package.metadata.fandhe.android]\n\
         backup-exclusions = [{ domain = \"sharedpref\", path = \"a&<b.xml\" }]\n",
    );
    let sel = app_pkg(
        "[dependencies]\nfandhe-cap-bk = { path = \"../fandhe-cap-bk\" }\n\
         [package.metadata.fandhe-app.android]\nbackup-exclusions = [{ domain = \"file\", path = \"mine\" }]\n",
    );
    let m = make_ws(
        "bk-sel",
        "app",
        &[("fandhe-cap-bk", cap.clone()), ("app", sel)],
    );
    let f = generate(Some(&m)).unwrap().files;
    let x = &f["android/res/xml/fandhe_data_extraction_rules.xml"];
    assert!(x.contains("path=\"a&amp;&lt;b.xml\""), "{x}");
    assert_eq!(excludes(x).len(), 4, "{x}");
    assert_eq!(
        excludes(&f["android/res/xml/fandhe_backup_rules.xml"]).len(),
        2
    );
    assert!(f["android/AndroidManifest.xml"].contains("android:fullBackupContent"));

    // 選ばないアプリには何も出ない。
    let m = make_ws(
        "bk-none",
        "app",
        &[("fandhe-cap-bk", cap), ("app", app_pkg(""))],
    );
    let f = generate(Some(&m)).unwrap().files;
    assert!(!f.keys().any(|k| k.starts_with("android/res")), "{f:?}");
    for n in ["dataExtractionRules", "fullBackupContent"] {
        assert!(!f["android/AndroidManifest.xml"].contains(n));
    }
    assert!(!f["android/fandhe.gradle"].contains("android/res"));
}

#[test]
fn app_backup_exclusions_reject_bad_path() {
    let app = app_pkg(
        "[package.metadata.fandhe-app.android]\nbackup-exclusions = [{ domain = \"file\", path = \".\" }]\n",
    );
    let m = make_ws("bk-bad", "app", &[("app", app)]);
    assert!(errs(generate(Some(&m))).contains("fandhe-app.android.backup-exclusions[0].path"));
}

/// cdylib の無いアプリ（iOS・デスクトップだけ）は Android を対象にしないため、Android を出さずに通る（Issue #1200）。
#[test]
fn app_without_cdylib_skips_android() {
    let (dir, toml) = cap("cam");
    let app = pkg(
        "app",
        "[package.metadata.fandhe-app.ios]\nbundle-id = \"com.example.app\"\n\n\
         [dependencies]\nfandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n\
         [package.metadata.fandhe-app.ios.usage-descriptions]\nNSCameraUsageDescription = \"写真を撮ります\"\n",
    );
    let m = make_ws("nocdylib", "app", &[(&dir, toml), ("app", app)]);
    let g = generate(Some(&m)).expect("cdylib が無くても iOS は出る");
    assert!(g.files.contains_key("ios/Info.plist"));
    assert!(
        !g.files.keys().any(|k| k.starts_with("android/")),
        "{:?}",
        g.files.keys()
    );
    // 飛ばしたことは通知で知らせる（Issue #1399）。
    assert_eq!(g.notes, [cargo_fandhe::ANDROID_SKIPPED_NOTE]);
}

/// アプリの追記（`fandhe-app.ios.info-plist`・`entitlements`）が実際の TOML から plist へ届く（Issue #1202）。
/// 6 つの型を通し、殻のテーブルへ再帰し、機能の宣言の配列と和集合になる。
#[test]
fn app_info_plist_and_entitlements_append_from_toml() {
    let (dir, toml) = cap("cam");
    let app = app_pkg(
        "[dependencies]\nfandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n\
         [package.metadata.fandhe-app.ios.usage-descriptions]\nNSCameraUsageDescription = \"写真を撮ります\"\n\
         [package.metadata.fandhe-app.ios.info-plist]\n\
         Str = \"a\"\nBool = true\nInt = 7\nReal = 1.5\nArr = [\"x\", \"y\"]\n\
         UILaunchScreen = { UIColorName = \"LaunchBackground\" }\n\
         [package.metadata.fandhe-app.ios.entitlements]\n\"aps-environment\" = \"development\"\n",
    );
    let m = make_ws("append-ios", "app", &[(&dir, toml), ("app", app)]);
    let g = generate(Some(&m)).unwrap();
    let p = &g.files["ios/Info.plist"];
    assert_eq!(plist_string(p, "Str"), "a");
    for k in [
        "<key>Bool</key>\n\t<true/>",
        "<key>Int</key>\n\t<integer>7</integer>",
        "<key>Real</key>\n\t<real>1.5</real>",
        "<key>UIColorName</key>",
        "<string>写真を撮ります</string>",
    ] {
        assert!(p.contains(k), "{k}\n{p}");
    }
    assert_eq!(
        plist_array(p, "Arr"),
        "<string>x</string><string>y</string>"
    );
    assert_eq!(
        plist_string(&g.files["ios/App.entitlements"], "aps-environment"),
        "development"
    );
    // 生成値との食い違いは葉のパスと両側の値つきのエラー（配列の UIDeviceFamily は和集合なので衝突しない）。
    let app = app_pkg("[package.metadata.fandhe-app.ios.info-plist]\nUIDeviceFamily = [3]\nCFBundleName = \"other\"\n");
    let m = make_ws("append-conflict", "app", &[("app", app)]);
    let e = errs(generate(Some(&m)));
    assert!(
        e.contains("CFBundleName") && e.contains("\"app\"") && e.contains("\"other\""),
        "{e}"
    );
    assert!(!e.contains("UIDeviceFamily"), "{e}");
}

/// TOML の日時は `cargo metadata` で `$__toml_private_datetime` のテーブルになる。plist の値にできないので拒否する。
#[test]
fn app_plist_inputs_reject_datetime_and_app_groups_but_accept_usage_description() {
    for body in [
        "[package.metadata.fandhe-app.ios.info-plist]\nWhen = 1979-05-27T07:32:00Z\n",
        "[package.metadata.fandhe-app.ios.info-plist]\nNested = { X = [1979-05-27] }\n",
        "[package.metadata.fandhe-app.ios.entitlements]\nWhen = 07:32:00\n",
    ] {
        let m = make_ws("append-datetime", "app", &[("app", app_pkg(body))]);
        let e = errs(generate(Some(&m)));
        assert!(e.contains("日時"), "{body}\n{e}");
    }
    // `*UsageDescription` は info-plist にも書ける（機能が要求しないキーはそのまま出る）。
    let body = "[package.metadata.fandhe-app.ios.info-plist]\nNSFaceIDUsageDescription = \"顔で解除します\"\n";
    let m = make_ws("append-usage", "app", &[("app", app_pkg(body))]);
    let g = generate(Some(&m)).unwrap();
    assert_eq!(
        plist_string(&g.files["ios/Info.plist"], "NSFaceIDUsageDescription"),
        "顔で解除します"
    );
    let body = "[package.metadata.fandhe-app.ios.entitlements]\n\"com.apple.security.application-groups\" = [\"g\"]\n";
    let m = make_ws("append-groups", "app", &[("app", app_pkg(body))]);
    assert!(errs(generate(Some(&m))).contains("から生成されるため"));
}

/// macOS は機能も bundle-id も無くても、追記があれば Info.plist を出す。
#[test]
fn macos_append_emits_without_features_or_bundle_id() {
    let app = app_pkg(
        "[package.metadata.fandhe-app.macos.info-plist]\nLSUIElement = true\n\
         [package.metadata.fandhe-app.macos.entitlements]\n\"com.apple.security.app-sandbox\" = true\n",
    );
    let m = make_ws("append-macos", "app", &[("app", app)]);
    let g = generate(Some(&m)).unwrap();
    assert!(g.files["macos/Info.plist"].contains("<key>LSUIElement</key>"));
    assert!(g.files["macos/App.entitlements"].contains("com.apple.security.app-sandbox"));
}

/// アプリの `android` 欄は機能の宣言と同じ型で読み、擬似の宣言者として統合される（Issue #1202）。
/// 権限が overlay に出て、`res`・`java` はアプリの crate が基準になり、食い違いは宣言者の id つきでエラーになる。
#[test]
fn app_android_declaration_merges_like_a_feature() {
    let (dir, toml) = cap("cam");
    let body = "[dependencies]\nfandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n[package.metadata.fandhe-app.ios.usage-descriptions]\nNSCameraUsageDescription = \"撮影\"\n\
         [package.metadata.fandhe-app.android]\n\
         permissions = [{ name = \"android.permission.INTERNET\" }, { name = \"android.permission.CAMERA\" }]\n\
         res = [\"android/res\"]\njava = [\"android/java\"]\n";
    let m = make_ws(
        "app-android",
        "app",
        &[(&dir, toml.clone()), ("app", app_pkg(body))],
    );
    let app_dir = m.parent().unwrap();
    fs::create_dir_all(app_dir.join("android/res")).unwrap();
    fs::create_dir_all(app_dir.join("android/java")).unwrap();
    let f = generate(Some(&m)).unwrap().files;
    let man = &f["android/AndroidManifest.xml"];
    assert!(
        man.contains("android:name=\"android.permission.INTERNET\""),
        "{man}"
    );
    // 同じ権限は機能と 1 つに統合される。
    assert_eq!(man.matches("android.permission.CAMERA").count(), 1, "{man}");
    let base = app_dir.to_string_lossy().replace('\\', "/");
    let g = &f["android/fandhe.gradle"];
    assert!(
        g.contains(&format!("res.srcDirs += ['{base}/android/res']")),
        "{g}"
    );
    assert!(
        g.contains(&format!("java.srcDirs += ['{base}/android/java']")),
        "{g}"
    );
    // アプリが何も書かなければ従来どおり（宣言者が増えても出力は変わらない）。
    let m = make_ws(
        "app-android-none",
        "app",
        &[
            (&dir, toml.clone()),
            (
                "app",
                app_pkg("[dependencies]\nfandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n[package.metadata.fandhe-app.ios.usage-descriptions]\nNSCameraUsageDescription = \"撮影\"\n"),
            ),
        ],
    );
    let g = generate(Some(&m)).unwrap().files["android/fandhe.gradle"].clone();
    assert!(!g.contains("srcDirs +="), "{g}");

    // 同名の権限で flags が違えば、アプリを指す id つきのエラー（機能同士と同じ規則）。
    let body = "[dependencies]\nfandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n[package.metadata.fandhe-app.ios.usage-descriptions]\nNSCameraUsageDescription = \"撮影\"\n\
         [package.metadata.fandhe-app.android]\n\
         permissions = [{ name = \"android.permission.CAMERA\", flags = [\"neverForLocation\"] }]\n";
    let m = make_ws(
        "app-android-conflict",
        "app",
        &[(&dir, toml), ("app", app_pkg(body))],
    );
    let e = errs(generate(Some(&m)));
    assert!(
        e.contains("[package.metadata.fandhe-app]")
            && e.contains("`cam`")
            && e.contains("android.permission.CAMERA")
            && e.contains("--eject android/AndroidManifest.xml"),
        "{e}"
    );
}

/// アプリの `android` 欄も機能の宣言と同じ検証を通る。実在しないパス・crate の外・未知のキーは拒否する。
#[test]
fn app_android_declaration_rejects_bad_paths_and_unknown_keys() {
    for (tag, body, want) in [
        (
            "missing-res",
            "res = [\"android/res\"]\n",
            "fandhe-app.android.res[0]",
        ),
        (
            "missing-java",
            "java = [\"nope\"]\n",
            "fandhe-app.android.java[0]",
        ),
        ("outside", "res = [\"..\"]\n", "fandhe-app.android.res[0]"),
        (
            "unknown",
            "colour = \"red\"\n",
            "`application-id`・`application-attributes` を書けます",
        ),
        ("unknown-name", "colour = \"red\"\n", "colour"),
        (
            "attr-ctrl",
            "application-attributes = { label = \"a\\u0001b\" }\n",
            "application-attributes.label: 制御文字",
        ),
        (
            "attr-digit-first",
            "application-attributes = { \"1x\" = \"v\" }\n",
            "application-attributes.1x",
        ),
        (
            "reserved-case",
            "application-attributes = { Debuggable = \"true\" }\n",
            "Debuggable",
        ),
        (
            "bad-perm",
            "permissions = [{ name = \"1x\" }]\n",
            "fandhe-app.android.permissions[0].name",
        ),
        (
            "bad-attr",
            "application-attributes = { \"android:icon\" = \"x\" }\n",
            "application-attributes",
        ),
        (
            "reserved-backup",
            "application-attributes = { dataExtractionRules = \"@xml/x\" }\n",
            "backup-exclusions",
        ),
        (
            "reserved-debug",
            "application-attributes = { debuggable = \"true\" }\n",
            "debuggable",
        ),
    ] {
        let app = app_pkg(&format!("[package.metadata.fandhe-app.android]\n{body}"));
        let m = make_ws(&format!("app-android-{tag}"), "app", &[("app", app)]);
        let e = errs(generate(Some(&m)));
        assert!(e.contains(want), "{tag}: {e}");
    }
}

/// `application-attributes` のキーは先頭が英字なら英数字を含められる（`use32bitAbi` 等）。
#[test]
fn app_application_attributes_accept_alphanumeric_keys() {
    let app = app_pkg(
        "[package.metadata.fandhe-app.android]\napplication-attributes = { use32bitAbi = \"true\" }\n",
    );
    let m = make_ws("app-android-attr-digit", "app", &[("app", app)]);
    generate(Some(&m)).unwrap();
}
