//! `cargo fandhe check` / `cargo fandhe gen`。
//! - `check`: workspace 内の機能宣言を検証する
//! - `gen`: アプリが選んだ機能の宣言からネイティブ設定を `<アプリの crate>/gen/` へ生成する（`--os web` は Web App Manifest だけ、#1238）
//! - `gen --eject <パス>`: 生成した設定ファイルを `<アプリの crate>/eject/` へ書き出す（以後は生成の代わりに使われる、#1202）
//! - `run` / `build`: アプリを OS 向けにビルド・起動する（macos・windows・linux・web・android・ios シミュレータ、#1422・#1423・#1424）
//! - `verify-scope`: 選ばなかった機能の宣言が成果物の観測テキストに無いことを検査する（#1178）
//!
//! 終了コード: 0 正常 / 1 宣言・入力の不正（`run`/`build` では前提の不足と子プロセスの失敗も 1） / 2 未知の引数。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "使い方: cargo fandhe check|gen [--os web] [--manifest-path <Cargo.toml>]\n       （--os web は gen のみ。Web App Manifest だけを生成する）\n       cargo fandhe gen --eject <パス> [--eject <パス>]... [--manifest-path <Cargo.toml>]\n       （<パス> は gen/ からの相対パス）\n       （生成した設定ファイルを <アプリの crate>/eject/ へ書き出す。以後は生成の代わりに使われ、消すと生成へ戻る。\n       　対象: android/AndroidManifest.xml・{ios,macos}/Info.plist・{ios,macos}/App.entitlements・{ios,macos}/extensions/<name>/ の plist・entitlements）\n       cargo fandhe run|build [--os macos|windows|linux|web|android|ios] [--release] [--device] [--manifest-path <アプリの Cargo.toml>]\n       （--os ios は iOS シミュレータ。--device（iOS 実機）は未対応で、指定すると止まる。#1425）\n       cargo fandhe verify-scope [--catalog <Cargo.toml>] [--manifest-path <アプリの Cargo.toml>] --os android|ios|macos <観測ファイル>...";

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
    if matches!(strs.first(), Some(&"run" | &"build")) {
        let Some(opts) = cargo_fandhe::run::parse_args(&strs) else {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        };
        return match cargo_fandhe::run::run_cli(&opts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(p) => report(p),
        };
    }
    let Some(&cmd @ ("check" | "gen")) = strs.first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let (mut web, mut manifest, mut ejects) = (false, None, Vec::new());
    let mut it = strs[1..].iter();
    let mut bad = false;
    while let Some(&a) = it.next() {
        match (a, it.clone().next()) {
            ("--os", Some(&"web")) => {
                web = true;
                it.next();
            }
            ("--manifest-path", Some(&v)) => {
                manifest = Some(Path::new(v).to_path_buf());
                it.next();
            }
            ("--eject", Some(&v)) if !v.starts_with('-') => {
                ejects.push(v.to_string());
                it.next();
            }
            _ => bad = true,
        }
    }
    // `--eject` は gen のネイティブ設定だけ（check では意味が無く、Web は eject の対象外）。
    if bad || (!ejects.is_empty() && (cmd == "check" || web)) || (web && cmd == "check") {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    if cmd == "check" {
        return match cargo_fandhe::check_workspace(manifest.as_deref()) {
            Ok(_) => ExitCode::SUCCESS,
            Err(p) => report(p),
        };
    }
    if !ejects.is_empty() {
        return match cargo_fandhe::eject(manifest.as_deref(), &ejects) {
            Ok(paths) => {
                for p in paths {
                    println!("{} へ書き出しました。消すと生成へ戻ります", p.display());
                }
                ExitCode::SUCCESS
            }
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
                for note in &g.notes {
                    eprintln!("{note}");
                }
                ExitCode::SUCCESS
            }
            Err(e) => report(vec![e]),
        },
        Err(p) => report(p),
    }
}
