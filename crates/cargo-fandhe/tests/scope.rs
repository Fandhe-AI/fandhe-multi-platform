//! `cargo fandhe verify-scope` の検査のテスト（Issue #1178）。
//!
//! 一時 workspace に 3 機能（cam・nfc・both）を置き、アプリは cam だけを選ぶ。
//! 実物に近い観測テキスト（aapt2・Gradle・codesign・otool・plutil の断片）が通ること、
//! 選ばなかった機能の値を 1 つ混ぜるとそれぞれ失敗することを確かめる。

use std::fs;
use std::path::{Path, PathBuf};

use cargo_fandhe::verify_scope;

fn write_crate(root: &Path, dir: &str, toml: &str) {
    let d = root.join("c").join(dir);
    fs::create_dir_all(d.join("src")).unwrap();
    fs::write(d.join("src/lib.rs"), "").unwrap();
    fs::write(d.join("Cargo.toml"), toml).unwrap();
}

fn pkg(name: &str, rest: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n{rest}")
}

fn cap(id: &str, rest: &str) -> (String, String) {
    (
        format!("fandhe-cap-{id}"),
        pkg(
            &format!("fandhe-cap-{id}"),
            &format!("[package.metadata.fandhe]\nid = \"{id}\"\n{rest}"),
        ),
    )
}

const CAM: &str = "\
[package.metadata.fandhe.android]
permissions = [{ name = \"android.permission.CAMERA\" }]
gradle = [\"com.example:cam-lib:1.0\"]
[package.metadata.fandhe.ios]
usage-descriptions = [\"NSCameraUsageDescription\"]
frameworks = [\"AVFoundation\"]
[package.metadata.fandhe.macos]
usage-descriptions = [\"NSCameraUsageDescription\"]
";

const NFC: &str = "\
[package.metadata.fandhe.android]
permissions = [{ name = \"android.permission.NFC\" }]
components = [{ kind = \"service\", name = \"com.example.NfcService\", exported = false }]
gradle = [\"com.example:nfc-lib:2.0\"]
[package.metadata.fandhe.ios]
usage-descriptions = [\"NFCReaderUsageDescription\"]
frameworks = [\"CoreNFC\"]
[package.metadata.fandhe.ios.entitlements]
\"com.apple.developer.nfc.readersession.formats\" = [\"TAG\"]
[[package.metadata.fandhe.ios.extensions]]
name = \"NfcExt\"
bundle-suffix = \"nfc\"
deployment-target = \"17.0\"
sources = [\"ext/Nfc.swift\"]
[package.metadata.fandhe.ios.extensions.info-plist.NSExtension]
NSExtensionPointIdentifier = \"com.apple.widgetkit-extension\"
[package.metadata.fandhe.macos]
frameworks = [\"CoreNFC\"]
";

/// cam と同じ権限だけを宣言する機能（到達した機能と共有する値は禁止にならない）。
const BOTH: &str = "\
[package.metadata.fandhe.android]
permissions = [{ name = \"android.permission.CAMERA\" }]
";

/// `app_deps` はアプリの `[dependencies]` 以降の TOML。ワークスペースの `c/app/Cargo.toml` を返す。
fn make_ws(tag: &str, app_deps: &str, extra: &[(&str, String)]) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("scope-{tag}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"c/*\"]\n",
    )
    .unwrap();
    for (id, rest) in [("cam", CAM), ("nfc", NFC), ("both", BOTH)] {
        let (dir, toml) = cap(id, rest);
        write_crate(&root, &dir, &toml);
    }
    fs::create_dir_all(root.join("c/fandhe-cap-nfc/ext")).unwrap();
    fs::write(root.join("c/fandhe-cap-nfc/ext/Nfc.swift"), "").unwrap();
    write_crate(&root, "app", &pkg("app", app_deps));
    for (dir, toml) in extra {
        write_crate(&root, dir, toml);
    }
    root.join("c/app/Cargo.toml")
}

const SELECT_CAM: &str = "[dependencies]\nfandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n";

const ANDROID_CLEAN: &str = "\
E: uses-permission (name=\"android.permission.CAMERA\")
E: uses-permission (name=\"android.permission.INTERNET\")
E: service (name=\"androidx.work.impl.background.systemjob.SystemJobService\")
+--- com.example:cam-lib:1.0
+--- androidx.core:core-ktx:1.12.0
";
const IOS_CLEAN: &str = "\
<key>NSCameraUsageDescription</key><string>撮影します</string>
<key>CFBundleIdentifier</key><string>com.example.app</string>
/System/Library/Frameworks/AVFoundation.framework/Versions/A/AVFoundation (compatibility version 1.0.0)
/usr/lib/libSystem.B.dylib (compatibility version 1.0.0)
";

fn check(os: &str, m: &Path, observed: &str) -> Result<(), String> {
    verify_scope(Some(m), Some(m), os, observed)
        .map(|_| ())
        .map_err(|e| e.join("\n"))
}

#[test]
fn clean_artifacts_pass() {
    let m = make_ws("clean", SELECT_CAM, &[]);
    check("android", &m, ANDROID_CLEAN).unwrap();
    check("ios", &m, IOS_CLEAN).unwrap();
    check("macos", &m, IOS_CLEAN).unwrap();
}

#[test]
fn leaked_declaration_fails_with_feature_kind_and_value() {
    let m = make_ws("leak", SELECT_CAM, &[]);
    let cases: [(&str, &str, &str, &str); 7] = [
        (
            "android",
            "E: uses-permission (name=\"android.permission.NFC\")",
            "権限",
            "android.permission.NFC",
        ),
        (
            "android",
            "E: service (name=\"com.example.NfcService\")",
            "コンポーネント",
            "com.example.NfcService",
        ),
        (
            "android",
            "+--- com.example:nfc-lib:2.0",
            "Gradle 依存",
            "com.example:nfc-lib",
        ),
        (
            "ios",
            "<key>com.apple.developer.nfc.readersession.formats</key>",
            "entitlement",
            "com.apple.developer.nfc.readersession.formats",
        ),
        (
            "ios",
            "/System/Library/Frameworks/CoreNFC.framework/CoreNFC (compat)",
            "framework",
            "CoreNFC",
        ),
        (
            "ios",
            "<key>NFCReaderUsageDescription</key>",
            "説明文キー",
            "NFCReaderUsageDescription",
        ),
        (
            "ios",
            "PlugIns/NfcExt.appex",
            "拡張ターゲット",
            "NfcExt.appex",
        ),
    ];
    for (os, extra, kind, value) in cases {
        let base = if os == "android" {
            ANDROID_CLEAN
        } else {
            IOS_CLEAN
        };
        let e = check(os, &m, &format!("{base}{extra}\n")).expect_err(value);
        assert!(
            e.contains("nfc") && e.contains(kind) && e.contains(value),
            "{e}"
        );
    }
    // macOS でも framework の混入を検出する。
    let e = check("macos", &m, &format!("{IOS_CLEAN}CoreNFC\n")).unwrap_err();
    assert!(e.contains("CoreNFC"), "{e}");
}

#[test]
fn shared_value_with_selected_feature_is_not_forbidden() {
    // both は CAMERA を宣言するが、到達した cam と共有なので CAMERA が出ても違反ではない。
    let m = make_ws("shared", SELECT_CAM, &[]);
    check("android", &m, ANDROID_CLEAN).unwrap();
}

#[test]
fn empty_observation_fails_and_no_unselected_passes() {
    let m = make_ws("empty", SELECT_CAM, &[]);
    assert!(check("android", &m, "  \n").unwrap_err().contains("空"));
    assert!(check("watch", &m, "x").unwrap_err().contains("未知の OS"));
    let all = "[dependencies]\nfandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n\
               fandhe-cap-nfc = { path = \"../fandhe-cap-nfc\" }\n\
               fandhe-cap-both = { path = \"../fandhe-cap-both\" }\n";
    let m = make_ws("all", all, &[]);
    let r = verify_scope(Some(&m), Some(&m), "android", "").unwrap();
    assert_eq!(r.unselected, 0);
}

#[test]
fn features_enabled_by_another_member_do_not_make_a_feature_reached() {
    // 別 member が app の optional feature を有効にしても、app の default が有効にしない nfc は未選択のまま。
    let app = "[features]\nextra = [\"dep:fandhe-cap-nfc\"]\n[dependencies]\n\
               fandhe-cap-cam = { path = \"../fandhe-cap-cam\" }\n\
               fandhe-cap-nfc = { path = \"../fandhe-cap-nfc\", optional = true }\n";
    let other = pkg(
        "other",
        "[dependencies]\napp = { path = \"../app\", features = [\"extra\"] }\n",
    );
    let m = make_ws("other-member", app, &[("other", other)]);
    let e = check(
        "android",
        &m,
        &format!("{ANDROID_CLEAN}android.permission.NFC\n"),
    )
    .unwrap_err();
    assert!(e.contains("nfc"), "{e}");
}
