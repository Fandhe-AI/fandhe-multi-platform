//! workspace 全体の検査。`cargo metadata` の JSON を入力にして、TOML の解釈は cargo に任せる。
//! 呼び出し元は `cargo fandhe check`（`main.rs`）と CI の `cargo test`（実 workspace への検査）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::declaration::{check_paths, parse, validate, Declaration};
use crate::merge::{merge, Merged};

/// `(name, version, 取得元)`。
type PkgKey = (String, String, String);

const CAP_PREFIX: &str = "fandhe-cap-";

/// `manifest` の workspace と、その解決済み依存にある全 crate の宣言を検証する。
/// 統合は `manifest` が指すアプリ（root package）から通常依存で到達する crate に限る
/// （選ばなかった機能の権限を入れない。consumer-app-structure.md §4）。
/// 仮想 workspace（root package なし）では対象アプリが決まらないため、宣言を持つ全 crate を統合する。
/// 問題は 1 件ずつの文字列（crate 名つき）で全件返す。
pub fn check_workspace(manifest: Option<&Path>) -> Result<Merged, Vec<String>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    // 引数は配列で渡し、シェルを介さない。
    // `--no-deps` は付けない: workspace 外の path 依存・公開された `fandhe-cap-*` も解決済みの
    // 依存として packages に入り、宣言が集まる（consumer-app-structure.md §4）。
    cmd.args(["metadata", "--format-version", "1"]);
    if let Some(m) = manifest {
        cmd.arg("--manifest-path").arg(m);
    }
    let output = cmd
        .output()
        .map_err(|e| vec![format!("cargo metadata を実行できません: {e}")])?;
    if !output.status.success() {
        return Err(vec![format!(
            "cargo metadata が失敗しました: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )]);
    }
    let meta: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| vec![format!("cargo metadata の出力を読めません: {e}")])?;

    let mut problems = Vec::new();
    let mut decls: Vec<(PkgKey, String, Declaration)> = Vec::new();
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for pkg in meta["packages"].as_array().into_iter().flatten() {
        // 同じ package が複数回現れても 1 度だけ見る。
        if !seen.insert(pkg["id"].as_str().unwrap_or("?").to_string()) {
            continue;
        }
        let name = pkg["name"].as_str().unwrap_or("?");
        let decl = &pkg["metadata"]["fandhe"];
        if decl.is_null() {
            if name.starts_with(CAP_PREFIX) {
                problems.push(format!(
                    "{name}: `{CAP_PREFIX}*` なのに [package.metadata.fandhe] がありません"
                ));
            }
            continue;
        }
        let d = match parse(decl.clone()) {
            Ok(d) => d,
            Err(p) => {
                problems.push(format!("{name}: {p}"));
                continue;
            }
        };
        let before = problems.len();
        problems.extend(validate(&d).into_iter().map(|p| format!("{name}: {p}")));
        if let Some(dir) = pkg["manifest_path"]
            .as_str()
            .and_then(|p| Path::new(p).parent())
        {
            problems.extend(
                check_paths(&d, &PathBuf::from(dir))
                    .into_iter()
                    .map(|p| format!("{name}: {p}")),
            );
        }
        if name.starts_with(CAP_PREFIX) && name[CAP_PREFIX.len()..] != d.id {
            problems.push(format!(
                "{name}: id `{}` が crate 名 `{CAP_PREFIX}<id>` と一致しません",
                d.id
            ));
        }
        if let Some(prev) = ids.insert(d.id.clone(), name.to_string()) {
            problems.push(format!("{name}: id `{}` が {prev} と重複しています", d.id));
        }
        if problems.len() == before {
            let ver = pkg["version"].as_str().unwrap_or("?").to_string();
            let src = meta_source(pkg);
            decls.push(((name.to_string(), ver, src), d.id.clone(), d));
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let reach = reachable(&meta, manifest).map_err(|e| vec![e])?;
    let refs: Vec<(&str, &Declaration)> = decls
        .iter()
        .filter(|(pid, _, _)| reach.as_ref().is_none_or(|r| r.contains(pid)))
        .map(|(_, i, d)| (i.as_str(), d))
        .collect();
    merge(&refs).map_err(|es| es.iter().map(|e| e.to_string()).collect())
}

/// root package から通常依存（build・dev を除く）で到達する `(name, version, 取得元)`。
/// 取得元は registry・git・path の同名同版を区別する（`meta_source` と同じ正規化）。root が無ければ `None`。
/// `--target all` で `[target.'cfg(...)'.dependencies]` の依存（Android 専用機能等）も全対象ぶん集める。
/// `cargo metadata` の resolve は workspace 全体の feature を統合するため辿らない。
/// `cargo tree` は対象 package の feature 選択だけで解決する（別 member が有効にした feature の辺が混ざらない）。
fn reachable(meta: &Value, manifest: Option<&Path>) -> Result<Option<BTreeSet<PkgKey>>, String> {
    let Some(root) = meta["resolve"]["root"].as_str() else {
        return Ok(None);
    };
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args([
        "tree", "-e", "normal", "--target", "all", "--prefix", "none", "--format", "{p}",
    ]);
    // default-members があっても root package の依存だけを辿るよう、package を明示する。
    cmd.args(["--package", root]);
    if let Some(m) = manifest {
        cmd.arg("--manifest-path").arg(m);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("cargo tree を実行できません: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo tree が失敗しました: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    // 行は `name vX.Y.Z (path)` / `name vX.Y.Z` / `... (*)`。
    Ok(Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let (n, v) = (it.next()?.to_string(), it.next()?.trim_start_matches('v'));
                // 取得元は `(...)` の中身。無ければ crates.io。`(*)`（重複表示）は取得元ではない。
                let src = it
                    .next()
                    .filter(|t| *t != "(*)")
                    .map(|t| t.trim_matches(|c| c == '(' || c == ')').to_string())
                    .unwrap_or_default();
                Some((n, v.to_string(), src))
            })
            .collect(),
    ))
}

/// `cargo tree` の取得元表記に合わせた package の取得元。
/// crates.io は空、git は `git+` を除いた URL、path は package のディレクトリ。
fn meta_source(pkg: &Value) -> String {
    match pkg["source"].as_str() {
        None => pkg["manifest_path"]
            .as_str()
            .and_then(|p| Path::new(p).parent())
            .map(|d| d.display().to_string())
            .unwrap_or_default(),
        Some(s) if s.starts_with("git+") => s["git+".len()..].to_string(),
        Some(s) if s.contains("crates.io-index") => String::new(),
        Some(s) => s.to_string(),
    }
}
