//! 宣言の形式・検証・統合のテスト。正常系は本物の `cargo metadata` を通し、
//! 拒否系は JSON から作った宣言を `validate` に当てる（TOML パーサを足さないため）。

use std::fs;
use std::path::{Path, PathBuf};

use cargo_fandhe::{check_paths, check_workspace, merge, parse, validate, Declaration};
use serde_json::{json, Value};

const IDS: [&str; 5] = [
    "camera-capture",
    "document-scan",
    "nfc-card-emulation",
    "share-receive",
    "home-widget",
];

fn fixture(id: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{id}.toml")),
    )
    .unwrap()
}

/// `[package]` と宣言の断片から成る機能 crate を `root/c/<dir>` に作る。参照先のディレクトリも作る。
fn make_crate(root: &Path, dir: &str, name: &str, decl: &str) {
    let d = root.join("c").join(dir);
    for p in [
        "src",
        "android/java",
        "android/res",
        "apple/share",
        "apple/widget",
    ] {
        fs::create_dir_all(d.join(p)).unwrap();
    }
    fs::write(d.join("src/lib.rs"), "").unwrap();
    fs::write(
        d.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n{decl}"),
    )
    .unwrap();
}

fn make_ws(tag: &str, crates: &[(String, String, String)]) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(tag);
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"c/*\"]\n",
    )
    .unwrap();
    for (dir, name, decl) in crates {
        make_crate(&root, dir, name, decl);
    }
    root.join("Cargo.toml")
}

fn fixtures_ws(tag: &str, ids: &[&str]) -> PathBuf {
    let crates: Vec<_> = ids
        .iter()
        .map(|id| (id.to_string(), format!("fandhe-cap-{id}"), fixture(id)))
        .collect();
    make_ws(tag, &crates)
}

fn errors(r: Result<Vec<Declaration>, Vec<String>>) -> String {
    r.expect_err("拒否されるはず").join("\n")
}

fn decl(v: Value) -> Declaration {
    parse(v).unwrap()
}

// ---- 正常系・統合 ----

#[test]
fn five_fixtures_pass_through_cargo_metadata_and_merge() {
    let decls = check_workspace(Some(&fixtures_ws("ws-five", &IDS))).unwrap();
    assert_eq!(decls.len(), IDS.len());
    let refs: Vec<_> = decls.iter().map(|d| (d.id.as_str(), d)).collect();
    let m = merge(&refs).unwrap();
    // camera-capture と document-scan が共有する説明文キーは 1 つ。
    assert_eq!(m.ios.usage_descriptions.len(), 1);
    assert!(m.ios.frameworks.contains("VisionKit"));
    assert_eq!(m.ios.extensions.len(), 2);
    assert!(m.macos.info_plist.contains_key("NSServices"));
    assert_eq!(m.android.features.len(), 4);
    assert!(m.android.features.values().all(|r| !r));
    assert_eq!(m.android.components.len(), 2);
    assert_eq!(m.android.gradle.len(), 1);
    assert!(m.android.permissions.contains_key("android.permission.NFC"));
    assert!(m.windows_identity);
    // 拡張の取り込み元は OS の所属を保持する。
    assert!(m
        .extension_sources
        .iter()
        .all(|(_, os, _, _)| os == "ios" || os == "macos"));
    assert!(!m.extension_sources.is_empty());
}

#[test]
fn camera_and_document_scan_share_usage_description() {
    let decls = check_workspace(Some(&fixtures_ws(
        "ws-share",
        &["camera-capture", "document-scan"],
    )))
    .unwrap();
    let refs: Vec<_> = decls.iter().map(|d| (d.id.as_str(), d)).collect();
    let m = merge(&refs).unwrap();
    assert_eq!(m.ios.usage_descriptions.len(), 1);
}

#[test]
fn real_workspace_declarations_are_valid() {
    // 宣言を持つ crate が増えたら CI（cargo test）が自動で検証する。
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    check_workspace(Some(&manifest)).unwrap();
}

#[test]
fn merge_dedups_permissions_and_resolves_flags() {
    let a = decl(json!({"id":"a","android":{
        "permissions":[{"name":"android.permission.NFC"},{"name":"android.permission.X","max-sdk":28}],
        "features":[{"name":"f.x","required":false}]}}));
    let b = decl(json!({"id":"b","android":{
        "permissions":[{"name":"android.permission.NFC"},{"name":"android.permission.X","max-sdk":30}],
        "features":[{"name":"f.x","required":true}],
        "gradle":["g:a:1.0","g:a:2.0"]}}));
    let m = merge(&[("b", &b), ("a", &a)]).unwrap();
    assert_eq!(m.android.permissions.len(), 2);
    assert_eq!(m.android.permissions["android.permission.X"], Some(30));
    assert_eq!(m.android.permissions["android.permission.NFC"], None);
    assert!(m.android.features["f.x"]);
    assert_eq!(m.android.gradle.len(), 2);
}

#[test]
fn merge_unions_arrays_and_rejects_conflicts() {
    let a = decl(json!({"id":"a","ios":{"info-plist":{"K":["1"],"S":"x"}}}));
    let b = decl(json!({"id":"b","ios":{"info-plist":{"K":["1","2"],"S":"x"}}}));
    let m = merge(&[("a", &a), ("b", &b)]).unwrap();
    assert_eq!(m.ios.info_plist["K"], json!(["1", "2"]));

    let a = decl(json!({"id":"a","ios":{"info-plist":{"K":"1"}}}));
    let b = decl(json!({"id":"b","ios":{"info-plist":{"K":"2"}}}));
    let e = merge(&[("a", &a), ("b", &b)]).unwrap_err().remove(0);
    assert!(
        e.field == "ios.info-plist.K" && e.a == "a" && e.b == "b",
        "{e}"
    );
}

#[test]
fn merge_components_same_ok_different_rejected() {
    let comp = |x: bool| json!({"kind":"service","name":"a.B","exported":x});
    let a = decl(json!({"id":"a","android":{"components":[comp(true)]}}));
    let b = decl(json!({"id":"b","android":{"components":[comp(false)]}}));
    assert!(merge(&[("a", &a), ("b", &b)]).is_err());
    let c = decl(json!({"id":"c","android":{"components":[comp(true)]}}));
    assert_eq!(
        merge(&[("a", &a), ("c", &c)])
            .unwrap()
            .android
            .components
            .len(),
        1
    );
}

// ---- 拒否 ----

fn problems(v: Value) -> String {
    match parse(v) {
        Err(p) => p.to_string(),
        Ok(d) => validate(&d)
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn assert_rejected(v: Value, needle: &str) {
    let p = problems(v);
    assert!(p.contains(needle), "{needle:?} を含むはずが: {p}");
}

fn android(a: Value) -> Value {
    json!({"id":"x","android":a})
}

fn comp(extra: Value) -> Value {
    let mut c = json!({"kind":"service","name":"a.B","exported":false});
    c.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    android(json!({"components":[c]}))
}

#[test]
fn accepts_minimal_valid() {
    assert_eq!(problems(comp(json!({}))), "");
}

#[test]
fn rejects_structure_errors() {
    assert_rejected(json!({"id":"x","web":{}}), "unknown field");
    assert_rejected(
        json!({"id":"x","android":{"permisions":[]}}),
        "unknown field",
    );
    assert_rejected(json!({"id":"x","android":{"gradle":"a"}}), "invalid type");
    assert_rejected(android(json!({"features":[{"name":"f.x"}]})), "required");
    assert_rejected(
        json!({"id":"x","android":{"components":[{"kind":"service","name":"a.B"}]}}),
        "exported",
    );
    assert_rejected(
        json!({"id":"x","android":{"components":[{"kind":"widget","name":"a.B","exported":true}]}}),
        "widget",
    );
}

#[test]
fn rejects_id_and_names() {
    assert_rejected(json!({"id":"Bad_Id"}), "id:");
    assert_rejected(
        android(json!({"permissions":[{"name":".relative"}]})),
        "permissions[0].name",
    );
    assert_rejected(
        android(json!({"permissions":[{"name":"a b"}]})),
        "permissions[0].name",
    );
    assert_rejected(
        android(json!({"permissions":[{"name":"a.B"},{"name":"a.B"}]})),
        "重複",
    );
    assert_rejected(comp(json!({"name":".Rel"})), "components[0].name");
}

#[test]
fn rejects_apple_errors() {
    assert_rejected(
        json!({"id":"x","ios":{"usage-descriptions":["NSCamera"]}}),
        "usage-descriptions[0]",
    );
    assert_rejected(
        json!({"id":"x","ios":{"info-plist":{"NSCameraUsageDescription":"埋め込み"}}}),
        "usage-descriptions",
    );
    assert_rejected(
        json!({"id":"x","ios":{"entitlements":{"com.apple.security.application-groups":["g"]}}}),
        "app-groups",
    );
    for bad in ["group.com.x", "$(TeamIdentifierPrefix)x", "a.b"] {
        assert_rejected(
            json!({"id":"x","macos":{"app-groups":[bad]}}),
            "app-groups[0]",
        );
    }
    assert_rejected(
        json!({"id":"x","ios":{"frameworks":["Vision Kit"]}}),
        "frameworks[0]",
    );
    assert_rejected(
        json!({"id":"x","ios":{"info-plist":{"K":"a\u{0}b"}}}),
        "制御文字",
    );
}

#[test]
fn rejects_extension_errors() {
    let ext = |k: &str, v: Value| {
        let mut e = json!({"name":"Ext","bundle-suffix":"ext","deployment-target":"14.0","sources":["apple/widget"],
            "info-plist":{"NSExtension":{}}});
        e[k] = v;
        json!({"id":"x","ios":{"extensions":[e]}})
    };
    assert_eq!(problems(ext("name", json!("Ext"))), "");
    assert_rejected(ext("bundle-suffix", json!("Bad")), "bundle-suffix");
    assert_rejected(ext("deployment-target", json!("14")), "deployment-target");
    assert_rejected(ext("sources", json!([])), "sources");
    assert_rejected(ext("sources", json!(["../x"])), "sources[0]");
    assert_rejected(ext("name", json!("a-b")), "name");
    assert_rejected(ext("info-plist", json!({})), "NSExtension");
}

#[test]
fn rejects_gradle_and_paths() {
    for bad in [
        "g:a:+",
        "g:a:latest",
        "g:a:[1,2)",
        "g:a:${v}",
        "g:a",
        "g:a:1.0:x",
        "g:a:1.0-SNAPSHOT",
        "g:a:1.0-snapshot",
    ] {
        assert_rejected(android(json!({"gradle":[bad]})), "gradle[0]");
    }
    for bad in ["../x", "/abs", "a\\b", "C:/x", "", "a\nb"] {
        assert_rejected(android(json!({"java":[bad]})), "java[0]");
        assert_rejected(android(json!({"res":[bad]})), "res[0]");
    }
    // Apple 拡張の sources も同じ相対パス規則
    let ext = |src: &str| {
        json!({"id":"x","ios":{"extensions":[{"name":"Ext","bundle-suffix":"ext",
            "deployment-target":"14.0","sources":[src],"info-plist":{"NSExtension":{}}}]}})
    };
    assert_rejected(ext("a\nb"), "sources[0]");
    assert_rejected(ext("a\u{7}b"), "sources[0]");
}

#[test]
fn rejects_component_details() {
    assert_rejected(comp(json!({"name":"MyService"})), "完全修飾名");
    assert_rejected(comp(json!({"attributes":{"name":"x"}})), "attributes.name");
    assert_rejected(comp(json!({"attributes":{"a:b":"x"}})), "attributes.a:b");
    assert_rejected(comp(json!({"attributes":{"label":"a\nb"}})), "制御文字");
    assert_rejected(comp(json!({"intent-filters":[{"actions":[]}]})), "actions");
    assert_rejected(comp(json!({"meta-data":[{"name":"a.b"}]})), "どちらか一方");
    assert_rejected(
        comp(json!({"meta-data":[{"name":"a.b","resource":"@xml/x","value":"v"}]})),
        "どちらか一方",
    );
    assert_rejected(
        comp(json!({"meta-data":[{"name":"a.b","resource":"xml/x"}]})),
        "resource",
    );
}

// ---- check_paths ----

#[test]
fn check_paths_rejects_missing_and_escaping() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("paths");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("crate/ok")).unwrap();
    fs::create_dir_all(root.join("outside")).unwrap();
    let d = decl(json!({"id":"x","android":{"java":["ok","missing"]}}));
    let p = check_paths(&d, &root.join("crate"));
    assert_eq!(p.len(), 1);
    assert!(p[0].field.contains("java[1]"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("outside"), root.join("crate/link")).unwrap();
        let d = decl(json!({"id":"x","android":{"res":["link"]}}));
        assert!(check_paths(&d, &root.join("crate"))[0]
            .message
            .contains("外"));
    }
}

// ---- check_workspace ----

#[test]
fn workspace_rejects_name_mismatch_missing_declaration_duplicate_and_unknown_key() {
    let mk = |id: &str| format!("[package.metadata.fandhe]\nid = \"{id}\"\n");
    let c = |dir: &str, name: &str, decl: String| (dir.to_string(), name.to_string(), decl);

    let m = make_ws("ws-mismatch", &[c("a", "fandhe-cap-a", mk("b"))]);
    assert!(errors(check_workspace(Some(&m))).contains("一致しません"));

    let m = make_ws("ws-missing", &[c("a", "fandhe-cap-a", String::new())]);
    assert!(errors(check_workspace(Some(&m))).contains("がありません"));

    let m = make_ws(
        "ws-dup",
        &[c("a", "third-a", mk("same")), c("b", "third-b", mk("same"))],
    );
    assert!(errors(check_workspace(Some(&m))).contains("重複"));

    let m = make_ws(
        "ws-unknown",
        &[c("a", "fandhe-cap-a", format!("{}bogus = 1\n", mk("a")))],
    );
    assert!(errors(check_workspace(Some(&m))).contains("unknown field"));
}

#[test]
fn workspace_checks_declarations_of_path_dependencies_outside_workspace() {
    // workspace 外の path 依存（公開 crate と同じ立場）の不正な宣言も拾う。
    let m = make_ws(
        "ws-outside",
        &[(
            "app".into(),
            "app".into(),
            "[dependencies]\nfandhe-cap-ext = { path = \"../../ext\" }\n".into(),
        )],
    );
    let ext = m.parent().unwrap().join("ext");
    fs::create_dir_all(ext.join("src")).unwrap();
    fs::write(ext.join("src/lib.rs"), "").unwrap();
    fs::write(
        ext.join("Cargo.toml"),
        "[package]\nname = \"fandhe-cap-ext\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[package.metadata.fandhe]\nid = \"WRONG\"\n",
    )
    .unwrap();
    assert!(errors(check_workspace(Some(&m))).contains("fandhe-cap-ext"));
}

#[test]
fn merge_reports_owner_of_object_vs_scalar_conflict() {
    let d = |id: &str, v: Value| decl(json!({"id":id,"ios":{"info-plist":{"K":v}}}));
    let (a, b) = (d("a", json!({"X":1})), d("b", json!("s")));
    let e = merge(&[("a", &a), ("b", &b)]).unwrap_err().remove(0);
    assert!(e.a == "a" && e.b == "b", "{e}");
}

#[test]
fn merge_reports_nested_conflict_owners() {
    let d = |id: &str, v: Value| decl(json!({"id":id,"ios":{"info-plist":{"K":v}}}));
    let (a, b, c) = (
        d("a", json!({"X":1})),
        d("b", json!({"Y":1})),
        d("c", json!({"Y":2})),
    );
    let e = merge(&[("a", &a), ("b", &b), ("c", &c)])
        .unwrap_err()
        .remove(0);
    assert!(
        e.field == "ios.info-plist.K.Y" && e.a == "b" && e.b == "c",
        "{e}"
    );
}

#[test]
fn merge_collects_all_conflicts() {
    let d = |id: &str, x: &str, y: &str| decl(json!({"id":id,"ios":{"info-plist":{"X":x,"Y":y}}}));
    let (a, b) = (d("a", "1", "1"), d("b", "2", "2"));
    assert_eq!(merge(&[("a", &a), ("b", &b)]).unwrap_err().len(), 2);
}

#[test]
fn merge_reports_all_conflicts_inside_one_object() {
    let d = |id: &str, n: i32| decl(json!({"id":id,"ios":{"info-plist":{"K":{"X":n,"Y":n}}}}));
    let (a, b) = (d("a", 1), d("b", 2));
    let errs = merge(&[("a", &a), ("b", &b)]).unwrap_err();
    let fields: Vec<_> = errs.iter().map(|e| e.field.as_str()).collect();
    assert_eq!(fields, ["ios.info-plist.K.X", "ios.info-plist.K.Y"]);
    assert!(errs.iter().all(|e| e.a == "a" && e.b == "b"));
}

#[test]
fn merge_reports_conflicts_across_three_features_in_one_object() {
    let d = |id: &str, k: Value| decl(json!({"id":id,"ios":{"info-plist":{"K":k}}}));
    let (a, b, c) = (
        d("a", json!({"X":1})),
        d("b", json!({"X":2,"Y":1})),
        d("c", json!({"Y":2})),
    );
    let errs = merge(&[("a", &a), ("b", &b), ("c", &c)]).unwrap_err();
    let got: Vec<_> = errs
        .iter()
        .map(|e| (e.field.as_str(), e.a.as_str(), e.b.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("ios.info-plist.K.X", "a", "b"),
            ("ios.info-plist.K.Y", "b", "c")
        ]
    );
}

#[test]
fn rejects_qualified_usage_description_keys_in_plist() {
    for k in [
        "NSCameraUsageDescription~ipad",
        "NSCameraUsageDescription-iphoneos",
        "NSCameraUsageDescription-iphoneos~ipad",
    ] {
        let body = json!({k: "既定文言"});
        assert_rejected(
            json!({"id":"x","ios":{"info-plist":body.clone()}}),
            "説明文",
        );
        assert_rejected(
            json!({"id":"x","ios":{"extensions":[{"name":"E","bundle-suffix":"ext","deployment-target":"17.0","sources":["apple/widget"],
                "info-plist":{"NSExtension":{},k:"既定文言"}}]}}),
            "説明文",
        );
    }
    let ok = json!({"id":"x","ios":{"info-plist":{"NSSomething-iphoneos":"v"}}});
    assert_eq!(problems(ok), "");
}

#[test]
fn merge_distinguishes_dotted_key_from_nested_path() {
    let a = decl(json!({"id":"a","ios":{"info-plist":{"K.X":1}}}));
    let b = decl(json!({"id":"b","ios":{"info-plist":{"K":{"X":1}}}}));
    let c = decl(json!({"id":"c","ios":{"info-plist":{"K":{"X":2}}}}));
    let errs = merge(&[("a", &a), ("b", &b), ("c", &c)]).unwrap_err();
    assert_eq!(errs.len(), 1);
    let e = &errs[0];
    assert_eq!(
        (e.field.as_str(), e.a.as_str(), e.b.as_str()),
        ("ios.info-plist.K.X", "b", "c")
    );
}
