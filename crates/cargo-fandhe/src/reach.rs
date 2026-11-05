//! アプリが依存として解決した package の到達判定（capability-declaration.md §5）。
//!
//! `cargo fandhe gen` が、どの機能 crate の宣言を生成に入れるかを決めるために呼ぶ。
//! `resolve.nodes[].features` は workspace 全体で統合された値で、他の member が有効にした
//! optional 依存が混ざる。そのため辺と feature は、アプリの `default` から自前で活性化して辿る。
//! 入力は `cargo metadata --filter-platform <triple>` の JSON（対象 OS に限った依存の cfg 評価は cargo に任せる）。
//! 辿る辺は通常依存（dev・build は除く）。package は取得元を含む ID で区別する。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// 依存項目 1 件（`packages[].dependencies[]`）。
struct Entry {
    /// Cargo.toml 上の名前（rename があればそれ）。feature 記法の `X` と照合する。
    alias: String,
    optional: bool,
    features: Vec<String>,
    default: bool,
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(String::from))
        .collect()
}

/// アプリ（`resolve.root`）から到達する package ID の集合を返す。root が無ければ空。
pub(crate) fn reachable(meta: &Value) -> BTreeSet<String> {
    let Some(root) = meta["resolve"]["root"].as_str() else {
        return BTreeSet::new();
    };
    let pkgs: BTreeMap<&str, &Value> = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p["id"].as_str()?, p)))
        .collect();
    let nodes: BTreeMap<&str, &Value> = meta["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| Some((n["id"].as_str()?, n)))
        .collect();

    // 通常依存の項目だけを取り、(項目, 解決先 package ID) の組にする。
    // resolve の辺は dep_kinds に (通常, 項目の target) がある項目だけを採る（他 OS 向けの項目を混ぜない）。
    let mut entries: BTreeMap<String, Vec<(Entry, String)>> = BTreeMap::new();
    for (id, node) in &nodes {
        let Some(pkg) = pkgs.get(id) else { continue };
        let mut list = Vec::new();
        for d in pkg["dependencies"].as_array().into_iter().flatten() {
            if !d["kind"].is_null() {
                continue;
            }
            let alias = d["rename"]
                .as_str()
                .or(d["name"].as_str())
                .unwrap_or("")
                .to_string();
            let target = d["target"].as_str();
            let lib = alias.replace('-', "_");
            let hit = node["deps"].as_array().into_iter().flatten().find(|e| {
                e["name"].as_str() == Some(&lib)
                    && e["dep_kinds"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|k| k["kind"].is_null() && k["target"].as_str() == target)
            });
            if let Some(pid) = hit.and_then(|e| e["pkg"].as_str()) {
                list.push((
                    Entry {
                        alias,
                        optional: d["optional"].as_bool().unwrap_or(false),
                        features: strs(&d["features"]),
                        default: d["uses_default_features"].as_bool().unwrap_or(true),
                    },
                    pid.to_string(),
                ));
            }
        }
        entries.insert(id.to_string(), list);
    }

    let mut feats: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // 有効になった optional 依存項目（package ID, 項目の添字）。
    let mut on: BTreeSet<(String, usize)> = BTreeSet::new();
    feats
        .entry(root.to_string())
        .or_default()
        .insert("default".into());

    // 不動点まで広げる。弱依存 `X?/f` は X が別経路で有効になるのを待つので 1 回では終わらない。
    loop {
        let size = |f: &BTreeMap<String, BTreeSet<String>>, o: &BTreeSet<_>| {
            (
                f.len(),
                f.values().map(BTreeSet::len).sum::<usize>(),
                o.len(),
            )
        };
        let before = size(&feats, &on);
        let ids: Vec<String> = feats.keys().cloned().collect();
        for id in ids {
            let (Some(list), Some(pkg)) = (entries.get(&id), pkgs.get(id.as_str())) else {
                continue;
            };
            let declared = &pkg["features"];
            let alias_idx = |x: &str| {
                list.iter()
                    .enumerate()
                    .filter(|(_, (e, _))| e.alias == x)
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>()
            };
            let mut add: Vec<(String, String)> = Vec::new(); // (対象 package, feature)
            let mut enable: Vec<usize> = Vec::new();
            for f in feats[&id].iter() {
                let Some(items) = declared.get(f) else {
                    // `dep:` で使われていない optional 依存は同名の暗黙 feature を持つ。
                    enable.extend(alias_idx(f));
                    continue;
                };
                for item in strs(items) {
                    if let Some(x) = item.strip_prefix("dep:") {
                        enable.extend(alias_idx(x));
                    } else if let Some((x, sub)) = item.split_once('/') {
                        let (x, weak) = match x.strip_suffix('?') {
                            Some(x) => (x, true),
                            None => (x, false),
                        };
                        for i in alias_idx(x) {
                            if !weak {
                                enable.push(i);
                            }
                            let (e, pid) = &list[i];
                            if !weak || !e.optional || on.contains(&(id.clone(), i)) {
                                add.push((pid.clone(), sub.to_string()));
                            }
                        }
                    } else {
                        // 他の feature への参照（同名の optional 依存の暗黙 feature を含む）。
                        add.push((id.clone(), item));
                    }
                }
            }
            for (i, (e, pid)) in list.iter().enumerate() {
                if e.optional && !on.contains(&(id.clone(), i)) && !enable.contains(&i) {
                    continue;
                }
                on.insert((id.clone(), i));
                feats.entry(pid.clone()).or_default();
                if e.default {
                    add.push((pid.clone(), "default".into()));
                }
                add.extend(e.features.iter().map(|f| (pid.clone(), f.clone())));
            }
            for (pid, f) in add {
                feats.entry(pid).or_default().insert(f);
            }
        }
        if before == size(&feats, &on) {
            break;
        }
    }
    feats.into_keys().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const N: Value = Value::Null;

    /// 依存項目。`opt` は optional。
    fn dep(name: &str, opt: bool, target: Value, kind: Value) -> Value {
        json!({"name": name, "optional": opt, "features": [],
               "uses_default_features": true, "target": target, "kind": kind})
    }
    fn pkg(id: &str, features: Value, deps: Vec<Value>) -> Value {
        json!({"id": id, "features": features, "dependencies": deps})
    }
    fn edge(name: &str, pkg: &str, target: Value, kind: Value) -> Value {
        json!({"name": name, "pkg": pkg, "dep_kinds": [{"kind": kind, "target": target}]})
    }
    fn meta(pkgs: Vec<Value>, nodes: Vec<(&str, Vec<Value>)>) -> Value {
        let nodes: Vec<Value> = nodes
            .into_iter()
            .map(|(id, deps)| json!({"id": id, "deps": deps}))
            .collect();
        json!({"packages": pkgs, "resolve": {"root": "app", "nodes": nodes}})
    }

    #[test]
    fn optional_dep_needs_own_feature_not_workspace_union() {
        // resolve には optional の辺があるが、app の default は cap を有効にしない。
        let m = meta(
            vec![
                pkg(
                    "app",
                    json!({"default": [], "on": ["dep:cap"]}),
                    vec![dep("cap", true, N, N)],
                ),
                pkg("cap", json!({}), vec![]),
            ],
            vec![("app", vec![edge("cap", "cap", N, N)]), ("cap", vec![])],
        );
        assert_eq!(reachable(&m), BTreeSet::from(["app".to_string()]));
    }

    #[test]
    fn dep_syntax_slash_weak_and_implicit_features_enable_optional_dep() {
        for (feats, expect) in [
            (json!({"default": ["dep:cap"]}), true),
            (json!({"default": ["cap/x"]}), true),
            (json!({"default": ["cap"]}), true),
            (json!({"default": ["cap?/x"]}), false),
        ] {
            let m = meta(
                vec![
                    pkg("app", feats.clone(), vec![dep("cap", true, N, N)]),
                    pkg("cap", json!({"x": []}), vec![]),
                ],
                vec![("app", vec![edge("cap", "cap", N, N)]), ("cap", vec![])],
            );
            assert_eq!(reachable(&m).contains("cap"), expect, "{feats}");
        }
    }

    #[test]
    fn feature_forwarded_to_transitive_optional_dep() {
        let m = meta(
            vec![
                pkg(
                    "app",
                    json!({"default": ["mid/hw"]}),
                    vec![dep("mid", false, N, N)],
                ),
                pkg(
                    "mid",
                    json!({"hw": ["dep:cap"]}),
                    vec![dep("cap", true, N, N)],
                ),
                pkg("cap", json!({}), vec![]),
            ],
            vec![
                ("app", vec![edge("mid", "mid", N, N)]),
                ("mid", vec![edge("cap", "cap", N, N)]),
                ("cap", vec![]),
            ],
        );
        assert!(reachable(&m).contains("cap"));
    }

    #[test]
    fn same_name_different_source_are_distinct_ids() {
        let m = meta(
            vec![
                pkg("app", json!({}), vec![dep("a", false, N, N)]),
                pkg("a 1.0 (registry)", json!({}), vec![]),
                pkg("a 1.0 (path)", json!({}), vec![]),
            ],
            vec![
                ("app", vec![edge("a", "a 1.0 (path)", N, N)]),
                ("a 1.0 (registry)", vec![]),
                ("a 1.0 (path)", vec![]),
            ],
        );
        let r = reachable(&m);
        assert!(r.contains("a 1.0 (path)") && !r.contains("a 1.0 (registry)"));
    }

    #[test]
    fn dev_build_and_unresolved_target_entries_are_skipped() {
        let m = meta(
            vec![
                pkg(
                    "app",
                    json!({}),
                    vec![
                        dep("d", false, N, json!("dev")),
                        dep("b", false, N, json!("build")),
                        // resolve の dep_kinds に無い target の項目（別 OS 向け）。
                        dep("t", false, json!("cfg(windows)"), N),
                    ],
                ),
                pkg("d", json!({}), vec![]),
                pkg("b", json!({}), vec![]),
                pkg("t", json!({}), vec![]),
            ],
            vec![(
                "app",
                vec![
                    edge("d", "d", N, json!("dev")),
                    edge("b", "b", N, json!("build")),
                ],
            )],
        );
        assert_eq!(reachable(&m), BTreeSet::from(["app".to_string()]));
    }
}
