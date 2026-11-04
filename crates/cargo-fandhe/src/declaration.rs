//! 機能宣言の型と検証規則。
//!
//! 型は `deny_unknown_fields` で未知キーを拒否し（綴り間違い・未対応欄の黙殺を防ぐ）、
//! 意味の検査は [`validate`] が全問題を集めて返す。パスの実在確認は crate の位置が要るため
//! [`check_paths`] に分ける。呼び出し元は [`crate::check_workspace`] と、将来の
//! `cargo fandhe gen`（#1176）。

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Map, Value};

/// 1 機能の宣言（`[package.metadata.fandhe]`）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Declaration {
    /// 対応表の id。crate 名 `fandhe-cap-<id>` と一致させる。
    pub id: String,
    pub ios: Option<Apple>,
    pub macos: Option<Apple>,
    pub android: Option<Android>,
    pub windows: Option<Windows>,
}

/// iOS / macOS 共通の欄。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Apple {
    /// Info.plist の `*UsageDescription` のキーだけ。文言は利用者が書く。
    #[serde(default)]
    pub usage_descriptions: Vec<String>,
    #[serde(default)]
    pub frameworks: Vec<String>,
    #[serde(default)]
    pub entitlements: Map<String, Value>,
    /// App Group の記号名。生成時に OS ごとの正式 ID へ展開する。
    #[serde(default)]
    pub app_groups: Vec<String>,
    /// 説明文以外の任意の Info.plist キー。
    #[serde(default)]
    pub info_plist: Map<String, Value>,
    #[serde(default)]
    pub extensions: Vec<Extension>,
}

/// Apple の拡張ターゲット（Widget・Share 等）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Extension {
    pub name: String,
    pub bundle_suffix: String,
    pub deployment_target: String,
    /// crate からの相対パス。
    pub sources: Vec<String>,
    #[serde(default)]
    pub frameworks: Vec<String>,
    #[serde(default)]
    pub app_groups: Vec<String>,
    #[serde(default)]
    pub entitlements: Map<String, Value>,
    #[serde(default)]
    pub info_plist: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Android {
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub features: Vec<Feature>,
    /// `group:artifact:version`。
    #[serde(default)]
    pub gradle: Vec<String>,
    /// crate からの相対パス。
    #[serde(default)]
    pub java: Vec<String>,
    #[serde(default)]
    pub res: Vec<String>,
    #[serde(default)]
    pub main_activity_intent_filters: Vec<IntentFilter>,
    #[serde(default)]
    pub components: Vec<Component>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Permission {
    pub name: String,
    pub max_sdk: Option<u32>,
}

/// `required` は省略不可（暗黙の既定値による端末の除外を防ぐ）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Feature {
    pub name: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentKind {
    Activity,
    Service,
    Receiver,
    Provider,
}

/// `exported` は省略不可（意図しない公開を防ぐ）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Component {
    pub kind: ComponentKind,
    pub name: String,
    pub exported: bool,
    /// 他の `android:*` 属性（`android:` を除いた名前）。
    #[serde(default)]
    pub attributes: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub intent_filters: Vec<IntentFilter>,
    #[serde(default)]
    pub meta_data: Vec<MetaData>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct IntentFilter {
    pub actions: Vec<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub data: Vec<IntentData>,
}

/// `<data>` の属性。名前は Android の原語。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentData {
    pub scheme: Option<String>,
    pub host: Option<String>,
    pub mime_type: Option<String>,
    pub path_prefix: Option<String>,
}

/// `resource` と `value` はどちらか一方。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MetaData {
    pub name: String,
    pub resource: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Windows {
    /// パッケージ ID が要る機能なら true（生成は #1208）。
    pub identity: bool,
}

/// 検証で見つかった問題 1 件。`field` は欄のパス（例 `android.gradle[0]`）。
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub field: String,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// `package.metadata.fandhe` の JSON を型へ読む。未知キー・型違いは `Problem` になる。
pub fn parse(value: Value) -> Result<Declaration, Problem> {
    serde_json::from_value(value).map_err(|e| Problem {
        field: "(構造)".into(),
        message: e.to_string(),
    })
}

// ---- 判定関数 ----

fn is_kebab(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c.is_ascii_alphabetic())
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// ドット区切りの Java 識別子。先頭 `.` の相対名は空セグメントになり拒否される。
fn is_java_name(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|seg| {
            let mut cs = seg.chars();
            cs.next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && cs.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        })
}

fn is_rel_path(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('/')
        && !s.contains(['\\', ':'])
        && !s.chars().any(char::is_control)
        && s.split('/').all(|seg| seg != "..")
}

/// Info.plist のキーから端末修飾子（`key-<platform>~<device>` の `-…`・`~…`）を落とす。
/// `info-plist` の説明文拒否判定が使う。修飾子つきでも説明文を機能側に埋め込ませないため。
fn strip_qualifier(k: &str) -> &str {
    &k[..k.find(['-', '~']).unwrap_or(k.len())]
}

fn no_ctrl(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

fn json_no_ctrl(v: &Value) -> bool {
    match v {
        Value::String(s) => no_ctrl(s),
        Value::Array(a) => a.iter().all(json_no_ctrl),
        Value::Object(m) => m.iter().all(|(k, v)| no_ctrl(k) && json_no_ctrl(v)),
        _ => true,
    }
}

fn is_gradle(s: &str) -> bool {
    let parts: Vec<&str> = s.split(':').collect();
    let ok = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    // 版は数字始まり。`+`・`latest`・範囲・変数は文字集合で落ちる。
    parts.len() == 3
        && parts.iter().all(|p| ok(p))
        && parts[2].starts_with(|c: char| c.is_ascii_digit())
}

fn is_deployment_target(s: &str) -> bool {
    s.split_once('.').is_some_and(|(a, b)| {
        !a.is_empty()
            && !b.is_empty()
            && a.chars().all(|c| c.is_ascii_digit())
            && b.chars().all(|c| c.is_ascii_digit())
    })
}

// ---- 検証 ----

struct Ctx(Vec<Problem>);

impl Ctx {
    fn bad(&mut self, field: &str, message: impl Into<String>) {
        self.0.push(Problem {
            field: field.to_string(),
            message: message.into(),
        });
    }
    fn check(&mut self, ok: bool, field: &str, message: impl Into<String>) {
        if !ok {
            self.bad(field, message);
        }
    }
    fn dup<'a>(&mut self, field: &str, names: impl Iterator<Item = &'a str>) {
        let mut seen = BTreeSet::new();
        for n in names {
            if !seen.insert(n) {
                self.bad(field, format!("`{n}` が重複しています"));
            }
        }
    }
    fn paths(&mut self, field: &str, paths: &[String]) {
        for (i, p) in paths.iter().enumerate() {
            self.check(
                is_rel_path(p),
                &format!("{field}[{i}]"),
                format!("`{p}` は crate からの相対パスにしてください（空・絶対・`..`・`\\`・`:` は不可）"),
            );
        }
    }
    fn idents(&mut self, field: &str, names: &[String]) {
        for (i, n) in names.iter().enumerate() {
            self.check(
                is_ident(n),
                &format!("{field}[{i}]"),
                format!("`{n}` は英数字と `_` の識別子にしてください"),
            );
        }
    }
    fn app_groups(&mut self, field: &str, groups: &[String]) {
        for (i, g) in groups.iter().enumerate() {
            self.check(
                is_kebab(g),
                &format!("{field}[{i}]"),
                format!("`{g}` は記号名（小文字英数字とハイフン）にしてください。`group.`・`$`・`.` を含む正式 ID は生成時に展開します"),
            );
        }
    }
    fn plist(&mut self, field: &str, m: &Map<String, Value>) {
        for (k, v) in m {
            self.check(
                !strip_qualifier(k).ends_with("UsageDescription"),
                &format!("{field}.{k}"),
                "説明文は既定値を持たず利用者が書きます（`-iphoneos`・`~ipad` 等の修飾子つきも同じ）。`usage-descriptions` にキーだけを書いてください",
            );
            self.check(
                no_ctrl(k) && json_no_ctrl(v),
                &format!("{field}.{k}"),
                "制御文字は使えません",
            );
        }
    }
    fn entitlements(&mut self, field: &str, m: &Map<String, Value>) {
        for (k, v) in m {
            self.check(
                k != "com.apple.security.application-groups",
                &format!("{field}.{k}"),
                "App Group は `app-groups` に記号名で書いてください",
            );
            self.check(
                no_ctrl(k) && json_no_ctrl(v),
                &format!("{field}.{k}"),
                "制御文字は使えません",
            );
        }
    }
    fn intent_filters(&mut self, field: &str, fs: &[IntentFilter]) {
        for (i, f) in fs.iter().enumerate() {
            let p = format!("{field}[{i}]");
            self.check(
                !f.actions.is_empty(),
                &format!("{p}.actions"),
                "1 件以上が必要です",
            );
            for a in f.actions.iter().chain(&f.categories) {
                self.check(
                    is_java_name(a),
                    &p,
                    format!("`{a}` はドット区切りの識別子にしてください"),
                );
            }
            for d in &f.data {
                let vals = [&d.scheme, &d.host, &d.mime_type, &d.path_prefix];
                self.check(
                    vals.iter().all(|v| v.as_deref().is_none_or(no_ctrl)),
                    &format!("{p}.data"),
                    "制御文字は使えません",
                );
            }
        }
    }
}

/// 構造が型に合った宣言の意味を検査し、問題をすべて返す（空なら正常）。
pub fn validate(d: &Declaration) -> Vec<Problem> {
    let mut c = Ctx(Vec::new());
    c.check(
        is_kebab(&d.id),
        "id",
        format!(
            "`{}` は kebab-case（小文字英数字とハイフン）にしてください",
            d.id
        ),
    );
    for (os, a) in [("ios", &d.ios), ("macos", &d.macos)] {
        if let Some(a) = a {
            validate_apple(&mut c, os, a);
        }
    }
    if let Some(a) = &d.android {
        validate_android(&mut c, a);
    }
    c.0
}

fn validate_apple(c: &mut Ctx, os: &str, a: &Apple) {
    for (i, k) in a.usage_descriptions.iter().enumerate() {
        c.check(
            is_ident(k) && k.ends_with("UsageDescription"),
            &format!("{os}.usage-descriptions[{i}]"),
            format!("`{k}` は `*UsageDescription` のキーにしてください"),
        );
    }
    c.idents(&format!("{os}.frameworks"), &a.frameworks);
    c.app_groups(&format!("{os}.app-groups"), &a.app_groups);
    c.plist(&format!("{os}.info-plist"), &a.info_plist);
    c.entitlements(&format!("{os}.entitlements"), &a.entitlements);
    c.dup(
        &format!("{os}.extensions"),
        a.extensions.iter().map(|e| e.name.as_str()),
    );
    for (i, e) in a.extensions.iter().enumerate() {
        let p = format!("{os}.extensions[{i}]");
        c.check(
            is_ident(&e.name),
            &format!("{p}.name"),
            format!("`{}` は識別子にしてください", e.name),
        );
        c.check(
            is_kebab(&e.bundle_suffix),
            &format!("{p}.bundle-suffix"),
            "小文字英数字とハイフンにしてください",
        );
        c.check(
            is_deployment_target(&e.deployment_target),
            &format!("{p}.deployment-target"),
            "`14.0` のように 数字.数字 にしてください",
        );
        c.check(
            !e.sources.is_empty(),
            &format!("{p}.sources"),
            "1 件以上が必要です",
        );
        c.paths(&format!("{p}.sources"), &e.sources);
        c.idents(&format!("{p}.frameworks"), &e.frameworks);
        c.app_groups(&format!("{p}.app-groups"), &e.app_groups);
        c.plist(&format!("{p}.info-plist"), &e.info_plist);
        c.entitlements(&format!("{p}.entitlements"), &e.entitlements);
        c.check(
            e.info_plist.contains_key("NSExtension"),
            &format!("{p}.info-plist"),
            "拡張ターゲットには `NSExtension` が必要です",
        );
    }
}

fn validate_android(c: &mut Ctx, a: &Android) {
    c.dup(
        "android.permissions",
        a.permissions.iter().map(|p| p.name.as_str()),
    );
    for (i, p) in a.permissions.iter().enumerate() {
        c.check(
            is_java_name(&p.name),
            &format!("android.permissions[{i}].name"),
            format!("`{}` はドット区切りの識別子にしてください", p.name),
        );
    }
    c.dup(
        "android.features",
        a.features.iter().map(|f| f.name.as_str()),
    );
    for (i, f) in a.features.iter().enumerate() {
        c.check(
            is_java_name(&f.name),
            &format!("android.features[{i}].name"),
            format!("`{}` はドット区切りの識別子にしてください", f.name),
        );
    }
    for (i, g) in a.gradle.iter().enumerate() {
        c.check(
            is_gradle(g),
            &format!("android.gradle[{i}]"),
            format!("`{g}` は `group:artifact:version` の固定版（数字始まり。`+`・`latest`・範囲・変数は不可）にしてください"),
        );
    }
    c.paths("android.java", &a.java);
    c.paths("android.res", &a.res);
    c.intent_filters(
        "android.main-activity-intent-filters",
        &a.main_activity_intent_filters,
    );
    c.dup(
        "android.components",
        a.components.iter().map(|k| k.name.as_str()),
    );
    for (i, k) in a.components.iter().enumerate() {
        let p = format!("android.components[{i}]");
        c.check(
            is_java_name(&k.name) && k.name.contains('.'),
            &format!("{p}.name"),
            format!("`{}` は完全修飾名にしてください（先頭 `.` の相対名・ドットなしの単一名は利用者の applicationId で解決されて壊れます）", k.name),
        );
        for (key, v) in &k.attributes {
            c.check(
                !key.is_empty()
                    && key.chars().all(|ch| ch.is_ascii_alphabetic())
                    && key != "name"
                    && key != "exported",
                &format!("{p}.attributes.{key}"),
                "属性名は英字のみで、`name`・`exported` は専用の欄に書いてください",
            );
            c.check(
                no_ctrl(v),
                &format!("{p}.attributes.{key}"),
                "制御文字は使えません",
            );
        }
        c.intent_filters(&format!("{p}.intent-filters"), &k.intent_filters);
        for (j, m) in k.meta_data.iter().enumerate() {
            let mp = format!("{p}.meta-data[{j}]");
            c.check(
                is_java_name(&m.name),
                &format!("{mp}.name"),
                format!("`{}` はドット区切りの識別子にしてください", m.name),
            );
            match (&m.resource, &m.value) {
                (Some(r), None) => c.check(
                    r.starts_with('@') && no_ctrl(r),
                    &mp,
                    "resource は `@xml/...` の形にしてください",
                ),
                (None, Some(v)) => c.check(no_ctrl(v), &mp, "制御文字は使えません"),
                _ => c.bad(
                    &mp,
                    "`resource` と `value` はどちらか一方だけを書いてください",
                ),
            }
        }
    }
}

/// `java`・`res`・`sources` が crate 内に実在するか確かめる（symlink による crate 外への脱出も拒否）。
/// 構造の検証（[`validate`]）と分けるのは、fixture のように crate の位置を持たない入力があるため。
pub fn check_paths(d: &Declaration, crate_dir: &Path) -> Vec<Problem> {
    let mut c = Ctx(Vec::new());
    let root = match crate_dir.canonicalize() {
        Ok(r) => r,
        Err(e) => {
            c.bad(
                "(crate)",
                format!("{} を解決できません: {e}", crate_dir.display()),
            );
            return c.0;
        }
    };
    let mut probe = |field: String, rel: &str| match root.join(rel).canonicalize() {
        Ok(p) if p.starts_with(&root) => {}
        Ok(_) => c.bad(&field, format!("`{rel}` が crate の外を指しています")),
        Err(_) => c.bad(&field, format!("`{rel}` が存在しません")),
    };
    if let Some(a) = &d.android {
        for (i, p) in a.java.iter().enumerate() {
            probe(format!("android.java[{i}]"), p);
        }
        for (i, p) in a.res.iter().enumerate() {
            probe(format!("android.res[{i}]"), p);
        }
    }
    for (os, a) in [("ios", &d.ios), ("macos", &d.macos)] {
        for (i, e) in a.iter().flat_map(|a| &a.extensions).enumerate() {
            for (j, p) in e.sources.iter().enumerate() {
                probe(format!("{os}.extensions[{i}].sources[{j}]"), p);
            }
        }
    }
    c.0
}
