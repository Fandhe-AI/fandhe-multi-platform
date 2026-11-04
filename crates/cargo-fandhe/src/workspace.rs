//! workspace 全体の検査。`cargo metadata` の JSON を入力にして、TOML の解釈は cargo に任せる。
//! 呼び出し元は `cargo fandhe check`（`main.rs`）と CI の `cargo test`（実 workspace への検査）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::declaration::{check_paths, parse, validate, Declaration};
use crate::merge::{merge, Merged};

const CAP_PREFIX: &str = "fandhe-cap-";

/// `manifest` の workspace にある全 crate の宣言を検証し、統合まで通す。
/// 問題は 1 件ずつの文字列（crate 名つき）で全件返す。
pub fn check_workspace(manifest: Option<&Path>) -> Result<Merged, Vec<String>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    // 引数は配列で渡し、シェルを介さない。
    cmd.args(["metadata", "--no-deps", "--format-version", "1"]);
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
    let mut decls: Vec<(String, Declaration)> = Vec::new();
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    for pkg in meta["packages"].as_array().into_iter().flatten() {
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
            decls.push((d.id.clone(), d));
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let refs: Vec<(&str, &Declaration)> = decls.iter().map(|(i, d)| (i.as_str(), d)).collect();
    merge(&refs).map_err(|e| vec![e.to_string()])
}
