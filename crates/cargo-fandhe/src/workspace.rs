//! workspace 全体の検査。`cargo metadata` の JSON を入力にして、TOML の解釈は cargo に任せる。
//! 呼び出し元は `cargo fandhe check`（`main.rs`）と CI の `cargo test`（実 workspace への検査）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::declaration::{check_paths, parse, validate, Declaration};

const CAP_PREFIX: &str = "fandhe-cap-";

/// `manifest` の workspace と、その解決済み依存にある全 crate の宣言を検証する。
/// 形式・命名・パスの検証だけを行い、統合はしない。どの機能を生成に入れるか（到達判定）は
/// `reach` モジュールと `cargo fandhe gen` の担当（capability-declaration.md §5）。
/// 問題は 1 件ずつの文字列（crate 名つき）で全件返し、問題がなければ検証済みの宣言を返す。
pub fn check_workspace(manifest: Option<&Path>) -> Result<Vec<Declaration>, Vec<String>> {
    let meta = run_metadata(manifest, None)?;
    Ok(collect(&meta)?.into_iter().map(|f| f.decl).collect())
}

/// 検証済みの宣言 1 件と、その持ち主の crate。`cargo fandhe gen` が到達判定（package id）と
/// 生成物のパス（`dir`）に使う。
pub(crate) struct Found {
    pub pkg_id: String,
    pub dir: PathBuf,
    pub decl: Declaration,
}

/// `cargo metadata` を実行して JSON を返す。`filter_platform` は対象 triple（OS ごとの依存解決用）。
/// 引数は配列で渡し、シェルを介さない。
/// `--no-deps` は付けない: workspace 外の path 依存・公開された `fandhe-cap-*` も解決済みの
/// 依存として packages に入り、宣言が集まる（consumer-app-structure.md §4）。
pub(crate) fn run_metadata(
    manifest: Option<&Path>,
    filter_platform: Option<&str>,
) -> Result<Value, Vec<String>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args(["metadata", "--format-version", "1"]);
    if let Some(t) = filter_platform {
        cmd.args(["--filter-platform", t]);
    }
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
    serde_json::from_slice(&output.stdout)
        .map_err(|e| vec![format!("cargo metadata の出力を読めません: {e}")])
}

/// metadata の全 package の宣言を検証して集める。
pub(crate) fn collect(meta: &Value) -> Result<Vec<Found>, Vec<String>> {
    let mut problems = Vec::new();
    let mut found: Vec<Found> = Vec::new();
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for pkg in meta["packages"].as_array().into_iter().flatten() {
        // 同じ package が複数回現れても 1 度だけ見る。
        let pkg_id = pkg["id"].as_str().unwrap_or("?").to_string();
        if !seen.insert(pkg_id.clone()) {
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
            let dir = pkg["manifest_path"]
                .as_str()
                .and_then(|p| Path::new(p).parent())
                .map(Path::to_path_buf)
                .unwrap_or_default();
            found.push(Found {
                pkg_id,
                dir,
                decl: d,
            });
        }
    }
    if problems.is_empty() {
        Ok(found)
    } else {
        Err(problems)
    }
}
