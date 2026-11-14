//! 複数機能の宣言を 1 つへ統合する規則（`capability-declaration.md` §統合規則）。
//!
//! 機能を id 順に処理するので結果は決定的。入力は検証済みの宣言で、出力は
//! `cargo fandhe gen`（#1176）が Info.plist・Manifest・Gradle へ展開する元になる。
//! `java`・`res`・`sources` は統合せず crate ごとに集める（同名リソースの衝突は Gradle のマージに任せる）。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::declaration::{
    Component, Declaration, Extension, IntentFilter, MetaData, PermissionFlag,
};

/// 統合できない不一致。どの欄で、どの 2 機能が食い違ったかを含む。
#[derive(Debug, thiserror::Error, PartialEq)]
#[error("{field}: 機能 `{a}` と `{b}` の宣言が食い違います（{detail}）")]
pub struct MergeError {
    pub field: String,
    pub a: String,
    pub b: String,
    pub detail: String,
}

/// OS ごとの Apple 欄の統合結果。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct AppleMerged {
    pub usage_descriptions: BTreeSet<String>,
    pub frameworks: BTreeSet<String>,
    pub app_groups: BTreeSet<String>,
    pub entitlements: Map<String, Value>,
    pub info_plist: Map<String, Value>,
    pub extensions: BTreeMap<String, Extension>,
}

/// 統合した権限 1 件。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PermissionMerged {
    /// `max-sdk`（None は無指定）。
    pub max_sdk: Option<u32>,
    /// `usesPermissionFlags`。機能間で食い違えばエラーなので、統合結果は全機能で同じ集合。
    pub flags: BTreeSet<PermissionFlag>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct AndroidMerged {
    /// 権限名 → 統合結果。
    pub permissions: BTreeMap<String, PermissionMerged>,
    /// MainActivity の追加属性（キー → 値）。
    pub main_activity_attributes: BTreeMap<String, String>,
    /// `<application>` 直下の meta-data（`name` → 宣言）。
    pub application_meta_data: BTreeMap<String, MetaData>,
    /// `<queries>` の `<intent>` の action（重複は 1 件に統合）。
    pub queries: BTreeSet<String>,
    /// feature 名 → `required`。
    pub features: BTreeMap<String, bool>,
    pub gradle: BTreeSet<String>,
    pub main_activity_intent_filters: Vec<IntentFilter>,
    pub components: BTreeMap<String, Component>,
    /// (機能 id, crate からの相対パス)。
    pub java: Vec<(String, String)>,
    pub res: Vec<(String, String)>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Merged {
    pub ios: AppleMerged,
    pub macos: AppleMerged,
    pub android: AndroidMerged,
    pub windows_identity: bool,
    /// Web manifest のメンバー（`web.manifest`）。
    pub web_manifest: Map<String, Value>,
    /// (機能 id, OS, 拡張名, crate からの相対パス)。
    pub extension_sources: Vec<(String, String, String, String)>,
}

/// 所有者表と `merge_value` の path の内部区切り。キーに `.` を含み得るので（`K.X` と `K` → `X` を区別する）、
/// validate が制御文字を拒否済みの U+001F を使う。`MergeError.field` へ出すときだけ `.` へ置換する。
const SEP: char = '\u{1f}';

/// 値の葉ごとに「最初に書いた機能」を記録する。深い位置の競合で正しい 2 機能を報告するため。
fn record(path: &str, v: &Value, owners: &mut BTreeMap<String, String>, id: &str) {
    // オブジェクト位置にも記録する（スカラーとの型不一致で先行機能を報告するため）。
    owners
        .entry(path.to_string())
        .or_insert_with(|| id.to_string());
    if let Value::Object(m) = v {
        m.iter()
            .for_each(|(k, c)| record(&format!("{path}{SEP}{k}"), c, owners, id));
    }
}

/// 同じキーの値を統合する。同値はそのまま、配列は和集合（出現順）、オブジェクトは再帰。
/// それ以外の不一致は競合した葉のパスと両側の値を `errs` へ積み、先に書いた側の値を残す。
/// 競合があっても残りのキーは統合し続ける（呼び出し側が `errs` を見て `Err` にするので、残した値は外へ出ない）。
fn merge_value(
    a: &Value,
    b: &Value,
    path: &str,
    owners: &mut BTreeMap<String, String>,
    id: &str,
    errs: &mut Vec<(String, String)>,
) -> Value {
    match (a, b) {
        _ if a == b => a.clone(),
        (Value::Array(x), Value::Array(y)) => {
            let mut out = x.clone();
            for v in y {
                if !out.contains(v) {
                    out.push(v.clone());
                }
            }
            Value::Array(out)
        }
        (Value::Object(x), Value::Object(y)) => {
            let mut out = x.clone();
            for (k, v) in y {
                let child = format!("{path}{SEP}{k}");
                let merged = match out.get(k) {
                    Some(cur) => merge_value(cur, v, &child, owners, id, errs),
                    None => {
                        record(&child, v, owners, id);
                        v.clone()
                    }
                };
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        _ => {
            errs.push((path.to_string(), format!("{a} と {b}")));
            a.clone()
        }
    }
}

fn merge_map(
    into: &mut Map<String, Value>,
    from: &Map<String, Value>,
    field: &str,
    owners: &mut BTreeMap<String, String>,
    id: &str,
    errs: &mut Vec<MergeError>,
) {
    for (k, v) in from {
        let merged = match into.get(k) {
            Some(cur) => {
                let mut leaves = Vec::new();
                let m = merge_value(cur, v, k, owners, id, &mut leaves);
                errs.extend(leaves.into_iter().map(|(leaf, detail)| MergeError {
                    field: format!("{field}.{}", leaf.replace(SEP, ".")),
                    a: owners.get(&leaf).cloned().unwrap_or_default(),
                    b: id.to_string(),
                    detail,
                }));
                m
            }
            None => {
                record(k, v, owners, id);
                v.clone()
            }
        };
        into.insert(k.clone(), merged);
    }
}

/// 名前で統合する共通処理。同内容は 1 つ、違えば最初に書いた機能との競合を積む。
// 呼び出し 4 か所が同じ owners・errs を共有するため、構造体にまとめず引数で渡す。
#[allow(clippy::too_many_arguments)]
fn merge_named<T: PartialEq + Clone>(
    into: &mut BTreeMap<String, T>,
    name: &str,
    item: &T,
    field: String,
    detail: &str,
    owners: &mut BTreeMap<String, BTreeMap<String, String>>,
    id: &str,
    errs: &mut Vec<MergeError>,
) {
    match into.get(name) {
        Some(cur) if cur != item => errs.push(MergeError {
            a: owners
                .entry(field.clone())
                .or_default()
                .get("")
                .cloned()
                .unwrap_or_default(),
            field,
            b: id.to_string(),
            detail: detail.into(),
        }),
        Some(_) => {}
        None => {
            owners
                .entry(field)
                .or_default()
                .insert(String::new(), id.to_string());
            into.insert(name.to_string(), item.clone());
        }
    }
}

/// 宣言を統合する。`decls` は `(id, 宣言)`。id 順に並べ替えて処理する。
pub fn merge(decls: &[(&str, &Declaration)]) -> Result<Merged, Vec<MergeError>> {
    let mut sorted: Vec<_> = decls.to_vec();
    sorted.sort_by_key(|(id, _)| *id);
    let mut out = Merged::default();
    // 競合は最初の 1 件で止めず全件集める（capability-declaration.md §3）。
    let mut errs: Vec<MergeError> = Vec::new();
    // 欄ごとの「最初にそのキーを書いた機能」。エラーに 2 機能を出すために持つ。
    let mut owners: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();

    for (id, d) in sorted {
        for (os, src, dst) in [
            ("ios", &d.ios, &mut out.ios),
            ("macos", &d.macos, &mut out.macos),
        ] {
            let Some(a) = src else { continue };
            dst.usage_descriptions
                .extend(a.usage_descriptions.iter().cloned());
            dst.frameworks.extend(a.frameworks.iter().cloned());
            dst.app_groups.extend(a.app_groups.iter().cloned());
            for (kind, from, into) in [
                ("entitlements", &a.entitlements, &mut dst.entitlements),
                ("info-plist", &a.info_plist, &mut dst.info_plist),
            ] {
                let field = format!("{os}.{kind}");
                merge_map(
                    into,
                    from,
                    &field,
                    owners.entry(field.clone()).or_default(),
                    id,
                    &mut errs,
                );
            }
            for e in &a.extensions {
                merge_named(
                    &mut dst.extensions,
                    &e.name,
                    e,
                    format!("{os}.extensions.{}", e.name),
                    "同名の拡張で内容が違います",
                    &mut owners,
                    id,
                    &mut errs,
                );
                for s in &e.sources {
                    out.extension_sources.push((
                        id.to_string(),
                        os.to_string(),
                        e.name.clone(),
                        s.clone(),
                    ));
                }
            }
        }
        if let Some(a) = &d.android {
            let dst = &mut out.android;
            for p in &a.permissions {
                let key = format!("android.permissions.{}", p.name);
                match dst.permissions.get_mut(&p.name) {
                    Some(cur) => {
                        if cur.flags != p.flags {
                            errs.push(MergeError {
                                a: owners
                                    .entry(key.clone())
                                    .or_default()
                                    .get("")
                                    .cloned()
                                    .unwrap_or_default(),
                                field: key,
                                b: id.to_string(),
                                detail: format!(
                                    "同じ権限で flags が違います: {:?} と {:?}",
                                    cur.flags, p.flags
                                ),
                            });
                        }
                        // 片方が無指定なら無指定、両方指定なら大きい方。
                        cur.max_sdk = cur.max_sdk.zip(p.max_sdk).map(|(x, y)| x.max(y));
                    }
                    None => {
                        owners
                            .entry(key)
                            .or_default()
                            .insert(String::new(), id.to_string());
                        dst.permissions.insert(
                            p.name.clone(),
                            PermissionMerged {
                                max_sdk: p.max_sdk,
                                flags: p.flags.clone(),
                            },
                        );
                    }
                }
            }
            for f in &a.features {
                dst.features
                    .entry(f.name.clone())
                    .and_modify(|r| *r |= f.required)
                    .or_insert(f.required);
            }
            dst.gradle.extend(a.gradle.iter().cloned());
            for f in &a.main_activity_intent_filters {
                if !dst.main_activity_intent_filters.contains(f) {
                    dst.main_activity_intent_filters.push(f.clone());
                }
            }
            for k in &a.components {
                merge_named(
                    &mut dst.components,
                    &k.name,
                    k,
                    format!("android.components.{}", k.name),
                    "同名のコンポーネントで内容が違います",
                    &mut owners,
                    id,
                    &mut errs,
                );
            }
            for m in &a.application_meta_data {
                merge_named(
                    &mut dst.application_meta_data,
                    &m.name,
                    m,
                    format!("android.application-meta-data.{}", m.name),
                    "同名の meta-data で内容が違います",
                    &mut owners,
                    id,
                    &mut errs,
                );
            }
            for (k, v) in &a.main_activity_attributes {
                merge_named(
                    &mut dst.main_activity_attributes,
                    k,
                    v,
                    format!("android.main-activity-attributes.{k}"),
                    "同じ属性で値が違います",
                    &mut owners,
                    id,
                    &mut errs,
                );
            }
            for q in &a.queries {
                dst.queries.extend(q.actions.iter().cloned());
            }
            dst.java
                .extend(a.java.iter().map(|p| (id.to_string(), p.clone())));
            dst.res
                .extend(a.res.iter().map(|p| (id.to_string(), p.clone())));
        }
        if let Some(w) = &d.web {
            merge_map(
                &mut out.web_manifest,
                &w.manifest,
                "web.manifest",
                owners.entry("web.manifest".into()).or_default(),
                id,
                &mut errs,
            );
        }
        out.windows_identity |= d.windows.as_ref().is_some_and(|w| w.identity);
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}
