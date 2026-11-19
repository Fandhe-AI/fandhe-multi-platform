//! `cargo fandhe run|build` の手順の組み立てと前提検査のテスト（Issue #1422）。
//!
//! 入力は fandhe-gallery の実メタデータ（`load_app`）と、引数で与える環境のスナップショット。
//! 手順の組み立て（`prepare`）では実行せず、「何を書き、何を実行するか」と「何が足りないと止まるか」を固定する。
//! 実行器（`execute`）はファイル操作と子プロセスの失敗の報告だけを、一時ディレクトリで確かめる。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cargo_fandhe::run::{
    execute, load_app, parse_args, prepare, Action, App, Env, Op, Options, Os, Step,
};

const SECRET: &str = "/secret-cargo-path-7f3a";

fn gallery_manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fandhe-gallery/Cargo.toml")
}

fn opts(action: Action, os: Os, release: bool) -> Options {
    Options {
        action,
        os: Some(os),
        release,
        manifest: Some(gallery_manifest()),
    }
}

fn gallery(os: Os) -> App {
    load_app(&opts(Action::Build, os, false), os).unwrap_or_else(|e| panic!("{e:?}"))
}

fn env(host: Os, cmds: &[&str], bindgen: &str) -> Env {
    Env {
        host: Some(host),
        commands: cmds.iter().map(|s| s.to_string()).collect(),
        installed_targets: Some(BTreeSet::from(["wasm32-unknown-unknown".to_string()])),
        wasm_bindgen_version: Some(bindgen.to_string()),
        cargo: SECRET.into(),
    }
}

fn full_env(app: &App) -> Env {
    env(
        Os::Macos,
        &["rustup", "wasm-bindgen", "wasm-opt"],
        app.wasm_bindgen.as_deref().unwrap_or("0"),
    )
}

fn spawns(ops: &[Op]) -> Vec<&Step> {
    ops.iter()
        .filter_map(|o| if let Op::Spawn(s) = o { Some(s) } else { None })
        .collect()
}

fn errs(r: Result<Vec<Op>, Vec<String>>) -> Vec<String> {
    r.expect_err("前提の不足で止まるはず")
}

#[test]
fn parse_args_accepts_options_and_rejects_unknown() {
    let o = parse_args(&[
        "run",
        "--os",
        "web",
        "--release",
        "--manifest-path",
        "a/Cargo.toml",
    ])
    .expect("解釈できる");
    assert_eq!(o.action, Action::Run);
    assert_eq!(o.os, Some(Os::Web));
    assert!(o.release);
    assert_eq!(o.manifest, Some(PathBuf::from("a/Cargo.toml")));
    let o = parse_args(&["build"]).expect("既定");
    assert_eq!((o.os, o.release, o.manifest), (None, false, None));
    for bad in [
        &["build", "--release", "--release"][..],
        &["build", "--os", "web", "--os", "macos"],
        &["build", "--manifest-path", "a", "--manifest-path", "b"],
        &["build", "--manifest-path", "--release"],
        &["build", "--manifest-path", "-x"],
        &["build", "--os", "beos"],
        &["build", "--os"],
        &["build", "--manifest-path"],
        &["build", "--bogus"],
        &["gen"],
    ] {
        assert!(parse_args(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn desktop_build_and_run_delegate_to_cargo() {
    let app = gallery(Os::Macos);
    let e = env(Os::Macos, &[], "0");
    let manifest = app.manifest.display().to_string();
    for (action, release, want) in [
        (Action::Build, false, vec!["build"]),
        (Action::Run, false, vec!["run"]),
        (Action::Build, true, vec!["build", "--release"]),
    ] {
        let ops = prepare(&app, &opts(action, Os::Macos, release), &e).expect("通る");
        let mut args: Vec<String> = want.iter().map(|s| s.to_string()).collect();
        args.extend(["--manifest-path".to_string(), manifest.clone()]);
        assert_eq!(
            ops,
            vec![Op::Spawn(Step {
                stage: "cargo",
                program: SECRET.into(),
                args
            })]
        );
    }
}

#[test]
fn desktop_os_defaults_to_host_and_cross_is_refused() {
    let app = gallery(Os::Macos);
    let e = env(Os::Macos, &[], "0");
    let mut o = opts(Action::Build, Os::Macos, false);
    o.os = None;
    assert!(prepare(&app, &o, &e).is_ok());
    for os in [Os::Windows, Os::Linux] {
        for action in [Action::Build, Action::Run] {
            let p = errs(prepare(&app, &opts(action, os, false), &e));
            assert_eq!(p.len(), 1);
            assert!(p[0].contains("クロスビルドの実行はできません"), "{p:?}");
        }
    }
    // ホストが対応外で --os も無ければ止まる。
    let mut e2 = e.clone();
    e2.host = None;
    assert!(errs(prepare(&app, &o, &e2))[0].contains("--os を指定"));
}

#[test]
fn desktop_without_bin_is_refused() {
    let mut app = gallery(Os::Macos);
    app.bins.clear();
    let p = errs(prepare(
        &app,
        &opts(Action::Build, Os::Macos, false),
        &env(Os::Macos, &[], "0"),
    ));
    assert!(p[0].contains("bin"), "{p:?}");
}

fn out(app: &App) -> PathBuf {
    app.target_dir.join("fandhe").join(&app.name).join("web")
}

#[test]
fn web_build_sequence_for_gallery() {
    let app = gallery(Os::Web);
    assert_eq!(app.name, "fandhe-gallery");
    assert_eq!(app.cdylib.as_deref(), Some("fandhe_gallery"));
    assert!(app.web_has_index);
    let e = full_env(&app);
    let ops = prepare(&app, &opts(Action::Build, Os::Web, false), &e).expect("通る");
    let web = out(&app);
    // 成果物の置き場の作り直しは cargo build の直後（失敗時に前回の成果物を消さない）、wasm-bindgen の前。
    assert!(matches!(&ops[0], Op::Spawn(s) if s.stage == "cargo"));
    assert_eq!(
        ops[1],
        Op::Reset {
            dir: web.clone(),
            within: app.target_dir.clone()
        }
    );
    assert!(matches!(&ops[2], Op::Spawn(s) if s.stage == "wasm-bindgen"));
    let steps = spawns(&ops);
    assert_eq!(
        steps.iter().map(|s| s.stage).collect::<Vec<_>>(),
        ["cargo", "wasm-bindgen"]
    );
    assert_eq!(
        steps[0].args,
        [
            "build",
            "--lib",
            "--target",
            "wasm32-unknown-unknown",
            "--manifest-path",
            &app.manifest.display().to_string()
        ]
    );
    let wasm = app
        .target_dir
        .join("wasm32-unknown-unknown/debug/fandhe_gallery.wasm");
    // glue は成果物の直下へ出す。
    assert_eq!(
        steps[1].args,
        [
            "--target",
            "web",
            "--out-dir",
            &web.display().to_string(),
            &wasm.display().to_string()
        ]
    );
    // gallery は web/ を持つので丸ごと写す。index.html もあるので既定ページは出さず、manifest も無い。
    assert!(ops.contains(&Op::CopyDir {
        from: app.dir.join("web"),
        to: web.clone()
    }));
    assert!(!ops.iter().any(|o| matches!(o, Op::Write { .. })));
    // フォントは fandhe-fonts の資産を写す。
    assert!(ops.iter().any(|o| matches!(o,
        Op::Copy { to, .. } if *to == web.join("fonts/noto-sans-jp-regular.otf"))));
    // build は配信しない。
    assert!(!ops.iter().any(|o| matches!(o, Op::Serve { .. })));
}

#[test]
fn web_release_adds_wasm_opt_and_run_serves() {
    let app = gallery(Os::Web);
    let e = full_env(&app);
    let ops = prepare(&app, &opts(Action::Run, Os::Web, true), &e).expect("通る");
    let steps = spawns(&ops);
    assert_eq!(
        steps.iter().map(|s| s.stage).collect::<Vec<_>>(),
        ["cargo", "wasm-bindgen", "wasm-opt"]
    );
    // run でも wasm は実行できないので build を使い、release は両方に伝わる。
    assert_eq!(steps[0].args[0], "build");
    assert!(steps[0].args.contains(&"--release".to_string()));
    let wasm = app
        .target_dir
        .join("wasm32-unknown-unknown/release/fandhe_gallery.wasm");
    assert!(Path::new(steps[1].args.last().expect("wasm"))
        .components()
        .eq(wasm.components()));
    assert_eq!(steps[2].args[0], "-Os");
    assert!(steps[2].args[1].ends_with("fandhe_gallery_bg.wasm"));
    // 最後が配信。
    assert_eq!(ops.last(), Some(&Op::Serve { dir: out(&app) }));
}

#[test]
fn web_without_app_index_writes_default_page_and_manifest() {
    let mut app = gallery(Os::Web);
    app.web_dir = None;
    app.web_has_index = false;
    app.font = None;
    app.web_manifest = Some("{\"name\":\"x\"}".to_string());
    let e = full_env(&app);
    let ops = prepare(&app, &opts(Action::Build, Os::Web, false), &e).expect("通る");
    let web = out(&app);
    let index = ops
        .iter()
        .find_map(|o| match o {
            Op::Write { path, body } if *path == web.join("index.html") => Some(body),
            _ => None,
        })
        .expect("既定の index.html");
    assert!(index.contains("id=\"fandhe-proxy-canvas\""));
    assert!(index.contains("style=\"width: 100vw; height: 100vh\""));
    assert!(index.contains("./fandhe_gallery.js"));
    assert!(index.contains("rel=\"manifest\""));
    assert!(ops.contains(&Op::Write {
        path: web.join("manifest.webmanifest"),
        body: "{\"name\":\"x\"}".to_string()
    }));
    assert!(!ops
        .iter()
        .any(|o| matches!(o, Op::Copy { .. } | Op::CopyDir { .. })));
    // web/ があっても index.html が無ければ、web/ を写したうえで既定ページを足す。
    app.web_dir = Some(app.dir.join("web"));
    let ops = prepare(&app, &opts(Action::Build, Os::Web, false), &e).expect("通る");
    let copy = ops
        .iter()
        .position(|o| matches!(o, Op::CopyDir { .. }))
        .expect("写す");
    let manifest = ops
        .iter()
        .position(|o| matches!(o, Op::Write { path, .. } if path.ends_with("manifest.webmanifest")))
        .expect("manifest");
    assert!(copy < manifest, "manifest は web/ の写しより後");
}

#[test]
fn web_prerequisites_are_all_collected_one_line_each() {
    let app = gallery(Os::Web);
    let mut e = env(Os::Macos, &["rustup"], "0.2.1");
    e.installed_targets = Some(BTreeSet::new());
    let p = errs(prepare(&app, &opts(Action::Run, Os::Web, true), &e));
    // target・wasm-bindgen（無い）・wasm-opt の 3 件。配信は内蔵なので外部コマンドは要らない。
    assert_eq!(p.len(), 3, "{p:?}");
    assert!(p.iter().all(|l| !l.contains('\n')));
    assert!(p[0].contains("rustup target add wasm32-unknown-unknown"));
    assert!(p[1].contains("wasm-bindgen CLI がありません") && p[1].contains("--locked"));
    assert!(p[2].contains("wasm-opt"));
}

#[test]
fn web_prerequisite_branches() {
    let app = gallery(Os::Web);
    let lock = app
        .wasm_bindgen
        .clone()
        .expect("gallery は wasm-bindgen に依存");
    let build = opts(Action::Build, Os::Web, false);

    // 版の不一致。
    let e = env(Os::Macos, &["wasm-bindgen"], "0.0.1");
    let p = errs(prepare(&app, &build, &e));
    assert_eq!(p.len(), 1);
    assert!(p[0].contains("0.0.1") && p[0].contains(&lock) && p[0].contains("一致しません"));
    // 版を取得できない。
    let mut e = env(Os::Macos, &["wasm-bindgen"], &lock);
    e.wasm_bindgen_version = None;
    assert!(errs(prepare(&app, &build, &e))[0].contains("版を取得できません"));
    // rustup が無ければ target の検査は飛ばす。
    let mut e = env(Os::Macos, &["wasm-bindgen"], &lock);
    e.installed_targets = None;
    assert!(prepare(&app, &build, &e).is_ok());
    // build は wasm-opt を要求しない。
    let e = env(Os::Macos, &["wasm-bindgen"], &lock);
    assert!(prepare(&app, &build, &e).is_ok());
    // cdylib が無い。
    let mut no_lib = app.clone();
    no_lib.cdylib = None;
    let e = env(Os::Macos, &["wasm-bindgen"], &lock);
    assert!(errs(prepare(&no_lib, &build, &e))[0].contains("cdylib"));
}

#[test]
fn errors_do_not_leak_environment_values() {
    let app = gallery(Os::Web);
    let mut e = env(Os::Macos, &[], "0.0.1");
    e.installed_targets = Some(BTreeSet::new());
    let mut all = errs(prepare(&app, &opts(Action::Run, Os::Web, true), &e));
    let mut d = env(Os::Macos, &[], "0");
    d.host = Some(Os::Macos);
    all.extend(errs(prepare(
        &app,
        &opts(Action::Build, Os::Linux, false),
        &d,
    )));
    assert!(!all.is_empty());
    assert!(all.iter().all(|l| !l.contains(SECRET)), "{all:?}");
}

#[test]
fn execute_reports_only_the_failed_stage() {
    let op = |program: &str| {
        vec![Op::Spawn(Step {
            stage: "cargo",
            program: program.into(),
            args: vec!["--secret-arg-9c1".to_string()],
        })]
    };
    let e = execute(&op("false")).expect_err("失敗");
    assert!(e.contains("cargo の段が失敗しました"), "{e}");
    assert!(!e.contains("secret-arg"));
    let e = execute(&op("/nonexistent-fandhe-cmd")).expect_err("起動できない");
    assert!(
        e.contains("cargo の段を起動できません") && !e.contains("secret-arg"),
        "{e}"
    );
    assert!(execute(&op("true")).is_ok());
}

// ---- load_app（実 cargo を通す。workspace 外の単独アプリ）----

/// 依存なしの単独アプリ（`[workspace]` を自分で持つ）を作り、その Cargo.toml を返す。
fn standalone(tag: &str, extra_toml: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("run-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("作成");
    std::fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [lib]\ncrate-type = [\"cdylib\", \"rlib\"]\n\n\
             [[bin]]\nname = \"my-app-desktop\"\npath = \"src/main.rs\"\n\n[workspace]\n\n{extra_toml}"
        ),
    )
    .expect("書き込み");
    std::fs::write(root.join("src/lib.rs"), "").expect("書き込み");
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("書き込み");
    root.join("Cargo.toml")
}

fn load(m: &Path, os: Os) -> Result<App, Vec<String>> {
    let o = Options {
        action: Action::Build,
        os: Some(os),
        release: false,
        manifest: Some(m.to_path_buf()),
    };
    load_app(&o, os)
}

#[test]
fn load_app_reads_a_standalone_app() {
    let m = standalone("plain", "");
    for os in [Os::Macos, Os::Web] {
        let app = load(&m, os).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(app.name, "my-app");
        assert_eq!(app.cdylib.as_deref(), Some("my_app"));
        assert_eq!(app.bins, ["my-app-desktop"]);
        // 依存が無いので wasm-bindgen もフォントも web/ も manifest も無い。
        assert_eq!(app.wasm_bindgen, None);
        assert!(app.font.is_none() && app.web_dir.is_none() && app.web_manifest.is_none());
        assert!(!app.web_has_index);
        assert!(app
            .manifest
            .components()
            .eq(m.canonicalize().expect("実在").components()));
        assert!(app.target_dir.is_absolute());
    }
}

#[test]
fn load_app_finds_web_dir_and_generates_manifest_only_when_declared() {
    let m = standalone(
        "webcfg",
        "[package.metadata.fandhe-app.web]\nmanifest = \"web.json\"\n",
    );
    let root = m.parent().expect("親");
    std::fs::write(root.join("web.json"), "{\"name\": \"My App\"}").expect("書き込み");
    std::fs::create_dir_all(root.join("web")).expect("作成");
    std::fs::write(root.join("web/index.html"), "<html></html>").expect("書き込み");
    let app = load(&m, Os::Web).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(app.web_has_index);
    assert!(app
        .web_dir
        .as_deref()
        .is_some_and(|d| d.components().eq(root
            .canonicalize()
            .expect("実在")
            .join("web")
            .components())));
    let body = app.web_manifest.expect("fandhe-app.web があるので出す");
    assert!(body.contains("My App"), "{body}");
    // デスクトップでは調べない。
    assert!(load(&m, Os::Macos).expect("通る").web_manifest.is_none());
    // テーブル以外は gen と同じくエラー。
    let bad = standalone(
        "webbad",
        "[package.metadata.fandhe-app]\nweb = \"web.json\"\n",
    );
    let e = load(&bad, Os::Web).expect_err("拒否").join("\n");
    assert!(e.contains("web"), "{e}");
}

#[cfg(unix)]
#[test]
fn symlinked_index_html_counts_as_missing() {
    let m = standalone("indexlink", "");
    let root = m.parent().expect("親");
    std::fs::create_dir_all(root.join("web")).expect("作成");
    std::fs::write(root.join("real.html"), "x").expect("書き込み");
    std::os::unix::fs::symlink(root.join("real.html"), root.join("web/index.html")).expect("link");
    let app = load(&m, Os::Web).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(app.web_dir.is_some());
    assert!(
        !app.web_has_index,
        "symlink は写されないので既定ページへ倒す"
    );
}

// ---- execute のファイル操作 ----

fn scratch(tag: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("run-exec-{tag}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("作成");
    d
}

#[test]
fn execute_writes_copies_and_recreates_directories() {
    let t = scratch("ops");
    let src = t.join("src");
    std::fs::create_dir_all(src.join("sub")).expect("作成");
    std::fs::write(src.join("a.txt"), "a").expect("書き込み");
    std::fs::write(src.join("sub/b.txt"), "b").expect("書き込み");
    let out = t.join("target/fandhe/app/web");
    std::fs::create_dir_all(&out).expect("作成");
    std::fs::write(out.join("stale.txt"), "old").expect("書き込み");
    execute(&[
        Op::Reset {
            dir: out.clone(),
            within: t.join("target"),
        },
        Op::CopyDir {
            from: src.clone(),
            to: out.clone(),
        },
        Op::Write {
            path: out.join("deep/w.txt"),
            body: "w".into(),
        },
        Op::Copy {
            from: src.join("a.txt"),
            to: out.join("c/a2.txt"),
        },
    ])
    .expect("通る");
    let read = |p: &str| std::fs::read_to_string(out.join(p)).expect(p);
    assert_eq!(read("a.txt"), "a");
    assert_eq!(read("sub/b.txt"), "b");
    assert_eq!(read("deep/w.txt"), "w");
    assert_eq!(read("c/a2.txt"), "a");
    assert!(
        !out.join("stale.txt").exists(),
        "作り直しで古いファイルは消える"
    );
}

#[test]
fn reset_refuses_paths_outside_or_through_symlinks() {
    let t = scratch("reset");
    let within = t.join("target");
    std::fs::create_dir_all(&within).expect("作成");
    // 配下ではない・within そのもの。
    for dir in [t.join("other"), within.clone()] {
        let e = execute(&[Op::Reset {
            dir: dir.clone(),
            within: within.clone(),
        }])
        .expect_err("拒否");
        assert!(e.contains("配下ではない"), "{e}");
    }
    // `..` を含み字句上だけ配下に見える形は、外のディレクトリを消さずに拒否する。
    let victim = t.join("victim");
    std::fs::create_dir_all(&victim).expect("作成");
    std::fs::write(victim.join("keep.txt"), "k").expect("書き込み");
    let e = execute(&[Op::Reset {
        dir: within.join("../victim"),
        within: within.clone(),
    }])
    .expect_err("`..` は拒否");
    assert!(e.contains("配下ではない"), "{e}");
    assert!(victim.join("keep.txt").exists());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&victim, within.join("fandhe")).expect("link");
        let e = execute(&[Op::Reset {
            dir: within.join("fandhe/app/web"),
            within: within.clone(),
        }])
        .expect_err("symlink は拒否");
        assert!(e.contains("symlink"), "{e}");
        assert!(victim.join("keep.txt").exists());
    }
}

#[test]
fn copy_dir_skips_symlinks() {
    let t = scratch("copy-link");
    let src = t.join("src");
    std::fs::create_dir_all(&src).expect("作成");
    std::fs::write(src.join("real.txt"), "r").expect("書き込み");
    #[cfg(unix)]
    {
        std::fs::write(t.join("secret.txt"), "s").expect("書き込み");
        std::os::unix::fs::symlink(t.join("secret.txt"), src.join("link.txt")).expect("link");
    }
    execute(&[Op::CopyDir {
        from: src,
        to: t.join("dst"),
    }])
    .expect("通る");
    assert!(t.join("dst/real.txt").is_file());
    assert!(!t.join("dst/link.txt").exists());
}

#[test]
fn web_finds_font_even_when_fonts_crate_is_not_a_wasm_dependency() {
    // 描画層と同じく wasm32 以外でだけ依存する形（利用者のアプリが描画層経由で持つ形）。
    let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fandhe-fonts");
    let m = standalone(
        "fontdep",
        &format!(
            "[target.'cfg(not(target_arch = \"wasm32\"))'.dependencies]\n\
             fandhe-fonts = {{ path = {:?} }}\n",
            fonts.canonicalize().expect("実在")
        ),
    );
    let app = load(&m, Os::Web).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(
        app.font.as_deref().is_some_and(Path::is_file),
        "{:?}",
        app.font
    );
}

#[cfg(unix)]
#[test]
fn symlinked_web_dir_is_ignored_and_not_copied() {
    let m = standalone("weblink", "");
    let root = m.parent().expect("親");
    let outside = root.join("private");
    std::fs::create_dir_all(&outside).expect("作成");
    std::fs::write(outside.join("secret.txt"), "s").expect("書き込み");
    std::fs::write(outside.join("index.html"), "x").expect("書き込み");
    std::os::unix::fs::symlink(&outside, root.join("web")).expect("link");
    let app = load(&m, Os::Web).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(app.web_dir.is_none(), "symlink の web/ は採らない");
    assert!(
        !app.web_has_index,
        "採らない web/ の index.html は数えず、既定ページを出す"
    );
    // 実行器も入口で拒否する。
    let t = scratch("copy-root-link");
    let e = execute(&[Op::CopyDir {
        from: root.join("web"),
        to: t.join("dst"),
    }])
    .expect_err("拒否");
    assert!(e.contains("symlink"), "{e}");
    assert!(!t.join("dst/secret.txt").exists());
}
