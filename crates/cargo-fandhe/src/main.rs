//! `cargo fandhe check`: workspace 内の機能宣言を検証する。
//! 終了コード: 0 正常 / 1 宣言の不正 / 2 未知の引数。

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `cargo fandhe ...` 経由では第 1 引数に `fandhe` が入る。
    if args.first().map(String::as_str) == Some("fandhe") {
        args.remove(0);
    }
    let manifest = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["check"] => None,
        ["check", "--manifest-path", p] => Some(Path::new(*p).to_path_buf()),
        _ => {
            eprintln!("使い方: cargo fandhe check [--manifest-path <Cargo.toml>]");
            return ExitCode::from(2);
        }
    };
    match cargo_fandhe::check_workspace(manifest.as_deref()) {
        Ok(_) => ExitCode::SUCCESS,
        Err(problems) => {
            for p in problems {
                eprintln!("エラー: {p}");
            }
            ExitCode::from(1)
        }
    }
}
