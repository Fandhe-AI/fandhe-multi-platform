//! `cargo fandhe gen`: アプリが選んだ機能の宣言だけからネイティブ設定を生成する（REQ-21 / TASK-21.3、Issue #1176）。
//!
//! 呼び出し元は `main.rs` の `gen` サブコマンド（と統合テスト）。入力は `cargo metadata` の解決結果で、
//! どの機能 crate を含めるかは `reach` が決め、宣言の統合は `merge` が行う。ここは統合結果を
//! 各 OS の文法（XML・plist・Groovy）へ直列化する層で、値はすべて出力先の文法でエスケープする
//! （宣言は第三者の crate からも来る。security.md A03）。
//!
//! 出力先は `<アプリの crate>/gen/` に固定する（consumer-app-structure.md §4、gitignore する）。
//! Info.plist の説明文（`*UsageDescription`）は既定値を持たず、アプリ側の
//! `[package.metadata.fandhe-app]` に無ければエラーにする（既定文言のまま審査に出る事故を防ぐ）。
//! 範囲外: Xcode のターゲット・framework のリンク（#1212）、Windows の identity（#1208）、
//! 既存設定からの切り替え（#1194）、run/build への組み込み（#1201）。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::merge::{merge, AndroidMerged, AppleMerged};
use crate::reach::reachable;
use crate::workspace::{collect, run_metadata, Found};

/// 生成物の印。これがある `gen/` だけを次回の生成で置き換える（利用者のディレクトリを消さない）。
const MARKER: &str = ".fandhe-gen";
const MAIN_ACTIVITY: &str = "com.fandhe.platform.MainActivity";
/// OS ごとに `cargo metadata --filter-platform` へ渡す triple（対象 OS に限った依存の cfg 評価は cargo に任せる）。
const ANDROID_TRIPLE: &str = "aarch64-linux-android";
const IOS_TRIPLE: &str = "aarch64-apple-ios";
const MACOS_TRIPLE: &str = "aarch64-apple-darwin";
const APP_GROUPS_KEY: &str = "com.apple.security.application-groups";

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
}

/// 本体と拡張の Info.plist が共有する説明文（キー → 文言）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct OsConfig {
    #[serde(default)]
    usage_descriptions: BTreeMap<String, String>,
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
            let ok = p.split('.').count() >= 2
                && p.split('.').all(|s| {
                    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                });
            if !ok {
                errs.push(format!("fandhe-app.app-group-prefix: `{p}` はドット区切り（2 セグメント以上）の英数字とハイフンにしてください"));
            }
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
fn android_manifest(m: &AndroidMerged) -> Result<String, String> {
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
    if !m.main_activity_intent_filters.is_empty() || !m.components.is_empty() {
        o.push_str("    <application>\n");
        if !m.main_activity_intent_filters.is_empty() {
            o.push_str(&format!(
                "        <activity{}>\n",
                xml_attr("name", MAIN_ACTIVITY)
            ));
            for f in &m.main_activity_intent_filters {
                filter_xml(f, "            ", &mut o);
            }
            o.push_str("        </activity>\n");
        }
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
fn android_gradle(m: &AndroidMerged, dirs: &BTreeMap<&str, &Path>) -> Option<String> {
    let src = |items: &[(String, String)]| -> Vec<String> {
        items
            .iter()
            .filter_map(|(id, rel)| {
                let p = dirs.get(id.as_str())?.join(rel);
                Some(groovy_str(&p.to_string_lossy().replace('\\', "/")))
            })
            .collect()
    };
    let (java, res) = (src(&m.java), src(&m.res));
    if java.is_empty() && res.is_empty() && m.gradle.is_empty() {
        return None;
    }
    let mut o = String::from("// cargo fandhe gen が生成。編集しない\n");
    if !java.is_empty() || !res.is_empty() {
        o.push_str("android {\n    sourceSets {\n        main {\n");
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
    }
    if !m.gradle.is_empty() {
        o.push_str("dependencies {\n");
        for g in &m.gradle {
            o.push_str(&format!("    implementation {}\n", groovy_str(g)));
        }
        o.push_str("}\n");
    }
    Some(o)
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
        keys: &BTreeSet<String>,
        info: &Map<String, Value>,
        errs: &mut Vec<String>,
    ) -> Map<String, Value> {
        let mut m = plist_template();
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

fn apple_files(
    ctx: &AppleCtx,
    m: &AppleMerged,
    files: &mut BTreeMap<String, String>,
    errs: &mut Vec<String>,
) {
    let os = ctx.os;
    let mut local = Vec::new();
    let plist = ctx.plist(&m.usage_descriptions, &m.info_plist, &mut local);
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
                d
            })
            .collect();
        let refs: Vec<_> = decls.iter().map(|d| (d.id.as_str(), d)).collect();
        merge(&refs)
            .map_err(|e| errs.extend(e.into_iter().map(|e| e.to_string())))
            .ok()
    };

    if let Some(m) = merged("android", android, &mut errs).map(|m| m.android) {
        if m != AndroidMerged::default() {
            let dirs: BTreeMap<&str, &Path> = android
                .iter()
                .map(|f| (f.decl.id.as_str(), f.dir.as_path()))
                .collect();
            match android_manifest(&m) {
                Ok(s) => {
                    files.insert("android/AndroidManifest.xml".to_string(), s);
                }
                Err(e) => errs.push(format!("android: {e}")),
            }
            if let Some(g) = android_gradle(&m, &dirs) {
                files.insert("android/fandhe.gradle".to_string(), g);
            }
        }
    }
    for (os, set) in [("ios", ios), ("macos", macos)] {
        let Some(m) = merged(os, set, &mut errs).map(|m| if os == "ios" { m.ios } else { m.macos })
        else {
            continue;
        };
        if m == AppleMerged::default() {
            continue;
        }
        let ctx = AppleCtx {
            os,
            cfg: if os == "ios" { &cfg.ios } else { &cfg.macos },
            prefix: cfg.app_group_prefix.as_deref(),
            found: set,
        };
        apple_files(&ctx, &m, &mut files, &mut errs);
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

/// `manifest`（省略時はカレントのパッケージ）のアプリが選んだ機能から、ネイティブ設定を生成する。
/// 宣言・入力に問題が 1 件でもあれば何も返さない（check と同じ厳しさ）。書き出しは [`write`]。
pub fn generate(manifest: Option<&Path>) -> Result<Generated, Vec<String>> {
    let mut sets: Vec<Vec<Found>> = Vec::new();
    let mut app: Option<(PathBuf, Value)> = None;
    for triple in [ANDROID_TRIPLE, IOS_TRIPLE, MACOS_TRIPLE] {
        let meta = run_metadata(manifest, Some(triple))?;
        if app.is_none() {
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
            app = Some((dir.to_path_buf(), pkg["metadata"]["fandhe-app"].clone()));
        }
        let reach = reachable(&meta);
        sets.push(
            collect(&meta)?
                .into_iter()
                .filter(|f| reach.contains(&f.pkg_id))
                .collect(),
        );
    }
    let (dir, cfg) = app.expect("ループで必ず設定される");
    let cfg = AppConfig::parse(&cfg)?;
    let r = |i: usize| sets[i].iter().collect::<Vec<_>>();
    let files = render(&r(0), &r(1), &r(2), &cfg)?;
    Ok(Generated {
        gen_dir: dir.join("gen"),
        files,
    })
}

/// `gen_dir` へ書き出す。印（`.fandhe-gen`）のある既存ディレクトリだけを消して作り直す
/// （選ばなくなった機能の拡張ディレクトリが残って成果物に混ざるのを防ぐ）。
/// symlink と、印の無い既存ディレクトリ（利用者のもの）は拒否する。
pub fn write(gen_dir: &Path, files: &BTreeMap<String, String>) -> Result<(), String> {
    match fs::symlink_metadata(gen_dir) {
        Ok(md) if md.file_type().is_symlink() => {
            return Err(format!("{} は symlink です。書き出しません", gen_dir.display()))
        }
        Ok(_) if !gen_dir.join(MARKER).is_file() => {
            return Err(format!(
                "{} は既に存在し、`{MARKER}` がありません。cargo fandhe gen が作ったものではないため上書きしません",
                gen_dir.display()
            ))
        }
        Ok(_) => fs::remove_dir_all(gen_dir).map_err(|e| format!("{} を消せません: {e}", gen_dir.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{} を調べられません: {e}", gen_dir.display())),
    }
    for (rel, body) in files {
        if rel.split('/').any(|s| s.is_empty() || s == "..") {
            return Err(format!("不正な出力パス `{rel}`"));
        }
        let path = rel.split('/').fold(gen_dir.to_path_buf(), |p, s| p.join(s));
        let dir = path.parent().unwrap_or(gen_dir);
        fs::create_dir_all(dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
        fs::write(&path, body).map_err(|e| format!("{} を書けません: {e}", path.display()))?;
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
    fn err(r: Result<BTreeMap<String, String>, Vec<String>>) -> String {
        r.expect_err("拒否されるはず").join("\n")
    }

    #[test]
    fn missing_usage_description_names_os_feature_and_key() {
        let f = found(
            json!({"id": "cam", "ios": {"usage-descriptions": ["NSCameraUsageDescription"]}}),
        );
        let e = err(render(&[], &[&f], &[], &cfg(json!(null))));
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
        let e = err(render(&[], &[], &[&f], &cfg(json!(null))));
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
        let e = err(render(&[], &[&f], &[], &cfg(json!(null))));
        assert!(e.contains("app-group-prefix"), "{e}");
        let c = cfg(json!({"app-group-prefix": "com.fandhe"}));
        let files = render(&[], &[&f], &[&f], &c).unwrap();
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
        let c =
            cfg(json!({"ios": {"usage-descriptions": {"NSCameraUsageDescription": "<b>&'\""}}}));
        let files = render(&[&f], &[&f], &[], &c).unwrap();
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
        let files = render(&[&a, &b], &[], &[], &cfg(json!({}))).unwrap();
        assert!(files.contains_key("android/AndroidManifest.xml"));
    }

    #[test]
    fn plist_rejects_null() {
        let f = found(json!({"id": "n", "ios": {"info-plist": {"K": null}}}));
        let e = err(render(&[], &[&f], &[], &cfg(json!(null))));
        assert!(e.contains("null"), "{e}");
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
}
