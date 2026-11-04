//! 複数機能の宣言を 1 つへ統合する規則（`capability-declaration.md` §統合規則）。
//!
//! 機能を id 順に処理するので結果は決定的。入力は検証済みの宣言で、出力は
//! `cargo fandhe gen`（#1176）が Info.plist・Manifest・Gradle へ展開する元になる。
//! `java`・`res`・`sources` は統合せず crate ごとに集める（同名リソースの衝突は Gradle のマージに任せる）。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::declaration::{Component, Declaration, Extension, IntentFilter};

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

#[derive(Debug, Default, Clone, PartialEq)]
pub struct AndroidMerged {
    /// 権限名 → `max-sdk`（None は無指定）。
    pub permissions: BTreeMap<String, Option<u32>>,
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
    /// (機能 id, OS, 拡張名, crate からの相対パス)。
    pub extension_sources: Vec<(String, String, String, String)>,
}

/// 値の葉ごとに「最初に書いた機能」を記録する。深い位置の競合で正しい 2 機能を報告するため。
fn record(path: &str, v: &Value, owners: &mut BTreeMap<String, String>, id: &str) {
    // オブジェクト位置にも記録する（スカラーとの型不一致で先行機能を報告するため）。
    owners
        .entry(path.to_string())
        .or_insert_with(|| id.to_string());
    if let Value::Object(m) = v {
        m.iter()
            .for_each(|(k, c)| record(&format!("{path}.{k}"), c, owners, id));
    }
}

/// 同じキーの値を統合する。同値はそのまま、配列は和集合（出現順）、オブジェクトは再帰。
/// それ以外の不一致は競合した葉のパスを `Err` で返す。
fn merge_value(
    a: &Value,
    b: &Value,
    path: &str,
    owners: &mut BTreeMap<String, String>,
    id: &str,
) -> Result<Value, String> {
    match (a, b) {
        _ if a == b => Ok(a.clone()),
        (Value::Array(x), Value::Array(y)) => {
            let mut out = x.clone();
            for v in y {
                if !out.contains(v) {
                    out.push(v.clone());
                }
            }
            Ok(Value::Array(out))
        }
        (Value::Object(x), Value::Object(y)) => {
            let mut out = x.clone();
            for (k, v) in y {
                let child = format!("{path}.{k}");
                let merged = match out.get(k) {
                    Some(cur) => merge_value(cur, v, &child, owners, id)?,
                    None => {
                        record(&child, v, owners, id);
                        v.clone()
                    }
                };
                out.insert(k.clone(), merged);
            }
            Ok(Value::Object(out))
        }
        _ => Err(path.to_string()),
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
            Some(cur) => match merge_value(cur, v, k, owners, id) {
                Ok(m) => m,
                Err(leaf) => {
                    errs.push(MergeError {
                        field: format!("{field}.{leaf}"),
                        a: owners.get(&leaf).cloned().unwrap_or_default(),
                        b: id.to_string(),
                        detail: format!("{cur} と {v}"),
                    });
                    continue;
                }
            },
            None => {
                record(k, v, owners, id);
                v.clone()
            }
        };
        into.insert(k.clone(), merged);
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
                let key = format!("{os}.extensions.{}", e.name);
                match dst.extensions.get(&e.name) {
                    Some(cur) if cur != e => {
                        errs.push(MergeError {
                            field: key.clone(),
                            a: owners
                                .entry(key)
                                .or_default()
                                .get("")
                                .cloned()
                                .unwrap_or_default(),
                            b: id.to_string(),
                            detail: "同名の拡張で内容が違います".into(),
                        });
                    }
                    Some(_) => {}
                    None => {
                        owners
                            .entry(key)
                            .or_default()
                            .insert(String::new(), id.to_string());
                        dst.extensions.insert(e.name.clone(), e.clone());
                    }
                }
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
                // 片方が無指定なら無指定、両方指定なら大きい方。
                dst.permissions
                    .entry(p.name.clone())
                    .and_modify(|cur| *cur = cur.zip(p.max_sdk).map(|(x, y)| x.max(y)))
                    .or_insert(p.max_sdk);
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
                let key = format!("android.components.{}", k.name);
                match dst.components.get(&k.name) {
                    Some(cur) if cur != k => {
                        errs.push(MergeError {
                            field: key.clone(),
                            a: owners
                                .entry(key)
                                .or_default()
                                .get("")
                                .cloned()
                                .unwrap_or_default(),
                            b: id.to_string(),
                            detail: "同名のコンポーネントで内容が違います".into(),
                        });
                    }
                    Some(_) => {}
                    None => {
                        owners
                            .entry(key)
                            .or_default()
                            .insert(String::new(), id.to_string());
                        dst.components.insert(k.name.clone(), k.clone());
                    }
                }
            }
            dst.java
                .extend(a.java.iter().map(|p| (id.to_string(), p.clone())));
            dst.res
                .extend(a.res.iter().map(|p| (id.to_string(), p.clone())));
        }
        out.windows_identity |= d.windows.as_ref().is_some_and(|w| w.identity);
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}
