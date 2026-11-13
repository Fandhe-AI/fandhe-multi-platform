//! `cargo fandhe gen`: アプリが選んだ機能の宣言だけからネイティブ設定を生成する（REQ-21 / TASK-21.3、Issue #1176）。
//!
//! 呼び出し元は `main.rs` の `gen` サブコマンド（と統合テスト）。入力は `cargo metadata` の解決結果で、
//! どの機能 crate を含めるかは `reach` が決め、宣言の統合は `merge` が行う。ここは統合結果を
//! 各 OS の文法（XML・plist・Groovy）へ直列化する層で、値はすべて出力先の文法でエスケープする
//! （宣言は第三者の crate からも来る。security.md A03）。
//!
//! 殻（Android の Gradle・Manifest、iOS の Info.plist）が要る値のうち、機能の宣言とアプリの crate から
//! 決まるものもここで出す（#1259）。基盤の Gradle 依存は `fandhe-platform-android` の宣言から、
//! MainActivity の宣言（exported・launchMode・configChanges・ライブラリ名・LAUNCHER）と
//! `FandheLibrary.NAME`、iOS の確定値の Info.plist（実行体名・bundle id・版・`UIDeviceFamily` 等）は
//! アプリの crate（lib/bin target・version）と `fandhe-app.ios.bundle-id` から出す。
//!
//! 出力先は `<アプリの crate>/gen/` に固定する（consumer-app-structure.md §4、gitignore する）。
//! Info.plist の説明文（`*UsageDescription`）は既定値を持たず、アプリ側の
//! `[package.metadata.fandhe-app]` に無ければエラーにする（既定文言のまま審査に出る事故を防ぐ）。
//! 範囲外: `CFBundleSupportedPlatforms`（シミュレータか実機かは組み立て時に決まる、#1201）、
//! applicationId・compileSdk・`<application>` の属性、macOS・拡張の Info.plist の確定値化、
//! Xcode のターゲット・framework のリンク（#1212）、Windows の identity（#1208）、
//! 既存設定からの切り替え（#1194）、run/build への組み込み（#1201）。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::merge::{merge, AndroidMerged, AppleMerged};
use crate::reach::{lib_name, reachable};
use crate::workspace::{collect, run_metadata, Found};

/// 生成物の印。これがある `gen/` だけを次回の生成で置き換える（利用者のディレクトリを消さない）。
const MARKER: &str = ".fandhe-gen";
const MAIN_ACTIVITY: &str = "com.fandhe.platform.MainActivity";
/// Android の MainActivity が受け取る設定変更。受け取らないと Activity が作り直され、winit が古い描画スレッドを
/// 残して黒画面・ANR になる（#73）。Rust の実行層の前提でアプリ固有ではない。古い端末では未知の flag は無視される。
const MAIN_ACTIVITY_CONFIG_CHANGES: &str = "orientation|screenSize|keyboardHidden|uiMode|fontScale|density|locale|layoutDirection|smallestScreenSize|screenLayout|colorMode|fontWeightAdjustment|keyboard|navigation|touchscreen|mcc|mnc|grammaticalGender";
/// iOS の最低対応版。`.cargo/config.toml` の `IPHONEOS_DEPLOYMENT_TARGET` と揃える（一致は tests/gen.rs が保証する）。
const IOS_MIN_VERSION: &str = "17.0";
/// ライブラリ名を Java から参照させるクラス（アプリは `System.loadLibrary(FandheLibrary.NAME)` と書く）。
const LIBRARY_CLASS_PATH: &str = "android/java/com/fandhe/platform/FandheLibrary.java";
/// OS ごとに `cargo metadata --filter-platform` へ渡す triple（対象 OS に限った依存の cfg 評価は cargo に任せる）。
pub(crate) const ANDROID_TRIPLE: &str = "aarch64-linux-android";
pub(crate) const IOS_TRIPLE: &str = "aarch64-apple-ios";
pub(crate) const MACOS_TRIPLE: &str = "aarch64-apple-darwin";
pub(crate) const WEB_TRIPLE: &str = "wasm32-unknown-unknown";
/// Web の出力先（`gen/` からの相対）。配信側（`scripts/web/build-wasm-dom.sh`）がここから dist へ写す。
const WEB_MANIFEST_PATH: &str = "web/manifest.webmanifest";
pub(crate) const APP_GROUPS_KEY: &str = "com.apple.security.application-groups";

/// 生成結果。`files` のキーは `gen_dir` からの相対パス（区切りは `/`）。
#[derive(Debug)]
pub struct Generated {
    pub gen_dir: PathBuf,
    pub files: BTreeMap<String, String>,
}

/// アプリ側の入力 `[package.metadata.fandhe-app]`。`metadata.fandhe` と別キーなのは、
/// 機能宣言として解釈されて `id` 必須で失敗するのを避けるため。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct AppConfig {
    /// App Group を使う機能があるときだけ必須。`com.example` のようなドット区切り。
    app_group_prefix: Option<String>,
    #[serde(default)]
    ios: OsConfig,
    #[serde(default)]
    macos: OsConfig,
    /// Web を出すアプリだけが書く。無いアプリには `web/` を出さない。
    web: Option<WebConfig>,
}

/// アプリの基礎 manifest。名前・アイコン・`start_url` などアプリが持つ値だけを書く。
/// 機能が持つメンバー（`share_target` 等）は書けず、`gen` が到達した機能の宣言から足す。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct WebConfig {
    /// アプリの crate からの相対パス。
    manifest: String,
}

/// 本体と拡張の Info.plist が共有する説明文（キー → 文言）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct OsConfig {
    #[serde(default)]
    usage_descriptions: BTreeMap<String, String>,
    /// iOS のバンドル ID（`CFBundleIdentifier`）。iOS の Info.plist を出すとき必須。macOS はまだ使わない。
    bundle_id: Option<String>,
}

/// アプリの crate の情報（`cargo metadata` の root package）。殻に要る値の出どころ。
#[derive(Debug, Clone)]
pub(crate) struct AppInfo {
    pub name: String,
    pub version: String,
    /// `cdylib` を含む lib target の名前（Android の `.so` の元）。無ければ `None`。
    pub cdylib: Option<String>,
    pub bins: Vec<String>,
    /// 生成先（`<アプリの crate>/gen`）の絶対パス。Gradle から参照する。
    pub gen_dir: PathBuf,
}

impl AppInfo {
    fn from_package(pkg: &Value, gen_dir: PathBuf) -> Self {
        let targets: Vec<&Value> = pkg["targets"].as_array().into_iter().flatten().collect();
        let has = |t: &Value, key: &str, want: &str| {
            t[key]
                .as_array()
                .into_iter()
                .flatten()
                .any(|k| k.as_str() == Some(want))
        };
        let cdylib = targets
            .iter()
            .any(|t| has(t, "crate_types", "cdylib"))
            .then(|| lib_name(pkg));
        AppInfo {
            name: pkg["name"].as_str().unwrap_or_default().to_string(),
            version: pkg["version"].as_str().unwrap_or_default().to_string(),
            cdylib,
            bins: targets
                .iter()
                .filter(|t| has(t, "kind", "bin"))
                .filter_map(|t| t["name"].as_str().map(String::from))
                .collect(),
            gen_dir,
        }
    }
}

/// ドット区切り（2 セグメント以上）の英数字とハイフン。App Group の接頭辞と bundle id が共用する。
fn is_reverse_dns(p: &str) -> bool {
    p.split('.').count() >= 2
        && p.split('.')
            .all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

fn is_ident(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl AppConfig {
    fn parse(v: &Value) -> Result<Self, Vec<String>> {
        let cfg: AppConfig = if v.is_null() {
            AppConfig::default()
        } else {
            serde_json::from_value(v.clone())
                .map_err(|e| vec![format!("[package.metadata.fandhe-app]: {e}")])?
        };
        let mut errs = Vec::new();
        if let Some(p) = &cfg.app_group_prefix {
            if !is_reverse_dns(p) {
                errs.push(format!("fandhe-app.app-group-prefix: `{p}` はドット区切り（2 セグメント以上）の英数字とハイフンにしてください"));
            }
        }
        if let Some(b) = &cfg.ios.bundle_id {
            if !is_reverse_dns(b) {
                errs.push(format!("fandhe-app.ios.bundle-id: `{b}` はドット区切り（2 セグメント以上）の英数字とハイフンにしてください"));
            }
        }
        if let Some(w) = &cfg.web {
            let m = &w.manifest;
            if m.is_empty()
                || m.starts_with('/')
                || m.contains(['\\', ':'])
                || m.chars().any(char::is_control)
                || m.split('/').any(|s| s == "..")
            {
                errs.push(format!("fandhe-app.web.manifest: `{m}` はアプリの crate からの相対パスにしてください（空・絶対・`..`・`\\`・`:` は不可）"));
            }
        }
        if cfg.macos.bundle_id.is_some() {
            errs.push("fandhe-app.macos.bundle-id: macOS ではまだ使いません（Info.plist は Xcode の変数のままです）".to_string());
        }
        for (os, c) in [("ios", &cfg.ios), ("macos", &cfg.macos)] {
            for (k, t) in &c.usage_descriptions {
                if !(is_ident(k) && k.ends_with("UsageDescription")) {
                    errs.push(format!("fandhe-app.{os}.usage-descriptions: `{k}` は `*UsageDescription` のキーにしてください"));
                }
                if t.trim().is_empty() || t.chars().any(char::is_control) {
                    errs.push(format!("fandhe-app.{os}.usage-descriptions.{k}: 文言は空白だけ・制御文字を含む値にできません"));
                }
            }
        }
        if errs.is_empty() {
            Ok(cfg)
        } else {
            Err(errs)
        }
    }
}

// ---- エスケープ・直列化 ----

fn esc_xml(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c => o.push(c),
        }
    }
    o
}

/// Groovy の単一引用符の文字列（補間なし）にする。
fn groovy_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// plist の値を直列化する。キーの順序は決定的にするためソートする。null は表現できないので拒否する。
fn plist_value(v: &Value, depth: usize, at: &str, out: &mut String) -> Result<(), String> {
    let pad = "\t".repeat(depth);
    match v {
        Value::String(s) => out.push_str(&format!("{pad}<string>{}</string>\n", esc_xml(s))),
        Value::Bool(b) => out.push_str(&format!("{pad}<{b}/>\n")),
        Value::Number(n) if n.is_f64() => out.push_str(&format!("{pad}<real>{n}</real>\n")),
        Value::Number(n) => out.push_str(&format!("{pad}<integer>{n}</integer>\n")),
        Value::Array(a) => {
            out.push_str(&format!("{pad}<array>\n"));
            for (i, x) in a.iter().enumerate() {
                plist_value(x, depth + 1, &format!("{at}[{i}]"), out)?;
            }
            out.push_str(&format!("{pad}</array>\n"));
        }
        Value::Object(m) => plist_dict(m, depth, at, out)?,
        Value::Null => return Err(format!("{at}: plist は null を表せません")),
    }
    Ok(())
}

fn plist_dict(
    m: &Map<String, Value>,
    depth: usize,
    at: &str,
    out: &mut String,
) -> Result<(), String> {
    let pad = "\t".repeat(depth);
    out.push_str(&format!("{pad}<dict>\n"));
    let sorted: BTreeMap<_, _> = m.iter().collect();
    for (k, v) in sorted {
        out.push_str(&format!("{pad}\t<key>{}</key>\n", esc_xml(k)));
        plist_value(v, depth + 1, &format!("{at}.{k}"), out)?;
    }
    out.push_str(&format!("{pad}</dict>\n"));
    Ok(())
}

fn plist_doc(m: &Map<String, Value>, at: &str) -> Result<String, String> {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!-- cargo fandhe gen が生成。編集しない -->\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n",
    );
    plist_dict(m, 0, at, &mut out)?;
    out.push_str("</plist>\n");
    Ok(out)
}

// ---- Android ----

fn xml_attr(k: &str, v: &str) -> String {
    format!(" android:{k}=\"{}\"", esc_xml(v))
}

fn filter_xml(f: &crate::declaration::IntentFilter, pad: &str, out: &mut String) {
    out.push_str(&format!("{pad}<intent-filter>\n"));
    for a in &f.actions {
        out.push_str(&format!("{pad}    <action{} />\n", xml_attr("name", a)));
    }
    for c in &f.categories {
        out.push_str(&format!("{pad}    <category{} />\n", xml_attr("name", c)));
    }
    for d in &f.data {
        let mut attrs = String::new();
        for (k, v) in [
            ("scheme", &d.scheme),
            ("host", &d.host),
            ("mimeType", &d.mime_type),
            ("pathPrefix", &d.path_prefix),
        ] {
            if let Some(v) = v {
                attrs.push_str(&xml_attr(k, v));
            }
        }
        out.push_str(&format!("{pad}    <data{attrs} />\n"));
    }
    out.push_str(&format!("{pad}</intent-filter>\n"));
}

/// manifest merger 用の overlay。`package` 属性は持たず、アプリ本体の Manifest へ足される。
/// MainActivity の宣言は常に出す（`MAIN_ACTIVITY` は cargo-fandhe の契約）。機能の intent-filter はその後に続ける。
fn android_manifest(m: &AndroidMerged, lib: &str) -> Result<String, String> {
    let mut o = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- cargo fandhe gen が生成。編集しない -->\n<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n",
    );
    for (name, max) in &m.permissions {
        let max = max
            .map(|n| xml_attr("maxSdkVersion", &n.to_string()))
            .unwrap_or_default();
        o.push_str(&format!(
            "    <uses-permission{}{max} />\n",
            xml_attr("name", name)
        ));
    }
    for (name, req) in &m.features {
        o.push_str(&format!(
            "    <uses-feature{}{} />\n",
            xml_attr("name", name),
            xml_attr("required", &req.to_string())
        ));
    }
    {
        o.push_str("    <application>\n");
        o.push_str(&format!(
            "        <activity{}{}{}{}>\n",
            xml_attr("name", MAIN_ACTIVITY),
            xml_attr("exported", "true"),
            xml_attr("launchMode", "singleTask"),
            xml_attr("configChanges", MAIN_ACTIVITY_CONFIG_CHANGES)
        ));
        o.push_str(&format!(
            "            <meta-data{}{} />\n",
            xml_attr("name", "android.app.lib_name"),
            xml_attr("value", lib)
        ));
        o.push_str("            <intent-filter>\n                <action android:name=\"android.intent.action.MAIN\" />\n                <category android:name=\"android.intent.category.LAUNCHER\" />\n            </intent-filter>\n");
        for f in &m.main_activity_intent_filters {
            filter_xml(f, "            ", &mut o);
        }
        o.push_str("        </activity>\n");
        for (name, c) in &m.components {
            let kind = format!("{:?}", c.kind).to_lowercase();
            let mut attrs = xml_attr("name", name) + &xml_attr("exported", &c.exported.to_string());
            for (k, v) in &c.attributes {
                if !is_ident(k) {
                    return Err(format!("{name}: 属性名 `{k}` が不正です"));
                }
                attrs.push_str(&xml_attr(k, v));
            }
            o.push_str(&format!("        <{kind}{attrs}>\n"));
            for f in &c.intent_filters {
                filter_xml(f, "            ", &mut o);
            }
            for md in &c.meta_data {
                let val = match (&md.resource, &md.value) {
                    (Some(r), _) => xml_attr("resource", r),
                    (None, Some(v)) => xml_attr("value", v),
                    (None, None) => String::new(),
                };
                o.push_str(&format!(
                    "            <meta-data{}{val} />\n",
                    xml_attr("name", &md.name)
                ));
            }
            o.push_str(&format!("        </{kind}>\n"));
        }
        o.push_str("    </application>\n");
    }
    o.push_str("</manifest>\n");
    Ok(o)
}

/// Gradle の apply 用スクリプト。srcDirs は crate の絶対パス（区切りは `/` に正規化）。
/// 常に出す: gen の Java（`FandheLibrary`）と Manifest を取り込む配線が要るため。
/// Manifest はアプリ本体の Manifest を残したまま、全 variant の overlay として manifest merger に載せる。
fn android_gradle(m: &AndroidMerged, dirs: &BTreeMap<&str, &Path>, gen_dir: &Path) -> String {
    let norm = |p: &Path| groovy_str(&p.to_string_lossy().replace('\\', "/"));
    let src = |items: &[(String, String)]| -> Vec<String> {
        items
            .iter()
            .filter_map(|(id, rel)| Some(norm(&dirs.get(id.as_str())?.join(rel))))
            .collect()
    };
    let (java, res) = (src(&m.java), src(&m.res));
    let mut o = String::from("// cargo fandhe gen が生成。編集しない\n");
    o.push_str("android {\n    sourceSets {\n        main {\n");
    o.push_str(&format!(
        "            java.srcDir {}\n",
        norm(&gen_dir.join("android/java"))
    ));
    if !java.is_empty() {
        o.push_str(&format!(
            "            java.srcDirs += [{}]\n",
            java.join(", ")
        ));
    }
    if !res.is_empty() {
        o.push_str(&format!(
            "            res.srcDirs += [{}]\n",
            res.join(", ")
        ));
    }
    o.push_str("        }\n    }\n}\n");
    o.push_str(&format!(
        "androidComponents {{\n    onVariants(selector().all()) {{ variant ->\n        variant.sources.manifests.addStaticManifestFile({})\n    }}\n}}\n",
        norm(&gen_dir.join("android/AndroidManifest.xml"))
    ));
    if !m.gradle.is_empty() {
        o.push_str("dependencies {\n");
        for g in &m.gradle {
            o.push_str(&format!("    implementation {}\n", groovy_str(g)));
        }
        o.push_str("}\n");
    }
    o
}

/// ライブラリ名の定数クラス。manifest の `android.app.lib_name` と `[lib] name` の 3 か所一致を 1 か所の正本にする。
fn library_class(lib: &str) -> String {
    format!(
        "// cargo fandhe gen が生成。編集しない\npackage com.fandhe.platform;\n\n/** アプリの cdylib 名（`[lib] name`）。`System.loadLibrary(FandheLibrary.NAME)` に使う。 */\npublic final class FandheLibrary {{\n    private FandheLibrary() {{}}\n\n    public static final String NAME = \"{lib}\";\n}}\n"
    )
}

// ---- Apple ----

/// 最小の Info.plist。値は Xcode のビルド設定変数で、アプリ固有の値は書かない。
fn plist_template() -> Map<String, Value> {
    [
        ("CFBundleExecutable", "$(EXECUTABLE_NAME)"),
        ("CFBundleIdentifier", "$(PRODUCT_BUNDLE_IDENTIFIER)"),
        ("CFBundleInfoDictionaryVersion", "6.0"),
        ("CFBundleName", "$(PRODUCT_NAME)"),
        ("CFBundlePackageType", "$(PRODUCT_BUNDLE_PACKAGE_TYPE)"),
        ("CFBundleShortVersionString", "$(MARKETING_VERSION)"),
        ("CFBundleVersion", "$(CURRENT_PROJECT_VERSION)"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Value::String(v.into())))
    .collect()
}

struct AppleCtx<'a> {
    os: &'a str,
    cfg: &'a OsConfig,
    prefix: Option<&'a str>,
    /// この OS に到達した (id, 宣言)。説明文が無いときに要求元の機能を示す。
    found: &'a [&'a Found],
}

impl AppleCtx<'_> {
    fn requesters(&self, key: &str) -> String {
        let ids: Vec<&str> = self
            .found
            .iter()
            .filter(|f| {
                let a = if self.os == "ios" {
                    &f.decl.ios
                } else {
                    &f.decl.macos
                };
                a.as_ref().is_some_and(|a| {
                    a.usage_descriptions.iter().any(|k| k == key)
                        || a.extensions
                            .iter()
                            .any(|e| e.usage_descriptions.iter().any(|k| k == key))
                })
            })
            .map(|f| f.decl.id.as_str())
            .collect();
        ids.join(", ")
    }

    fn plist(
        &self,
        base: Map<String, Value>,
        keys: &BTreeSet<String>,
        info: &Map<String, Value>,
        errs: &mut Vec<String>,
    ) -> Map<String, Value> {
        let mut m = base;
        m.extend(info.clone());
        for k in keys {
            match self.cfg.usage_descriptions.get(k) {
                Some(t) => {
                    m.insert(k.clone(), Value::String(t.clone()));
                }
                None => errs.push(format!(
                    "{}: 機能 [{}] が要求する {k} の文言がありません。[package.metadata.fandhe-app.{}.usage-descriptions] に書いてください",
                    self.os,
                    self.requesters(k),
                    self.os
                )),
            }
        }
        m
    }

    fn entitlements(
        &self,
        base: &Map<String, Value>,
        groups: &BTreeSet<String>,
        at: &str,
        errs: &mut Vec<String>,
    ) -> Map<String, Value> {
        let mut m = base.clone();
        if groups.is_empty() {
            return m;
        }
        let Some(prefix) = self.prefix else {
            errs.push(format!(
                "{at}: app-groups を使う機能があります。[package.metadata.fandhe-app] に app-group-prefix を書いてください"
            ));
            return m;
        };
        let ids = groups.iter().map(|g| {
            Value::String(if self.os == "ios" {
                format!("group.{prefix}.{g}")
            } else {
                format!("$(TeamIdentifierPrefix){prefix}.{g}")
            })
        });
        m.insert(APP_GROUPS_KEY.into(), Value::Array(ids.collect()));
        m
    }
}

/// iOS 本体の土台。Xcode の変数ではなく確定値を書く（Xcode を通さない組み立てでもそのまま使える）。
/// `CFBundleSupportedPlatforms` は組み立て時の値（シミュレータか実機か）のため出さない（#1201）。
fn ios_base(app: &AppInfo, cfg: &OsConfig) -> Result<Map<String, Value>, Vec<String>> {
    let mut errs = Vec::new();
    let bundle = cfg.bundle_id.clone().unwrap_or_else(|| {
        errs.push("ios: Info.plist に bundle id が要ります。[package.metadata.fandhe-app.ios] に bundle-id を書いてください".into());
        String::new()
    });
    if app.bins.len() != 1 {
        errs.push(format!(
            "ios: 実行体（bin target）がちょうど 1 つ要りますが {} 個あります: [{}]",
            app.bins.len(),
            app.bins.join(", ")
        ));
    }
    // CFBundleShortVersionString は数字とドットだけ。pre-release・build metadata は載せられない。
    let v = &app.version;
    if v.is_empty()
        || !v
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        errs.push(format!("ios: version `{v}` は数字とドットだけの版にしてください（pre-release・build metadata は使えません）"));
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    let s = |x: &str| Value::String(x.to_string());
    Ok([
        ("CFBundleExecutable", s(&app.bins[0])),
        ("CFBundleIdentifier", s(&bundle)),
        ("CFBundleInfoDictionaryVersion", s("6.0")),
        ("CFBundleName", s(&app.name)),
        ("CFBundlePackageType", s("APPL")),
        ("CFBundleShortVersionString", s(v)),
        ("CFBundleVersion", s(v)),
        ("MinimumOSVersion", s(IOS_MIN_VERSION)),
        ("UIDeviceFamily", Value::Array(vec![1.into(), 2.into()])),
        ("UILaunchScreen", Value::Object(Map::new())),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect())
}

fn apple_files(
    ctx: &AppleCtx,
    base: Map<String, Value>,
    m: &AppleMerged,
    files: &mut BTreeMap<String, String>,
    errs: &mut Vec<String>,
) {
    let os = ctx.os;
    let mut local = Vec::new();
    let plist = ctx.plist(base, &m.usage_descriptions, &m.info_plist, &mut local);
    let ent = ctx.entitlements(&m.entitlements, &m.app_groups, os, &mut local);
    let mut outs = vec![(format!("{os}/Info.plist"), plist_doc(&plist, "Info.plist"))];
    if !ent.is_empty() {
        outs.push((
            format!("{os}/App.entitlements"),
            plist_doc(&ent, "entitlements"),
        ));
    }
    for (name, e) in &m.extensions {
        let plist = ctx.plist(
            plist_template(),
            &e.usage_descriptions.iter().cloned().collect(),
            &e.info_plist,
            &mut local,
        );
        let ent = ctx.entitlements(
            &e.entitlements,
            &e.app_groups.iter().cloned().collect(),
            name,
            &mut local,
        );
        outs.push((
            format!("{os}/extensions/{name}/Info.plist"),
            plist_doc(&plist, name),
        ));
        if !ent.is_empty() {
            outs.push((
                format!("{os}/extensions/{name}/{name}.entitlements"),
                plist_doc(&ent, name),
            ));
        }
    }
    local.sort();
    local.dedup();
    errs.extend(local);
    for (p, r) in outs {
        match r {
            Ok(s) => {
                files.insert(p, s);
            }
            Err(e) => errs.push(format!("{os}: {e}")),
        }
    }
}

// ---- 生成本体 ----

/// 到達した機能 (OS ごと) とアプリ設定から、`gen/` 配下の相対パス → 内容を作る。純粋関数。
pub(crate) fn render(
    android: &[&Found],
    ios: &[&Found],
    macos: &[&Found],
    cfg: &AppConfig,
    app: &AppInfo,
) -> Result<BTreeMap<String, String>, Vec<String>> {
    let mut errs: Vec<String> = Vec::new();
    let mut files = BTreeMap::new();
    // 対象 OS 以外の欄を落としてから統合する（未到達 OS の欄の競合で全体を失敗させない）。
    let merged = |os: &str, set: &[&Found], errs: &mut Vec<String>| {
        let decls: Vec<_> = set
            .iter()
            .map(|f| {
                let mut d = f.decl.clone();
                if os != "ios" {
                    d.ios = None;
                }
                if os != "macos" {
                    d.macos = None;
                }
                if os != "android" {
                    d.android = None;
                }
                d.web = None;
                d
            })
            .collect();
        let refs: Vec<_> = decls.iter().map(|d| (d.id.as_str(), d)).collect();
        merge(&refs)
            .map_err(|e| errs.extend(e.into_iter().map(|e| e.to_string())))
            .ok()
    };

    if let Some(m) = merged("android", android, &mut errs).map(|m| m.android) {
        match &app.cdylib {
            None => errs.push(format!(
                "android: {} に cdylib の lib target がありません。Cargo.toml の [lib] に crate-type = [\"cdylib\", ...] を足してください",
                app.name
            )),
            Some(lib) if !is_ident(lib) => {
                errs.push(format!("android: lib 名 `{lib}` は英数字と `_` の識別子にしてください"))
            }
            Some(lib) => {
                let dirs: BTreeMap<&str, &Path> = android
                    .iter()
                    .map(|f| (f.decl.id.as_str(), f.dir.as_path()))
                    .collect();
                match android_manifest(&m, lib) {
                    Ok(s) => {
                        files.insert("android/AndroidManifest.xml".to_string(), s);
                    }
                    Err(e) => errs.push(format!("android: {e}")),
                }
                files.insert(
                    "android/fandhe.gradle".to_string(),
                    android_gradle(&m, &dirs, &app.gen_dir),
                );
                files.insert(LIBRARY_CLASS_PATH.to_string(), library_class(lib));
            }
        }
    }
    for (os, set) in [("ios", ios), ("macos", macos)] {
        let Some(m) = merged(os, set, &mut errs).map(|m| if os == "ios" { m.ios } else { m.macos })
        else {
            continue;
        };
        let base = if os == "ios" {
            match ios_base(app, &cfg.ios) {
                Ok(b) => b,
                Err(e) => {
                    errs.extend(e);
                    continue;
                }
            }
        } else {
            if m == AppleMerged::default() {
                continue;
            }
            plist_template()
        };
        let ctx = AppleCtx {
            os,
            cfg: if os == "ios" { &cfg.ios } else { &cfg.macos },
            prefix: cfg.app_group_prefix.as_deref(),
            found: set,
        };
        apple_files(&ctx, base, &m, &mut files, &mut errs);
    }
    errs.sort();
    errs.dedup();
    if !errs.is_empty() {
        return Err(errs);
    }
    files.insert(
        MARKER.into(),
        "cargo fandhe gen が生成したディレクトリ。次回の生成で置き換わる\n".into(),
    );
    Ok(files)
}

/// metadata の root パッケージ（アプリ）の (crate のディレクトリ, `metadata.fandhe-app`, package)。
fn app_root(meta: &Value) -> Result<(PathBuf, Value, Value), Vec<String>> {
    let root = meta["resolve"]["root"].as_str().ok_or_else(|| {
        vec!["アプリのパッケージを特定できません。--manifest-path にパッケージの Cargo.toml を指定してください".to_string()]
    })?;
    let pkg = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["id"].as_str() == Some(root))
        .ok_or_else(|| vec![format!("root パッケージ {root} が packages にありません")])?;
    let dir = pkg["manifest_path"]
        .as_str()
        .and_then(|p| Path::new(p).parent())
        .ok_or_else(|| vec!["manifest_path を読めません".to_string()])?;
    Ok((
        dir.to_path_buf(),
        pkg["metadata"]["fandhe-app"].clone(),
        pkg.clone(),
    ))
}

/// `manifest`（省略時はカレントのパッケージ）のアプリが選んだ機能から、ネイティブ設定を生成する。
/// 宣言・入力に問題が 1 件でもあれば何も返さない（check と同じ厳しさ）。書き出しは [`write`]。
pub fn generate(manifest: Option<&Path>) -> Result<Generated, Vec<String>> {
    let mut sets: Vec<Vec<Found>> = Vec::new();
    let mut app: Option<(PathBuf, Value, Value)> = None;
    for triple in [ANDROID_TRIPLE, IOS_TRIPLE, MACOS_TRIPLE] {
        let meta = run_metadata(manifest, Some(triple))?;
        if app.is_none() {
            app = Some(app_root(&meta)?);
        }
        let reach = reachable(&meta);
        sets.push(
            collect(&meta)?
                .into_iter()
                .filter(|f| reach.contains(&f.pkg_id))
                .collect(),
        );
    }
    let (dir, cfg, pkg) = app.expect("ループで必ず設定される");
    let cfg = AppConfig::parse(&cfg)?;
    let info = AppInfo::from_package(&pkg, dir.join("gen"));
    let r = |i: usize| sets[i].iter().collect::<Vec<_>>();
    let files = render(&r(0), &r(1), &r(2), &cfg, &info)?;
    Ok(Generated {
        gen_dir: dir.join("gen"),
        files,
    })
}

/// Web だけを生成する（`cargo fandhe gen --os web`、#1238）。bin や iOS 設定を持たない Web 専用の
/// アプリ（fandhe-web-dom）が、Android・iOS の殻の必須条件に阻まれず manifest を得るための入口。
/// 基礎 manifest（`fandhe-app.web.manifest`）に、wasm32 で到達した機能の `web.manifest` を足して出す。
/// 選ばなかった機能のメンバーは入らない。`fandhe-app.web` が無いアプリはエラー（何も出ないため）。
pub fn generate_web(manifest: Option<&Path>) -> Result<Generated, Vec<String>> {
    let meta = run_metadata(manifest, Some(WEB_TRIPLE))?;
    let (dir, cfg, _) = app_root(&meta)?;
    let cfg = AppConfig::parse(&cfg)?;
    let web = cfg.web.ok_or_else(|| {
        vec!["[package.metadata.fandhe-app.web] manifest がありません。Web の出力には基礎 manifest の指定が要ります".to_string()]
    })?;
    let reach = reachable(&meta);
    let found: Vec<Found> = collect(&meta)?
        .into_iter()
        .filter(|f| reach.contains(&f.pkg_id))
        .collect();
    let body = render_web(
        &found.iter().collect::<Vec<_>>(),
        &read_base_manifest(&dir, &web.manifest)?,
    )?;
    Ok(Generated {
        gen_dir: dir.join("gen"),
        files: BTreeMap::from([
            (WEB_MANIFEST_PATH.to_string(), body),
            (
                MARKER.to_string(),
                "cargo fandhe gen が生成したディレクトリ。次回の生成で置き換わる\n".to_string(),
            ),
        ]),
    })
}

/// 基礎 manifest を読む。crate の外（symlink 経由を含む）は拒否し、読めない・JSON のオブジェクトでないときはエラー
/// （黙って機能のメンバーだけの manifest を出さない）。
fn read_base_manifest(crate_dir: &Path, rel: &str) -> Result<Map<String, Value>, Vec<String>> {
    let err = |m: String| vec![format!("fandhe-app.web.manifest: {m}")];
    let root = crate_dir
        .canonicalize()
        .map_err(|e| err(format!("{} を解決できません: {e}", crate_dir.display())))?;
    let path = root
        .join(rel)
        .canonicalize()
        .map_err(|e| err(format!("`{rel}` を解決できません: {e}")))?;
    if !path.starts_with(&root) {
        return Err(err(format!("`{rel}` が crate の外を指しています")));
    }
    let text = fs::read_to_string(&path).map_err(|e| err(format!("`{rel}` を読めません: {e}")))?;
    match serde_json::from_str(&text) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err(err(format!("`{rel}` は JSON のオブジェクトにしてください"))),
        Err(e) => Err(err(format!("`{rel}` を JSON として読めません: {e}"))),
    }
}

/// 到達した機能の `web.manifest` を統合し、基礎 manifest に足して整形済みの JSON にする。純粋関数。
/// 基礎が機能の持つメンバーを書いていたらエラー（二重管理と、選ばない機能の値の残存を防ぐ）。
pub(crate) fn render_web(web: &[&Found], base: &Map<String, Value>) -> Result<String, Vec<String>> {
    let decls: Vec<_> = web
        .iter()
        .map(|f| {
            let mut d = f.decl.clone();
            d.ios = None;
            d.macos = None;
            d.android = None;
            d
        })
        .collect();
    let refs: Vec<_> = decls.iter().map(|d| (d.id.as_str(), d)).collect();
    let merged =
        merge(&refs).map_err(|e| e.into_iter().map(|e| e.to_string()).collect::<Vec<_>>())?;
    // 許可リストのメンバーは基礎に書けない（選ばなかった機能の値が残る・二重管理になるため）。
    if let Some(k) = crate::declaration::WEB_MANIFEST_MEMBERS
        .iter()
        .find(|k| base.contains_key(**k))
    {
        return Err(vec![format!(
            "fandhe-app.web.manifest: `{k}` は機能の宣言から足すメンバーです。基礎 manifest から削除してください"
        )]);
    }
    let mut out = base.clone();
    out.extend(merged.web_manifest);
    let mut s = serde_json::to_string_pretty(&Value::Object(out))
        .map_err(|e| vec![format!("manifest を直列化できません: {e}")])?;
    s.push('\n');
    Ok(s)
}

/// 印が通常ファイルとして在るか。symlink は印と認めない（書き込みがリンク先を上書きするため）。
fn has_marker(gen_dir: &Path) -> bool {
    fs::symlink_metadata(gen_dir.join(MARKER)).is_ok_and(|m| m.is_file())
}

/// `gen_dir` へ書き出す。印（`.fandhe-gen`）のある既存ディレクトリの、今回出す側（`web/` か、それ以外）だけを消して作り直す
/// （選ばなくなった機能の拡張ディレクトリが残って成果物に混ざるのを防ぐ）。
/// symlink と、印の無い既存ディレクトリ（利用者のもの）は拒否する。
pub fn write(gen_dir: &Path, files: &BTreeMap<String, String>) -> Result<(), String> {
    match fs::symlink_metadata(gen_dir) {
        Ok(md) if md.file_type().is_symlink() => {
            return Err(format!("{} は symlink です。書き出しません", gen_dir.display()))
        }
        Ok(_) if !has_marker(gen_dir) => {
            return Err(format!(
                "{} は既に存在し、`{MARKER}` がありません。cargo fandhe gen が作ったものではないため上書きしません",
                gen_dir.display()
            ))
        }
        Ok(_) => {
            // Web の生成（`web/` 配下を出す）とネイティブの生成は互いの成果物を消さない:
            // 今回出す側（web か、web 以外）の直下だけを消す。
            let web_run = files.keys().any(|k| k.starts_with("web/"));
            let rd = fs::read_dir(gen_dir).map_err(|e| format!("{} を読めません: {e}", gen_dir.display()))?;
            for ent in rd {
                let ent = ent.map_err(|e| format!("{} を列挙できません: {e}", gen_dir.display()))?;
                let name = ent.file_name();
                if name == MARKER || (name == "web") != web_run {
                    continue;
                }
                let p = ent.path();
                let md = fs::symlink_metadata(&p).map_err(|e| format!("{} を調べられません: {e}", p.display()))?;
                let r = if md.is_dir() { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
                r.map_err(|e| format!("{} を消せません: {e}", p.display()))?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{} を調べられません: {e}", gen_dir.display())),
    }
    // 印を最初に書く。途中で失敗しても印付きで残り、次回の gen が作り直せる。
    let mut entries: Vec<_> = files.iter().collect();
    entries.sort_by_key(|(rel, _)| rel.as_str() != MARKER);
    for (rel, body) in entries {
        if rel.split('/').any(|s| s.is_empty() || s == "..") {
            return Err(format!("不正な出力パス `{rel}`"));
        }
        let path = rel.split('/').fold(gen_dir.to_path_buf(), |p, s| p.join(s));
        let dir = path.parent().unwrap_or(gen_dir);
        let res = fs::create_dir_all(dir)
            .map_err(|e| format!("{} を作れません: {e}", dir.display()))
            .and_then(|()| {
                fs::write(&path, body).map_err(|e| format!("{} を書けません: {e}", path.display()))
            });
        if let Err(e) = res {
            // ここへ来る gen/ は新規か印付きを消した後の自前分。印が無いまま残ると次回が利用者のものとして拒否するため片付ける。
            if !has_marker(gen_dir) {
                let _ = fs::remove_dir_all(gen_dir);
            }
            return Err(e);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declaration::parse;
    use serde_json::json;

    fn found(v: Value) -> Found {
        Found {
            pkg_id: "x".into(),
            dir: PathBuf::from("/c/x"),
            decl: parse(v).unwrap(),
        }
    }
    fn cfg(v: Value) -> AppConfig {
        AppConfig::parse(&v).unwrap()
    }
    /// bundle id だけを持つ最小の設定に、足りない欄を足して使う。
    const BID: &str = "com.example.app";
    fn app() -> AppInfo {
        AppInfo {
            name: "my-app".into(),
            version: "1.2.3".into(),
            cdylib: Some("my_app".into()),
            bins: vec!["my-app-bin".into()],
            gen_dir: PathBuf::from("/a/gen"),
        }
    }
    fn err(r: Result<BTreeMap<String, String>, Vec<String>>) -> String {
        r.expect_err("拒否されるはず").join("\n")
    }

    #[test]
    fn missing_usage_description_names_os_feature_and_key() {
        let f = found(
            json!({"id": "cam", "ios": {"usage-descriptions": ["NSCameraUsageDescription"]}}),
        );
        let e = err(render(
            &[],
            &[&f],
            &[],
            &cfg(json!({"ios": {"bundle-id": BID}})),
            &app(),
        ));
        assert!(
            e.contains("ios") && e.contains("cam") && e.contains("NSCameraUsageDescription"),
            "{e}"
        );
    }

    #[test]
    fn missing_description_in_extension_is_an_error() {
        let ext = json!({"name": "W", "bundle-suffix": "w", "deployment-target": "14.0", "sources": ["a"],
            "usage-descriptions": ["NSCameraUsageDescription"], "info-plist": {"NSExtension": {}}});
        let f = found(json!({"id": "w", "macos": {"extensions": [ext]}}));
        let e = err(render(&[], &[], &[&f], &cfg(json!(null)), &app()));
        assert!(
            e.contains("macos") && e.contains("NSCameraUsageDescription"),
            "{e}"
        );
    }

    #[test]
    fn app_config_rejects_bad_input() {
        for v in [
            json!({"ios": {"usage-descriptions": {"NSCameraUsageDescription": "  "}}}),
            json!({"ios": {"usage-descriptions": {"NSCameraUsageDescription": "a\nb"}}}),
            json!({"ios": {"usage-descriptions": {"Camera": "a"}}}),
            json!({"app-group-prefix": "com"}),
            json!({"app-group-prefix": "com.x y"}),
            json!({"ios": {"bundle-id": "com"}}),
            json!({"ios": {"bundle-id": "com.a b"}}),
            json!({"macos": {"bundle-id": "com.example.app"}}),
            json!({"unknown": 1}),
        ] {
            assert!(AppConfig::parse(&v).is_err(), "{v}");
        }
    }

    #[test]
    fn app_groups_need_prefix_and_expand_per_os() {
        let f = found(
            json!({"id": "g", "ios": {"app-groups": ["widget"]}, "macos": {"app-groups": ["widget"]}}),
        );
        let ios = json!({"bundle-id": BID});
        let e = err(render(&[], &[&f], &[], &cfg(json!({"ios": ios})), &app()));
        assert!(e.contains("app-group-prefix"), "{e}");
        let c = cfg(json!({"app-group-prefix": "com.fandhe", "ios": ios}));
        let files = render(&[], &[&f], &[&f], &c, &app()).unwrap();
        assert!(files["ios/App.entitlements"].contains("<string>group.com.fandhe.widget</string>"));
        assert!(files["macos/App.entitlements"]
            .contains("<string>$(TeamIdentifierPrefix)com.fandhe.widget</string>"));
    }

    #[test]
    fn values_are_escaped_for_xml_plist_and_groovy() {
        let f = found(json!({"id": "e",
            "ios": {"usage-descriptions": ["NSCameraUsageDescription"]},
            "android": {"components": [{"kind": "service", "name": "a.B", "exported": false,
                "attributes": {"label": "x\"<&>y"}}], "gradle": ["g:a:1.0"]}}));
        let c = cfg(json!({"ios": {"bundle-id": BID,
            "usage-descriptions": {"NSCameraUsageDescription": "<b>&'\""}}}));
        let files = render(&[&f], &[&f], &[], &c, &app()).unwrap();
        assert!(files["ios/Info.plist"].contains("&lt;b&gt;&amp;&apos;&quot;"));
        assert!(files["android/AndroidManifest.xml"]
            .contains("android:label=\"x&quot;&lt;&amp;&gt;y\""));
        assert_eq!(groovy_str("a'b\\c"), "'a\\'b\\\\c'");
    }

    #[test]
    fn other_os_field_conflict_does_not_fail_target_os() {
        let a = found(
            json!({"id": "a", "android": {"permissions": [{"name": "p"}]}, "ios": {"info-plist": {"K": "1"}}}),
        );
        let b = found(
            json!({"id": "b", "android": {"permissions": [{"name": "q"}]}, "ios": {"info-plist": {"K": "2"}}}),
        );
        let files = render(&[&a, &b], &[], &[], &ios_cfg(), &app()).unwrap();
        assert!(files.contains_key("android/AndroidManifest.xml"));
    }

    #[test]
    fn plist_rejects_null() {
        let f = found(json!({"id": "n", "ios": {"info-plist": {"K": null}}}));
        let e = err(render(
            &[],
            &[&f],
            &[],
            &cfg(json!({"ios": {"bundle-id": BID}})),
            &app(),
        ));
        assert!(e.contains("null"), "{e}");
    }

    fn ios_cfg() -> AppConfig {
        cfg(json!({"ios": {"bundle-id": BID}}))
    }

    #[test]
    fn shell_values_are_emitted_even_without_features() {
        let files = render(&[], &[], &[], &ios_cfg(), &app()).unwrap();
        let m = &files["android/AndroidManifest.xml"];
        assert!(m.contains("android:name=\"com.fandhe.platform.MainActivity\""));
        assert!(m.contains("android:exported=\"true\""));
        assert!(m.contains("android:launchMode=\"singleTask\""));
        assert!(m.contains("android:name=\"android.app.lib_name\" android:value=\"my_app\""));
        assert!(m.contains("android.intent.category.LAUNCHER"));
        let g = &files["android/fandhe.gradle"];
        assert!(g.contains("java.srcDir '/a/gen/android/java'"), "{g}");
        assert!(
            g.contains("addStaticManifestFile('/a/gen/android/AndroidManifest.xml')"),
            "{g}"
        );
        assert!(files[LIBRARY_CLASS_PATH].contains("NAME = \"my_app\""));
        let p = &files["ios/Info.plist"];
        assert!(!p.contains("$("), "{p}");
        for k in [
            "<string>my-app-bin</string>",
            "<string>com.example.app</string>",
            "<string>1.2.3</string>",
            "<string>17.0</string>",
            "<key>UILaunchScreen</key>",
            "<integer>2</integer>",
        ] {
            assert!(p.contains(k), "{k}");
        }
        assert!(!p.contains("CFBundleSupportedPlatforms"));
    }

    #[test]
    fn shell_inputs_are_validated() {
        let ok = ios_cfg();
        let with = |f: &dyn Fn(&mut AppInfo)| {
            let mut a = app();
            f(&mut a);
            err(render(&[], &[], &[], &ok, &a))
        };
        assert!(with(&|a| a.cdylib = None).contains("cdylib"));
        assert!(with(&|a| a.bins.clear()).contains("bin target"));
        assert!(with(&|a| a.bins.push("b".into())).contains("bin target"));
        assert!(with(&|a| a.version = "1.0.0-rc.1".into()).contains("pre-release"));
        let e = err(render(&[], &[], &[], &cfg(json!(null)), &app()));
        assert!(e.contains("bundle-id"), "{e}");
    }

    #[test]
    fn write_refuses_unmarked_dir_and_replaces_marked_dir() {
        let root = std::env::temp_dir().join(format!("fandhe-native-write-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("gen")).unwrap();
        fs::write(root.join("gen/mine.txt"), "x").unwrap();
        let files = BTreeMap::from([
            (MARKER.to_string(), "m".to_string()),
            ("a/b.txt".into(), "1".into()),
        ]);
        assert!(write(&root.join("gen"), &files).is_err());
        assert!(root.join("gen/mine.txt").exists());
        fs::remove_dir_all(root.join("gen")).unwrap();
        write(&root.join("gen"), &files).unwrap();
        fs::write(root.join("gen/stale.txt"), "x").unwrap();
        write(&root.join("gen"), &files).unwrap();
        assert!(!root.join("gen/stale.txt").exists() && root.join("gen/a/b.txt").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn write_refuses_symlinked_marker() {
        let root =
            std::env::temp_dir().join(format!("fandhe-native-marker-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("gen")).unwrap();
        fs::write(root.join("target.txt"), "keep").unwrap();
        std::os::unix::fs::symlink(root.join("target.txt"), root.join("gen").join(MARKER)).unwrap();
        let files = BTreeMap::from([(MARKER.to_string(), "m".to_string())]);
        assert!(write(&root.join("gen"), &files).is_err());
        assert_eq!(fs::read_to_string(root.join("target.txt")).unwrap(), "keep");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn web_and_native_writes_keep_each_other() {
        let root = std::env::temp_dir().join(format!("fandhe-native-web-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let gen = root.join("gen");
        let m = (MARKER.to_string(), "m".to_string());
        let native = BTreeMap::from([m.clone(), ("android/a.txt".into(), "1".into())]);
        let web = BTreeMap::from([m, ("web/manifest.webmanifest".into(), "{}".into())]);
        // native -> web
        write(&gen, &native).unwrap();
        write(&gen, &web).unwrap();
        assert!(
            gen.join("android/a.txt").exists() && gen.join("web/manifest.webmanifest").exists()
        );
        // web -> native（古いネイティブ成果物は消え、web は残る）
        fs::write(gen.join("stale.txt"), "x").unwrap();
        write(&gen, &native).unwrap();
        assert!(!gen.join("stale.txt").exists() && gen.join("web/manifest.webmanifest").exists());
        // web 単独が先でも後続のネイティブが拒否されない
        fs::remove_dir_all(&gen).unwrap();
        write(&gen, &web).unwrap();
        write(&gen, &native).unwrap();
        assert!(
            gen.join("android/a.txt").exists() && gen.join("web/manifest.webmanifest").exists()
        );
        let _ = fs::remove_dir_all(&root);
    }
}
