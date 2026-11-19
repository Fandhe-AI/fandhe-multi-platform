//! `cargo fandhe run|build --os web` の手順（`run/mod.rs` の [`prepare`](super::prepare) から呼ばれる）。
//!
//! cdylib を wasm32 でビルドし、`wasm-bindgen --target web` の出力を成果物のディレクトリ直下へ置き、
//! アプリの `web/`・manifest・フォントを足して組む。canvas の id・CSS サイズは `fandhe` の `src/entry/web.rs`
//! と `docs/architecture/web-canvas-css-sizing.md` の要求に合わせる。

use std::path::Path;

use super::{cargo_step, Action, App, Env, Op, Options, Step};
use crate::native::WEB_TRIPLE;

/// 前提を全件検査し、問題がなければ手順を返す。`dir` は成果物の置き場。
pub(super) fn prepare(
    app: &App,
    opts: &Options,
    env: &Env,
    dir: &Path,
) -> Result<Vec<Op>, Vec<String>> {
    let mut problems = Vec::new();
    check_tools(app, opts, env, &mut problems);
    match &app.cdylib {
        Some(name) if problems.is_empty() => Ok(plan(app, opts, env, dir, name)),
        Some(_) => Err(problems),
        None => {
            problems.push(
                "lib の crate-type に cdylib がありません。`[lib]` の `crate-type` に \"cdylib\" を足してください"
                    .to_string(),
            );
            Err(problems)
        }
    }
}

fn check_tools(app: &App, opts: &Options, env: &Env, problems: &mut Vec<String>) {
    if let Some(t) = &env.installed_targets {
        if !t.contains(WEB_TRIPLE) {
            problems.push(format!(
                "Rust の target {WEB_TRIPLE} がありません。`rustup target add {WEB_TRIPLE}` を実行してください"
            ));
        }
    }
    let install = match &app.wasm_bindgen {
        Some(v) => format!("`cargo install wasm-bindgen-cli --version {v} --locked`"),
        None => "`cargo install wasm-bindgen-cli --locked`".to_string(),
    };
    if !env.commands.contains("wasm-bindgen") {
        problems.push(format!(
            "wasm-bindgen CLI がありません。{install} を実行してください"
        ));
    } else {
        match (&env.wasm_bindgen_version, &app.wasm_bindgen) {
            (None, _) => problems.push(format!(
                "wasm-bindgen CLI の版を取得できません。{install} で入れ直してください"
            )),
            (Some(cli), Some(lock)) if cli != lock => problems.push(format!(
                "wasm-bindgen CLI の版 {cli} が Cargo.lock の {lock} と一致しません。{install} を実行してください"
            )),
            _ => {}
        }
    }
    if opts.release && !env.commands.contains("wasm-opt") {
        problems.push(
            "wasm-opt がありません（--release で使います）。binaryen を入れてください（例: `brew install binaryen`）"
                .to_string(),
        );
    }
}

/// 検査を通ったアプリの手順。`name` は検査済みの cdylib 名。
fn plan(app: &App, opts: &Options, env: &Env, dir: &Path, name: &str) -> Vec<Op> {
    let profile = if opts.release { "release" } else { "debug" };
    let wasm = app
        .target_dir
        .join(WEB_TRIPLE)
        .join(profile)
        .join(format!("{name}.wasm"));
    // run でもここでは build 相当のコマンドを使う（wasm は実行できない）。
    let build = Options {
        action: Action::Build,
        ..opts.clone()
    };
    // 成果物の作り直しは cargo build の後ろ: コンパイルに失敗しても前回の成果物を消さず、
    // cargo が先に target/ を作って CACHEDIR.TAG を置けるようにする。
    let mut ops = vec![
        Op::Spawn(cargo_step(
            app,
            &build,
            env,
            &["--lib", "--target", WEB_TRIPLE],
        )),
        Op::Reset {
            dir: dir.to_path_buf(),
            within: app.target_dir.clone(),
        },
        Op::Spawn(Step {
            stage: "wasm-bindgen",
            program: "wasm-bindgen".into(),
            args: vec![
                "--target".into(),
                "web".into(),
                "--out-dir".into(),
                dir.display().to_string(),
                wasm.display().to_string(),
            ],
        }),
    ];
    if opts.release {
        let bg = dir.join(format!("{name}_bg.wasm")).display().to_string();
        ops.push(Op::Spawn(Step {
            stage: "wasm-opt",
            program: "wasm-opt".into(),
            args: vec!["-Os".into(), bg.clone(), "-o".into(), bg],
        }));
    }
    // アプリの web/ を写してから、生成した manifest を置く（manifest は生成物が正）。
    if let Some(web) = &app.web_dir {
        ops.push(Op::CopyDir {
            from: web.clone(),
            to: dir.to_path_buf(),
        });
    }
    if let Some(body) = &app.web_manifest {
        ops.push(Op::Write {
            path: dir.join("manifest.webmanifest"),
            body: body.clone(),
        });
    }
    if !app.web_has_index {
        ops.push(Op::Write {
            path: dir.join("index.html"),
            body: default_index(name, app.web_manifest.is_some()),
        });
    }
    if let Some(from) = &app.font {
        ops.push(Op::Copy {
            from: from.clone(),
            to: dir.join("fonts/noto-sans-jp-regular.otf"),
        });
    }
    ops.push(Op::Say(format!("Web の成果物: {}", dir.display())));
    if opts.action == Action::Run {
        ops.push(Op::Serve {
            dir: dir.to_path_buf(),
        });
    }
    ops
}

/// アプリが `web/index.html` を持たないときの既定ページ。canvas は `entry/web.rs` が探す id で、
/// CSS サイズはインライン style で明示する（Safari の無限リサイズ対策）。`name` は crate の lib 名（識別子）。
fn default_index(name: &str, manifest: bool) -> String {
    let link = if manifest {
        "    <link rel=\"manifest\" href=\"manifest.webmanifest\" />\n"
    } else {
        ""
    };
    format!(
        "<!doctype html>\n<html lang=\"ja\">\n  <head>\n    <meta charset=\"utf-8\" />\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n    <title>{name}</title>\n{link}    <style>\n      html,\n      body {{\n        margin: 0;\n        padding: 0;\n      }}\n      #fandhe-proxy-canvas {{\n        display: block;\n      }}\n    </style>\n  </head>\n  <body>\n    <canvas\n      id=\"fandhe-proxy-canvas\"\n      width=\"480\"\n      height=\"360\"\n      style=\"width: 100vw; height: 100vh\"\n    ></canvas>\n    <script type=\"module\">\n      import init from \"./{name}.js\";\n      await init();\n    </script>\n  </body>\n</html>\n"
    )
}
