//! `cargo fandhe check` / `cargo fandhe gen`。
//! - `check`: workspace 内の機能宣言を検証する
//! - `gen`: アプリが選んだ機能の宣言からネイティブ設定を `<アプリの crate>/gen/` へ生成する
//!
//! 終了コード: 0 正常 / 1 宣言・入力の不正 / 2 未知の引数。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "使い方: cargo fandhe check|gen [--manifest-path <Cargo.toml>]";

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
    let (cmd, manifest): (&str, Option<PathBuf>) = match strs.as_slice() {
        [c @ ("check" | "gen")] => (c, None),
        [c @ ("check" | "gen"), "--manifest-path", p] => (c, Some(Path::new(p).to_path_buf())),
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
    match cargo_fandhe::generate(manifest.as_deref()) {
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
