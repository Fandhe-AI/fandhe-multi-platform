//! `cargo fandhe gen` の生成物のテスト（Issue #1176）。
//!
//! 1. カタログアプリ（fandhe-gallery）の 5 機能分の生成結果を、現行の手書き設定と比べる
//!    （寿命: 設定を生成へ切り替える #1194 で、比べる相手ごと撤去する）。
//! 2. 一時 workspace で実際の cargo を通し、宣言・入力の不正の拒否と到達判定（他 member の feature の
//!    混入なし・対象 OS に限った依存）を確かめる。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use cargo_fandhe::declaration::{Component, IntentData, IntentFilter, MetaData};
use cargo_fandhe::{check_workspace, generate, Declaration};
use serde_json::Value;

const IDS: [&str; 5] = [
    "camera-capture",
    "document-scan",
    "nfc-card-emulation",
    "share-receive",
    "home-widget",
];

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

fn real_decls() -> Vec<Declaration> {
    let m = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    check_workspace(Some(&m))
        .unwrap()
        .into_iter()
        .filter(|d| IDS.contains(&d.id.as_str()))
        .collect()
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
    let kind = format!("{:?}", c.kind).to_lowercase();
    let start = t
        .iter()
        .position(|(n, a)| *n == kind && a.get("name") == Some(&c.name))
        .unwrap_or_else(|| panic!("Manifest に {} がない", c.name));
    let mut attrs = t[start].1.clone();
    let exported = attrs.remove("exported").expect("exported がない") == "true";
    attrs.remove("name");
    let mut out = Component {
        kind: c.kind.clone(),
        name: c.name.clone(),
        exported,
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

#[test]
fn android_manifest_overlay_matches_current_manifest() {
    let cur = tags(&repo("platforms/android/app/src/main/AndroidManifest.xml"));
    let gen = tags(&gallery()["android/AndroidManifest.xml"]);
    // 権限・feature は生成側の各要素が現行にあり（属性まで同じ）、宣言が無い camera・NFC の feature は現行にも無い。
    for (kind, key) in [("uses-permission", "name"), ("uses-feature", "name")] {
        let g: Vec<_> = gen.iter().filter(|(k, _)| k == kind).collect();
        assert!(!g.is_empty(), "{kind} が生成されていない");
        for (_, a) in g {
            assert!(
                cur.iter().any(|(k, m)| k == kind && m == a),
                "{kind} {} が現行 Manifest と違う",
                a[key]
            );
        }
    }
    let names = |t: &[Tag], prefix: &str| -> BTreeSet<String> {
        t.iter()
            .filter(|(k, a)| k == "uses-feature" && a["name"].starts_with(prefix))
            .map(|(_, a)| a["name"].clone())
            .collect()
    };
    assert_eq!(
        names(&cur, "android.hardware.camera"),
        names(&gen, "android.hardware.camera")
    );
    // android.hardware.nfc は nfc（タグ読み取り）の宣言で、HCE の宣言ではない。
    let mut cur_nfc = names(&cur, "android.hardware.nfc");
    assert!(cur_nfc.remove("android.hardware.nfc"));
    assert_eq!(cur_nfc, names(&gen, "android.hardware.nfc"));

    // コンポーネントは宣言ごとに、生成側と現行が同じ型の値になる。
    let mut n = 0;
    for d in real_decls() {
        for c in d.android.iter().flat_map(|a| &a.components) {
            assert_eq!(
                manifest_component(&gen, c),
                manifest_component(&cur, c),
                "{}",
                c.name
            );
            n += 1;
        }
    }
    assert_eq!(
        n, 5,
        "HomeWidgetProvider・NfcCardEmulationService と、camera-capture・document-scan・share-receive の Lifecycle provider"
    );

    // MainActivity の SEND の filter は両側で一致する（fandhe scheme など他の filter は機能の宣言ではない）。
    let send = |t: &[Tag]| -> Vec<IntentFilter> {
        main_activity_filters(t)
            .into_iter()
            .filter(|f| f.actions.iter().any(|a| a == "android.intent.action.SEND"))
            .collect()
    };
    assert_eq!(send(&gen).len(), 1);
    assert_eq!(send(&gen), send(&cur));
    assert_eq!(main_activity_filters(&gen).len(), 1);
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

#[test]
fn gradle_srcdirs_and_dependencies_match_build_gradle() {
    let gen = &gallery()["android/fandhe.gradle"];
    let kts = repo("platforms/android/app/build.gradle.kts");
    for kind in ["java", "res"] {
        let cur: BTreeSet<String> = kts
            .lines()
            .filter(|l| l.contains(&format!("{kind}.srcDirs(")) && l.contains("crates/fandhe-"))
            .map(|l| {
                let p = l.split('"').nth(1).unwrap();
                p[p.find("crates/").unwrap()..].to_string()
            })
            .collect();
        assert_eq!(gradle_dirs(gen, kind), cur, "{kind}");
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
        let line = kts
            .lines()
            .find(|l| l.contains(&format!("\"{ga}:${{androidVersion(\"")))
            .unwrap_or_else(|| panic!("{ga} が build.gradle.kts にない"));
        let key = line
            .split("androidVersion(\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
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

#[test]
fn ios_usage_description_matches_current_info_plist() {
    let key = "NSCameraUsageDescription";
    let gen = &gallery()["ios/Info.plist"];
    let cur = repo("platforms/apple/FandheIosApp/Resources/Info.plist");
    assert_eq!(plist_string(gen, key), plist_string(&cur, key));
    // 選ばなかった機能の説明文は出ない。
    assert_eq!(
        plist_keys(gen)
            .iter()
            .filter(|k| k.ends_with("UsageDescription"))
            .count(),
        1
    );
}

#[test]
fn macos_services_and_widget_extension_match_current_settings() {
    // NSServices: bundle-macos-url-scheme.sh の plutil -json 引数（`\"` と `$exe` を戻す）と比較する。
    let sh = repo("scripts/apple/bundle-macos-url-scheme.sh");
    let line = sh
        .lines()
        .find(|l| l.contains("\\\"NSMessage\\\""))
        .expect("NSServices の行がない");
    let json_text = line
        .trim()
        .trim_start_matches('"')
        .trim_end_matches("\" \"$plist\"")
        .replace("\\\"", "\"")
        .replace("$exe", "$(EXECUTABLE_NAME)");
    let want: Value = serde_json::from_str(&json_text).unwrap();
    fn leaves(v: &Value, keys: &mut Vec<String>, strs: &mut Vec<String>) {
        match v {
            Value::String(s) => strs.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|x| leaves(x, keys, strs)),
            Value::Object(m) => m.iter().for_each(|(k, x)| {
                keys.push(k.clone());
                leaves(x, keys, strs);
            }),
            _ => {}
        }
    }
    let (mut keys, mut strs) = (vec![], vec![]);
    leaves(&want, &mut keys, &mut strs);
    // 生成した plist の NSServices 以降の key と string が、JSON の key と string の多重集合と一致する。
    let plist = &gallery()["macos/Info.plist"];
    let svc = plist.split("<key>NSServices</key>").nth(1).unwrap();
    let svc = svc.split("</plist>").next().unwrap();
    for k in &keys {
        assert!(svc.contains(&format!("<key>{k}</key>")), "{k}");
    }
    for s in &strs {
        assert!(svc.contains(&format!("<string>{s}</string>")), "{s}");
    }
    assert_eq!(svc.matches("<key>").count(), keys.len());
    assert_eq!(svc.matches("<string>").count(), strs.len());

    // Widget 拡張の Info.plist と entitlements。
    let dir = "macos/extensions/FandheTextWidget";
    let gi = &gallery()[&format!("{dir}/Info.plist")];
    let ci = repo("platforms/apple/FandheTextWidget/Resources/Info.plist");
    for k in ["FandheAppGroupIdentifier", "NSExtensionPointIdentifier"] {
        assert_eq!(plist_string(gi, k), plist_string(&ci, k), "{k}");
    }
    let ge = &gallery()[&format!("{dir}/FandheTextWidget.entitlements")];
    let ce = repo("platforms/apple/FandheTextWidget/Resources/FandheTextWidget.entitlements");
    assert_eq!(plist_keys(ge), plist_keys(&ce));
    assert_eq!(
        plist_string(ge, "com.apple.security.application-groups"),
        plist_string(&ce, "com.apple.security.application-groups")
    );
    assert!(ge.contains("<true/>") && ce.contains("<true/>"));
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
                pkg(
                    "app",
                    "[dependencies]\nfandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n",
                ),
            ),
        ],
    );
    let e = errs(generate(Some(&m)));
    assert!(
        e.contains("ios") && e.contains("cam") && e.contains("NSCameraUsageDescription"),
        "{e}"
    );
    // 説明文を書けば通り、文言がそのまま出る。
    let app = pkg(
        "app",
        "[dependencies]\nfandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n\
         [package.metadata.fandhe-app.ios.usage-descriptions]\nNSCameraUsageDescription = \"写真を撮ります\"\n",
    );
    let (dir, toml) = cap("cam");
    let m = make_ws("withdesc", "app", &[(&dir, toml), ("app", app)]);
    let g = generate(Some(&m)).unwrap();
    assert!(g.files["ios/Info.plist"].contains("<string>写真を撮ります</string>"));
    assert!(g.files["android/AndroidManifest.xml"].contains("android.permission.CAMERA"));
}

#[test]
fn invalid_declaration_in_dependency_is_an_error() {
    let bad = pkg(
        "fandhe-cap-bad",
        "[package.metadata.fandhe]\nid = \"Bad_ID\"\n",
    );
    let app = pkg(
        "app",
        "[dependencies]\nfandhe-cap-bad = { path = \"../fandhe-cap-bad\" }\n",
    );
    let m = make_ws("baddecl", "app", &[("fandhe-cap-bad", bad), ("app", app)]);
    let e = errs(generate(Some(&m)));
    assert!(e.contains("fandhe-cap-bad"), "{e}");
}

#[test]
fn features_enabled_by_another_member_do_not_leak_into_the_app() {
    let (dir, toml) = cap("cam");
    let app = pkg(
        "app",
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
    assert_eq!(g.files.keys().collect::<Vec<_>>(), [".fandhe-gen"]);

    // 対照: app 自身の default が有効にすれば出る（説明文は書く）。
    let app = app.replace("[features]\n", "[features]\ndefault = [\"extra\"]\n")
        + "[package.metadata.fandhe-app.ios.usage-descriptions]\nNSCameraUsageDescription = \"x\"\n";
    let m = make_ws("leak-ctl", "app", &[(&dir, toml), ("app", app)]);
    assert!(generate(Some(&m))
        .unwrap()
        .files
        .contains_key("android/AndroidManifest.xml"));
}

#[test]
fn target_specific_dependency_reaches_only_its_os() {
    let (dir, toml) = cap("cam");
    // Android にだけ依存する。iOS には届かないので、iOS の説明文が無くてもエラーにならない。
    let app = pkg(
        "app",
        "[target.'cfg(target_os = \"android\")'.dependencies]\n\
         fandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n",
    );
    let m = make_ws("target", "app", &[(&dir, toml), ("app", app)]);
    let g = generate(Some(&m)).unwrap();
    assert!(g.files.contains_key("android/AndroidManifest.xml"));
    assert!(!g.files.keys().any(|k| k.starts_with("ios/")));
}
