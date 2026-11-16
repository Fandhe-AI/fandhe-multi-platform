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

/// 機能 crate（`crates/fandhe-cap-<id>`）があればその `Cargo.toml` の宣言部分、
/// 無ければ `tests/fixtures/<id>.toml` を返す（機能 crate へ移した機能から fixture を消していく）。
fn fixture(id: &str) -> String {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let krate = manifest.join(format!("../fandhe-cap-{id}/Cargo.toml"));
    if let Ok(toml) = fs::read_to_string(&krate) {
        let mut out = String::new();
        let mut inside = false;
        for line in toml.lines() {
            if line.starts_with("[package.metadata.fandhe")
                || line.starts_with("[[package.metadata.fandhe")
            {
                inside = true;
            } else if line.starts_with('[') {
                inside = false;
            }
            if inside {
                out.push_str(line);
                out.push('\n');
            }
        }
        return out;
    }
    fs::read_to_string(manifest.join(format!("tests/fixtures/{id}.toml"))).unwrap()
}

/// `[package]` と宣言の断片から成る機能 crate を `root/c/<dir>` に作る。参照先のディレクトリも作る。
fn make_crate(root: &Path, dir: &str, name: &str, decl: &str) {
    let d = root.join("c").join(dir);
    for p in ["src", "android/java", "android/res", "apple/widget"] {
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

/// 実 workspace の機能 crate（`crates/fandhe-cap-*`）が持つ宣言。機能 crate へ移した id は fixture を
/// 持たず、ここから読む（#1170〜#1174 が順に移す）。全テストで 1 回だけ `cargo metadata` を通す。
fn real_decls() -> &'static [Declaration] {
    static REAL: std::sync::OnceLock<Vec<Declaration>> = std::sync::OnceLock::new();
    REAL.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
        check_workspace(Some(&manifest)).unwrap()
    })
}

/// `id` の宣言を実 crate から引く。fixture のうちは `None`。
fn real_decl(id: &str) -> Option<Declaration> {
    real_decls().iter().find(|d| d.id == id).cloned()
}

/// `ids` の宣言を集める。実 crate に移した id はそれを使い、残りは fixture から作った一時 workspace を
/// 通す。順序は `ids` に揃える。
fn decls_for(tag: &str, ids: &[&str]) -> Vec<Declaration> {
    let rest: Vec<&str> = ids
        .iter()
        .copied()
        .filter(|id| real_decl(id).is_none())
        .collect();
    let fixture_decls = if rest.is_empty() {
        Vec::new()
    } else {
        check_workspace(Some(&fixtures_ws(tag, &rest))).unwrap()
    };
    ids.iter()
        .map(|id| {
            real_decl(id)
                .or_else(|| fixture_decls.iter().find(|d| d.id == *id).cloned())
                .unwrap_or_else(|| panic!("{id} の宣言がない"))
        })
        .collect()
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
    let decls = decls_for("ws-five", &IDS);
    assert_eq!(decls.len(), IDS.len());
    let refs: Vec<_> = decls.iter().map(|d| (d.id.as_str(), d)).collect();
    let m = merge(&refs).unwrap();
    // camera-capture と document-scan が共有する説明文キーは 1 つ。
    assert_eq!(m.ios.usage_descriptions.len(), 1);
    assert!(m.ios.frameworks.contains("VisionKit"));
    assert!(m.ios.frameworks.contains("AVFoundation"));
    assert_eq!(m.ios.frameworks.len(), 2);
    assert!(m.ios.extensions.is_empty());
    assert_eq!(m.macos.extensions.len(), 1);
    assert!(m.macos.info_plist.contains_key("NSServices"));
    assert_eq!(m.android.features.len(), 4);
    assert!(m.android.features.values().all(|r| !r));
    // HomeWidgetProvider・NfcCardEmulationService と、3 機能の Lifecycle provider（#1257）。
    assert_eq!(m.android.components.len(), 5);
    assert_eq!(m.android.gradle.len(), 1);
    assert!(m.android.permissions.contains_key("android.permission.NFC"));
    assert!(!m.windows_identity);
    // 拡張の取り込み元は OS の所属を保持する。
    assert!(m
        .extension_sources
        .iter()
        .all(|(_, os, _, _)| os == "macos"));
    assert!(!m.extension_sources.is_empty());
}

#[test]
fn planned_ios_extensions_and_windows_identity_are_expressible() {
    // 実装前の iOS 拡張・Windows の宣言は fixtures から外したので、形式の確認はここで保つ。
    let d = decl(
        json!({"id":"x","windows":{"identity":true},"ios":{"app-groups":["share"],
        "extensions":[
          {"name":"FandheShareExtension","bundle-suffix":"share","deployment-target":"15.0",
           "sources":["apple/widget"],"frameworks":["UniformTypeIdentifiers"],"app-groups":["share"],
           "info-plist":{"NSExtension":{"NSExtensionPointIdentifier":"com.apple.share-services"}}},
          {"name":"FandheTextWidget","bundle-suffix":"text","deployment-target":"14.0",
           "sources":["apple/widget"],"frameworks":["WidgetKit","SwiftUI"],"app-groups":["widget"],
           "info-plist":{"NSExtension":{"NSExtensionPointIdentifier":"com.apple.widgetkit-extension"}}}]}}),
    );
    assert!(validate(&d).is_empty());
    let m = merge(&[("x", &d)]).unwrap();
    assert_eq!(m.ios.extensions.len(), 2);
    assert!(m.windows_identity);
}

#[test]
fn camera_and_document_scan_share_usage_description() {
    let decls = decls_for("ws-share", &["camera-capture", "document-scan"]);
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
    assert_eq!(
        m.android.permissions["android.permission.X"].max_sdk,
        Some(30)
    );
    assert_eq!(
        m.android.permissions["android.permission.NFC"].max_sdk,
        None
    );
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

#[test]
fn merge_web_manifest_same_ok_different_rejected() {
    let st = |a: &str| json!({"id":"x","web":{"manifest":{"share_target":{"action":a}}}});
    let (a, b, c) = (decl(st("./")), decl(st("./")), decl(st("/other")));
    assert_eq!(
        merge(&[("a", &a), ("b", &b)]).unwrap().web_manifest.len(),
        1
    );
    let e = merge(&[("a", &a), ("c", &c)]).unwrap_err();
    assert!(
        e[0].field.contains("web.manifest.share_target.action"),
        "{e:?}"
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
    assert_rejected(json!({"id":"x","linux":{}}), "unknown field");
    // web は manifest のメンバーだけ（許可リスト・オブジェクト・制御文字）。#1238
    assert_rejected(
        json!({"id":"x","web":{"manifest":{"start_url":{}}}}),
        "web.manifest.start_url",
    );
    assert_rejected(
        json!({"id":"x","web":{"manifest":{"share_target":"x"}}}),
        "オブジェクト",
    );
    assert_rejected(
        json!({"id":"x","web":{"manifest":{"share_target":{"action":"a\nb"}}}}),
        "制御文字",
    );
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

#[test]
fn extension_usage_descriptions_validated_and_merged_by_equality() {
    let ext = |u: Value| {
        json!({"id":"x","ios":{"extensions":[{"name":"E","bundle-suffix":"ext","deployment-target":"17.0",
            "sources":["apple/widget"],"usage-descriptions":u,"info-plist":{"NSExtension":{}}}]}})
    };
    assert_eq!(problems(ext(json!(["NSCameraUsageDescription"]))), "");
    assert_rejected(ext(json!(["NSCamera"])), "usage-descriptions[0]");

    let with = |id: &str, u: Value| {
        decl(
            ext(u)
                .as_object()
                .cloned()
                .map(|mut o| {
                    o.insert("id".into(), json!(id));
                    Value::Object(o)
                })
                .unwrap(),
        )
    };
    let a = with("a", json!(["NSCameraUsageDescription"]));
    let b = with("b", json!([]));
    let errs = merge(&[("a", &a), ("b", &b)]).unwrap_err();
    assert_eq!(errs[0].field, "ios.extensions.E");
}

// ---- 現行設定との突き合わせ（capability-declaration.md §6）----
// Manifest・srcDirs・NSServices・拡張の plist/entitlements は生成物との比較へ移した（tests/gen.rs、#1176）。
// ここに残すのは生成しない項目（framework のリンク・Gradle の版・Xcode の拡張ターゲット・Swift の import）。
// 寿命: #1194（設定を生成へ切り替え）と #1212（Xcode ターゲット生成）で撤去する。

fn repo(rel: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel),
    )
    .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn declared(id: &str) -> Declaration {
    // 並行テストが同じ id を呼んでも workspace を壊し合わないよう、呼び出しごとに一意な名前にする
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    decls_for(&format!("xc-{id}-{n}"), &[id])
        .into_iter()
        .next()
        .unwrap()
}

#[test]
fn android_gradle_versions_match_versions_env() {
    let env = repo("scripts/android/versions.env");
    let gradle = repo("platforms/android/app/build.gradle.kts");
    for id in IDS {
        for g in declared(id).android.map(|a| a.gradle).unwrap_or_default() {
            let (ga, ver) = g.rsplit_once(':').unwrap();
            let line = gradle
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
            let want = format!("{key}=\"{ver}\"");
            assert!(
                env.lines().any(|l| l == want),
                "{id}: {want} が versions.env にない"
            );
        }
    }
}

#[test]
fn ios_declarations_match_current_settings() {
    let plist = repo("platforms/apple/FandheIosApp/Resources/Info.plist");
    for id in IDS {
        // 機能 crate へ移した機能は crate の build.rs、それ以外は egui-proxy の build.rs と比べる。
        let own = format!("crates/fandhe-cap-{id}/build.rs");
        let build_rs = if Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(&own)
            .exists()
        {
            repo(&own)
        } else {
            repo("crates/fandhe-egui-proxy/build.rs")
        };
        let Some(ios) = declared(id).ios else {
            continue;
        };
        for k in &ios.usage_descriptions {
            assert!(
                plist.contains(&format!("<key>{k}</key>")),
                "{id}: {k} が Info.plist にない"
            );
        }
        for f in &ios.frameworks {
            assert!(
                build_rs.contains(&format!("\"{f}\"")),
                "{id}: {f} が build.rs にない"
            );
        }
    }
}

#[test]
fn widget_extension_matches_project_yml_and_swift() {
    // Widget 拡張。
    let ext = declared("home-widget").macos.unwrap().extensions.remove(0);
    let yml = repo("platforms/apple/project.yml");
    let block = yml
        .split("  FandheTextWidget:\n")
        .nth(1)
        .unwrap()
        .split("\n\n")
        .next()
        .unwrap();
    assert!(block.contains(&format!("deploymentTarget: \"{}\"", ext.deployment_target)));
    assert!(block.contains(&format!(
        "PRODUCT_BUNDLE_IDENTIFIER: com.fandhe.widgethost.{}",
        ext.bundle_suffix
    )));
    // Swift 本体は機能 crate の `sources` にあり、project.yml がそのパスを拡張の sources に含める（#1174）。
    let mut swift = String::new();
    for src in &ext.sources {
        assert!(
            block.contains(&format!("../../crates/fandhe-cap-home-widget/{src}")),
            "{src} が project.yml の FandheTextWidget sources にない"
        );
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fandhe-cap-home-widget")
            .join(src);
        for e in fs::read_dir(dir).unwrap() {
            swift.push_str(&fs::read_to_string(e.unwrap().path()).unwrap());
        }
    }
    for f in &ext.frameworks {
        assert!(
            swift.contains(&format!("import {f}")),
            "{f} を import していない"
        );
    }
}

/// 基盤の Gradle 依存（games-activity 4.4.0）は android-activity 0.6 系に対応する版（#1259）。
/// 上げるときは `fandhe-platform-android` の宣言と `scripts/android/versions.env` を一緒に直す。
#[test]
fn games_activity_version_matches_android_activity_in_lock() {
    let lock =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock")).unwrap();
    let after = lock
        .split("name = \"android-activity\"\nversion = \"")
        .nth(1)
        .unwrap();
    assert!(
        after.starts_with("0.6."),
        "android-activity が 0.6 系でなくなった。games-activity の版（platform-android の宣言と versions.env）を合わせ直すこと"
    );
}

// ---- #1328: activity-alias・application の meta-data・権限のフラグ・MainActivity の属性 ----

fn full_android() -> Value {
    json!({
        "permissions":[{"name":"android.permission.NEARBY_WIFI_DEVICES","flags":["neverForLocation"]}],
        "main-activity-attributes":{"supportsPictureInPicture":"true"},
        "application-meta-data":[
            {"name":"com.google.android.gms.wallet.api.enabled","value":"true"},
            {"name":"com.google.android.gms.car.application","resource":"@xml/automotive_app_desc"}],
        "components":[
            {"kind":"activity","name":"com.example.Rationale","exported":false},
            {"kind":"activity-alias","name":"com.example.Alias","exported":true,
             "target-activity":"com.example.Rationale",
             "attributes":{"permission":"android.permission.START_VIEW_PERMISSION_USAGE"},
             "intent-filters":[{"actions":["android.intent.action.VIEW_PERMISSION_USAGE"]}]}]
    })
}

fn with_component(extra: Value) -> Value {
    let mut a = full_android();
    a["components"][1]
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    android(a)
}

#[test]
fn accepts_alias_meta_data_flags_and_main_activity_attributes() {
    assert!(cargo_fandhe::validate(&decl(android(full_android()))).is_empty());
}

#[test]
fn rejects_invalid_new_android_fields() {
    assert_rejected(
        android(json!({"permissions":[{"name":"a.B","flags":["bogus"]}]})),
        "bogus",
    );
    for k in ["exported", "launchMode", "taskAffinity"] {
        assert_rejected(
            android(json!({"main-activity-attributes":{k:"x"}})),
            &format!("main-activity-attributes.{k}"),
        );
    }
    assert_rejected(
        android(json!({"main-activity-attributes":{"supportsPictureInPicture":"a\nb"}})),
        "使えません",
    );
    // 値域は属性ごと。真偽値の属性は "true" / "false" の完全一致だけを通す。
    for bad in ["banana", "True", "FALSE", "", " true"] {
        assert_rejected(
            android(json!({"main-activity-attributes":{"supportsPictureInPicture":bad}})),
            "main-activity-attributes.supportsPictureInPicture",
        );
    }
    let mut a = full_android();
    a["components"][1]
        .as_object_mut()
        .unwrap()
        .remove("target-activity");
    assert_rejected(android(a), "target-activity");
    assert_rejected(
        with_component(json!({"target-activity":"com.example.Missing"})),
        "target-activity",
    );
    // alias を指す・alias 以外へ書く
    assert_rejected(
        with_component(json!({"target-activity":"com.example.Alias"})),
        "target-activity",
    );
    assert_rejected(comp(json!({"target-activity":"a.B"})), "activity-alias");
    assert_rejected(
        with_component(json!({"attributes":{"targetActivity":"x"}})),
        "targetActivity",
    );
    let mut a = full_android();
    a["application-meta-data"] = json!([
        {"name":"a.b","value":"1","resource":"@xml/x"},
        {"name":"c.d"},
        {"name":"bad name","value":"1"},
        {"name":"e.f","value":"1"},
        {"name":"e.f","value":"2"}]);
    let p = problems(android(a));
    for n in [
        "application-meta-data[0]",
        "application-meta-data[1]",
        "application-meta-data[2].name",
        "e.f",
    ] {
        assert!(p.contains(n), "{n}: {p}");
    }
}

#[test]
fn merge_new_android_fields() {
    let a = decl(json!({"id":"a","android":full_android()}));
    let b = decl(json!({"id":"b","android":full_android()}));
    let m = merge(&[("a", &a), ("b", &b)]).unwrap();
    let p = &m.android.permissions["android.permission.NEARBY_WIFI_DEVICES"];
    assert_eq!(p.flags.len(), 1);
    assert_eq!(m.android.main_activity_attributes.len(), 1);
    assert_eq!(m.android.application_meta_data.len(), 2);

    // flags の食い違いは 2 機能を報告するエラー。
    let c = decl(
        json!({"id":"c","android":{"permissions":[{"name":"android.permission.NEARBY_WIFI_DEVICES"}]}}),
    );
    let e = merge(&[("a", &a), ("c", &c)]).unwrap_err();
    assert_eq!(e.len(), 1);
    assert_eq!((e[0].a.as_str(), e[0].b.as_str()), ("a", "c"));
    assert!(e[0]
        .field
        .contains("android.permissions.android.permission.NEARBY_WIFI_DEVICES"));

    // meta-data・属性の値の不一致。
    let d = decl(json!({"id":"d","android":{
        "main-activity-attributes":{"supportsPictureInPicture":"false"},
        "application-meta-data":[{"name":"com.google.android.gms.wallet.api.enabled","value":"false"}]}}));
    let e = merge(&[("a", &a), ("d", &d)]).unwrap_err();
    let f: Vec<_> = e.iter().map(|x| x.field.as_str()).collect();
    assert!(
        f.contains(&"android.main-activity-attributes.supportsPictureInPicture"),
        "{f:?}"
    );
    assert!(
        f.contains(&"android.application-meta-data.com.google.android.gms.wallet.api.enabled"),
        "{f:?}"
    );
}

#[test]
fn queries_validate_and_merge() {
    let ok = |q: Value| android(json!({"queries": q}));
    assert!(problems(ok(
        json!([{"actions":["android.intent.action.TTS_SERVICE"]}])
    ))
    .is_empty());
    assert_rejected(ok(json!([{"actions":[]}])), "queries[0].actions");
    assert_rejected(ok(json!([{"actions":["bad name"]}])), "queries[0].actions");
    assert_rejected(
        ok(json!([{"actions":["a.b"],"categories":["c.d"]}])),
        "categories",
    );
    assert_rejected(ok(json!([{"actions":["a.b"],"data":[]}])), "data");
    // 重複は 1 件に統合する。
    let a = decl(json!({"id":"a","android":{"queries":[{"actions":["x.Y","x.Z"]}]}}));
    let b = decl(json!({"id":"b","android":{"queries":[{"actions":["x.Y"]}]}}));
    let m = merge(&[("a", &a), ("b", &b)]).unwrap();
    assert_eq!(m.android.queries.len(), 2);
}
