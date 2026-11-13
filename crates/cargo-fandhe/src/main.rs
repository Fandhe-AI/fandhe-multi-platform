//! `cargo fandhe check` / `cargo fandhe gen`。
//! - `check`: workspace 内の機能宣言を検証する
//! - `gen`: アプリが選んだ機能の宣言からネイティブ設定を `<アプリの crate>/gen/` へ生成する（`--os web` は Web App Manifest だけ、#1238）
//! - `verify-scope`: 選ばなかった機能の宣言が成果物の観測テキストに無いことを検査する（#1178）
//!
//! 終了コード: 0 正常 / 1 宣言・入力の不正 / 2 未知の引数。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "使い方: cargo fandhe check|gen [--os web] [--manifest-path <Cargo.toml>]\n       （--os web は gen のみ。Web App Manifest だけを生成する）\n       cargo fandhe verify-scope [--catalog <Cargo.toml>] [--manifest-path <アプリの Cargo.toml>] --os android|ios|macos <観測ファイル>...";

/// `verify-scope` の引数処理と実行。観測ファイルは UTF-8 のテキストとして連結して読む。
fn verify_scope(args: &[&str]) -> ExitCode {
    let (mut catalog, mut app, mut os, mut files) = (None, None, None, Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match *a {
            "--catalog" | "--manifest-path" | "--os" => {
                let Some(v) = it.next() else {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                };
                match *a {
                    "--catalog" => catalog = Some(PathBuf::from(v)),
                    "--manifest-path" => app = Some(PathBuf::from(v)),
                    _ => os = Some(v.to_string()),
                }
            }
            f if !f.starts_with('-') => files.push(f),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(os) = os else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut observed = String::new();
    for f in files {
        match std::fs::read_to_string(f) {
            Ok(t) => observed += &t,
            Err(e) => return report(vec![format!("{f} を読めません: {e}")]),
        }
        observed.push('\n');
    }
    match cargo_fandhe::verify_scope(catalog.as_deref(), app.as_deref(), &os, &observed) {
        Ok(r) if r.unselected == 0 => {
            println!("選ばなかった機能はありません（検査対象なし）");
            ExitCode::SUCCESS
        }
        Ok(r) => {
            println!(
                "選ばなかった機能 {} 件の宣言 {} 値が成果物に無いことを確認しました",
                r.unselected, r.forbidden
            );
            ExitCode::SUCCESS
        }
        Err(p) => report(p),
    }
}

fn report(problems: Vec<String>) -> ExitCode {
    for p in problems {
        eprintln!("エラー: {p}");
    }
    ExitCode::from(1)
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `cargo fandhe ...` 経由では第 1 引数に `fandhe` が入る。
    if args.first().map(String::as_str) == Some("fandhe") {
        args.remove(0);
    }
    let strs: Vec<&str> = args.iter().map(String::as_str).collect();
    if strs.first() == Some(&"verify-scope") {
        return verify_scope(&strs[1..]);
    }
    let (cmd, web, manifest): (&str, bool, Option<PathBuf>) = match strs.as_slice() {
        [c @ ("check" | "gen")] => (c, false, None),
        [c @ ("check" | "gen"), "--manifest-path", p] => {
            (c, false, Some(Path::new(p).to_path_buf()))
        }
        ["gen", "--os", "web"] => ("gen", true, None),
        ["gen", "--os", "web", "--manifest-path", p] => {
            ("gen", true, Some(Path::new(p).to_path_buf()))
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    if cmd == "check" {
        return match cargo_fandhe::check_workspace(manifest.as_deref()) {
            Ok(_) => ExitCode::SUCCESS,
            Err(p) => report(p),
        };
    }
    let generated = if web {
        cargo_fandhe::generate_web(manifest.as_deref())
    } else {
        cargo_fandhe::generate(manifest.as_deref())
    };
    match generated {
        Ok(g) => match cargo_fandhe::write(&g.gen_dir, &g.files) {
            Ok(()) => {
                println!(
                    "{} に {} ファイルを生成しました",
                    g.gen_dir.display(),
                    g.files.len()
                );
                ExitCode::SUCCESS
            }
            Err(e) => report(vec![e]),
        },
        Err(p) => report(p),
    }
}
