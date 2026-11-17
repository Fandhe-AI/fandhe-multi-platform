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
components = [{ kind = \"service\", name = \"com.example.NfcService\", exported = false },
  { kind = \"activity\", name = \"com.example.NfcActivity\", exported = false },
  { kind = \"activity-alias\", name = \"com.example.NfcAlias\", exported = true, target-activity = \"com.example.NfcActivity\" }]
application-meta-data = [{ name = \"com.example.nfc.enabled\", value = \"true\" }]
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

const BLE: &str = "\
[package.metadata.fandhe.android]
permissions = [{ name = \"android.permission.BLUETOOTH_SCAN\", flags = [\"neverForLocation\"] }]
";
const SCAN: &str = "\
[package.metadata.fandhe.android]
permissions = [{ name = \"android.permission.BLUETOOTH_SCAN\" }]
";
const BK: &str = "\
[package.metadata.fandhe.android]
backup-exclusions = [{ domain = \"sharedpref\", path = \"fandhe_bk.xml\" }]
";
const PIP: &str = "\
[package.metadata.fandhe.android.main-activity-attributes]
supportsPictureInPicture = \"true\"
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
    for (id, rest) in [
        ("cam", CAM),
        ("nfc", NFC),
        ("both", BOTH),
        ("ble", BLE),
        ("scan", SCAN),
        ("pip", PIP),
        ("bk", BK),
    ] {
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

/// aapt2 xmltree の実形式（`A:` 行は `名前(0x…)=値`）。MainActivity は殻が固定で付ける属性だけ。
const MAIN_ACTIVITY_CLEAN: &str = "\
      E: activity (line=20)
        A: http://schemas.android.com/apk/res/android:name(0x01010003)=\"com.fandhe.platform.MainActivity\" (Raw: \"com.fandhe.platform.MainActivity\")
        A: http://schemas.android.com/apk/res/android:exported(0x01010010)=true
        A: http://schemas.android.com/apk/res/android:launchMode(0x0101001d)=2
";
const ANDROID_CLEAN: &str = "\
      E: uses-permission (line=10)
        A: http://schemas.android.com/apk/res/android:name(0x01010003)=\"android.permission.CAMERA\" (Raw: \"android.permission.CAMERA\")
E: uses-permission (name=\"android.permission.INTERNET\")
      E: activity (line=20)
        A: http://schemas.android.com/apk/res/android:name(0x01010003)=\"com.fandhe.platform.MainActivity\" (Raw: \"com.fandhe.platform.MainActivity\")
        A: http://schemas.android.com/apk/res/android:exported(0x01010010)=true
        A: http://schemas.android.com/apk/res/android:launchMode(0x0101001d)=2
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
    let cases: [(&str, &str, &str, &str); 9] = [
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
            "E: activity-alias (name=\"com.example.NfcAlias\")",
            "コンポーネント",
            "com.example.NfcAlias",
        ),
        (
            "android",
            "E: meta-data (name=\"com.example.nfc.enabled\")",
            "application の meta-data",
            "com.example.nfc.enabled",
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
fn empty_observation_fails_even_when_all_selected() {
    let m = make_ws("empty", SELECT_CAM, &[]);
    assert!(check("android", &m, "  \n").unwrap_err().contains("空"));
    assert!(check("watch", &m, "x").unwrap_err().contains("未知の OS"));
    let m = make_ws(
        "all",
        &deps(&["cam", "nfc", "both", "ble", "scan", "pip", "bk"]),
        &[],
    );
    // 全機能選択でも空の観測（aapt2 失敗）は失敗にする。
    assert!(check("android", &m, "").unwrap_err().contains("空"));
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

fn scan_perm(flags: Option<&str>) -> String {
    let f = flags.map_or(String::new(), |v| {
        format!("        A: http://schemas.android.com/apk/res/android:usesPermissionFlags(0x01010644)={v}\n")
    });
    format!(
        "      E: uses-permission (line=30)\n        A: http://schemas.android.com/apk/res/android:name(0x01010003)=\"android.permission.BLUETOOTH_SCAN\" (Raw: \"android.permission.BLUETOOTH_SCAN\")\n{f}"
    )
}

fn deps(ids: &[&str]) -> String {
    let mut s = "[dependencies]\n".to_string();
    for i in ids {
        s += &format!("fandhe-cap-{i} = {{ path = \"../fandhe-cap-{i}\" }}\n");
    }
    s
}

#[test]
fn unselected_permission_flag_fails() {
    let m = make_ws("flag-leak", &deps(&["cam", "scan"]), &[]);
    let e = check(
        "android",
        &m,
        &format!("{ANDROID_CLEAN}{}", scan_perm(Some("0x00010000"))),
    )
    .unwrap_err();
    assert!(
        e.contains("ble")
            && e.contains("neverForLocation")
            && e.contains("android.permission.BLUETOOTH_SCAN"),
        "{e}"
    );
    // uses-permission-sdk-23 でも同じ検査が効く。
    let e = check(
        "android",
        &m,
        &format!(
            "{ANDROID_CLEAN}{}",
            scan_perm(Some("0x00010000"))
                .replace("E: uses-permission ", "E: uses-permission-sdk-23 ")
        ),
    )
    .unwrap_err();
    assert!(e.contains("neverForLocation"), "{e}");
    check(
        "android",
        &m,
        &format!("{ANDROID_CLEAN}{}", scan_perm(None)),
    )
    .unwrap();
    // 宣言のどこにも無いビットは「宣言にない」として失敗する。
    let e = check(
        "android",
        &m,
        &format!("{ANDROID_CLEAN}{}", scan_perm(Some("0x2"))),
    )
    .unwrap_err();
    assert!(e.contains("宣言にありません"), "{e}");
    let e = check(
        "android",
        &m,
        &format!("{ANDROID_CLEAN}{}", scan_perm(Some("zz"))),
    )
    .unwrap_err();
    assert!(e.contains("読めません"), "{e}");
}

#[test]
fn selected_permission_flag_passes() {
    let m = make_ws("flag-ok", &deps(&["cam", "ble"]), &[]);
    check(
        "android",
        &m,
        &format!("{ANDROID_CLEAN}{}", scan_perm(Some("0x00010000"))),
    )
    .unwrap();
}

#[test]
fn main_activity_attribute_follows_declaration() {
    let with = |v: &str| {
        ANDROID_CLEAN.replace(
            "launchMode(0x0101001d)=2\n",
            &format!("launchMode(0x0101001d)=2\n        A: http://schemas.android.com/apk/res/android:supportsPictureInPicture(0x010104f7)={v}\n"),
        )
    };
    let m = make_ws("pip-leak", SELECT_CAM, &[]);
    let e = check("android", &m, &with("true")).unwrap_err();
    assert!(
        e.contains("pip") && e.contains("supportsPictureInPicture"),
        "{e}"
    );
    let m = make_ws("pip-ok", &deps(&["cam", "pip"]), &[]);
    check("android", &m, &with("true")).unwrap();
    let e = check("android", &m, &with("false")).unwrap_err();
    assert!(e.contains("宣言にありません"), "{e}");
}

#[test]
fn missing_main_activity_fails_and_empty_selection_has_no_false_positive() {
    let m = make_ws("no-main", SELECT_CAM, &[]);
    let e = check(
        "android",
        &m,
        "E: uses-permission (name=\"android.permission.CAMERA\")\n",
    )
    .unwrap_err();
    assert!(e.contains("MainActivity"), "{e}");
    // どの機能も選ばない app + クリーンな殻（INTERNET と許可リスト属性なしの MainActivity）。
    let m = make_ws("none", "[dependencies]\n", &[]);
    check(
        "android",
        &m,
        &format!(
            "E: uses-permission (name=\"android.permission.INTERNET\")\n{MAIN_ACTIVITY_CLEAN}"
        ),
    )
    .unwrap();
}

#[test]
fn all_selected_still_checks_structure() {
    // 未選択 0 件でも構造照合は走る。全機能を選んだ正常系（宣言どおりのフラグ・属性）は誤検知しない。
    let m = make_ws(
        "all-flags",
        &deps(&["cam", "nfc", "both", "ble", "scan", "pip", "bk"]),
        &[],
    );
    let pip = "        A: http://schemas.android.com/apk/res/android:supportsPictureInPicture(0x010104f7)=true\n";
    let ok = format!(
        "{}{MAIN_ACTIVITY_CLEAN}{pip}",
        scan_perm(Some("0x00010000"))
    );
    assert_eq!(
        verify_scope(Some(&m), Some(&m), "android", &ok)
            .unwrap()
            .unselected,
        0
    );
    // 宣言にない属性値・未知のフラグビット・MainActivity 欠落・空の観測は、全機能選択でも失敗する。
    let bad_attr = format!("{ok}        A: http://schemas.android.com/apk/res/android:supportsPictureInPicture(0x010104f7)=false\n");
    let e = check("android", &m, &bad_attr).unwrap_err();
    assert!(e.contains("supportsPictureInPicture"), "{e}");
    let bad_flag = format!(
        "{}{MAIN_ACTIVITY_CLEAN}{pip}",
        scan_perm(Some("0x00000002"))
    );
    assert!(check("android", &m, &bad_flag).is_err());
    assert!(check("android", &m, &scan_perm(None))
        .unwrap_err()
        .contains("MainActivity"));
    assert!(check("android", &m, "").is_err());
}

/// aapt2 2.20 の `dump xmltree --file AndroidManifest.xml` の実出力の形に合わせて組んだ行（識別子は一般化済み）。
/// 値の形（`"v" (Raw: "v")`・素の `true`/`false`・`0x%08x`・`@0x7f…`）は実 APK で確かめたものだが、
/// `usesPermissionFlags` の行は手元の APK に無かったため、AOSP の `BinaryPrimitive::PrettyPrint`（`TYPE_INT_HEX`）に従った合成。
#[test]
fn real_aapt2_output_forms_pass_through_verify_scope() {
    let m = make_ws("real-forms", &deps(&["cam", "ble", "pip"]), &[]);
    let obs = "\
N: android=http://schemas.android.com/apk/res/android (line=3)
  E: manifest (line=3)
    A: http://schemas.android.com/apk/res/android:versionCode(0x0101021b)=1
    A: package=\"com.example.app\" (Raw: \"com.example.app\")
    A: platformBuildVersionCode=35
      E: uses-permission (line=12)
        A: http://schemas.android.com/apk/res/android:name(0x01010003)=\"android.permission.CAMERA\" (Raw: \"android.permission.CAMERA\")
      E: uses-permission (line=157)
        A: http://schemas.android.com/apk/res/android:name(0x01010003)=\"android.permission.BLUETOOTH_SCAN\" (Raw: \"android.permission.BLUETOOTH_SCAN\")
        A: http://schemas.android.com/apk/res/android:usesPermissionFlags(0x01010644)=0x00010000
      E: application (line=40)
        A: http://schemas.android.com/apk/res/android:theme(0x01010000)=@0x7f0d0005
        A: http://schemas.android.com/apk/res/android:allowBackup(0x01010280)=false
        E: activity (line=45)
            A: http://schemas.android.com/apk/res/android:name(0x01010003)=\"com.fandhe.platform.MainActivity\" (Raw: \"com.fandhe.platform.MainActivity\")
            A: http://schemas.android.com/apk/res/android:exported(0x01010010)=true
            A: http://schemas.android.com/apk/res/android:launchMode(0x0101001d)=2
            A: http://schemas.android.com/apk/res/android:configChanges(0x0101001f)=0x5000ffff
            A: http://schemas.android.com/apk/res/android:supportsPictureInPicture(0x010104f7)=true
";
    check("android", &m, obs).unwrap();
    // 未選択（nfc・both・scan）側から見ると、同じ出力のフラグ・属性は宣言どおりでなければ検出される。
    let m2 = make_ws("real-forms-nopip", &deps(&["cam", "ble"]), &[]);
    let e = check("android", &m2, obs).unwrap_err();
    assert!(e.contains("supportsPictureInPicture"), "{e}");
}

#[test]
fn unselected_backup_exclusion_fails_and_selected_passes() {
    // 観測は aapt2 が <application> の参照先 XML を dump した形（`A: path="…" (Raw: "…")`）。
    let rule = "      E: exclude (line=5)\n        A: domain=\"sharedpref\" (Raw: \"sharedpref\")\n        A: path=\"fandhe_bk.xml\" (Raw: \"fandhe_bk.xml\")\n";
    let m = make_ws("bk-leak", SELECT_CAM, &[]);
    let e = check("android", &m, &format!("{ANDROID_CLEAN}{rule}")).unwrap_err();
    assert!(
        e.contains("bk") && e.contains("バックアップ除外") && e.contains("fandhe_bk.xml"),
        "{e}"
    );
    let m = make_ws("bk-ok", &deps(&["cam", "bk"]), &[]);
    check("android", &m, &format!("{ANDROID_CLEAN}{rule}")).unwrap();
    // domain 違いの同名除外は別物。選んだ機能が file の同名を除外していても、未選択の bk の sharedpref の除外は検出する。
    let bkf = cap(
        "bkf",
        "[package.metadata.fandhe.android]\nbackup-exclusions = [{ domain = \"file\", path = \"fandhe_bk.xml\" }]\n",
    );
    let other = rule.replace("\"sharedpref\"", "\"file\"");
    let m = make_ws(
        "bk-domain",
        &deps(&["cam", "bkf"]),
        &[(bkf.0.as_str(), bkf.1.clone())],
    );
    check("android", &m, &format!("{ANDROID_CLEAN}{other}")).unwrap();
    let e = check("android", &m, &format!("{ANDROID_CLEAN}{other}{rule}")).unwrap_err();
    assert!(e.contains("sharedpref:fandhe_bk.xml"), "{e}");
}
