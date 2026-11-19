//! `cargo fandhe run|build` の手順の組み立てと前提検査のテスト（Issue #1422）。
//!
//! 入力は fandhe-gallery の実メタデータ（`load_app`）と、引数で与える環境のスナップショット。
//! 手順の組み立て（`prepare`）では実行せず、「何を書き、何を実行するか」と「何が足りないと止まるか」を固定する。
//! 実行器（`execute`）はファイル操作と子プロセスの失敗の報告だけを、一時ディレクトリで確かめる。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cargo_fandhe::run::{
    execute, load_app, parse_adb_devices, parse_args, prepare, Action, AndroidEnv, App, Env, Op,
    Options, Os, Step,
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
        android: AndroidEnv::default(),
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
                args,
                env: vec![]
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
            Op::Write { path, body, .. } if *path == web.join("index.html") => Some(body),
            _ => None,
        })
        .expect("既定の index.html");
    assert!(index.contains("id=\"fandhe-proxy-canvas\""));
    assert!(index.contains("style=\"width: 100vw; height: 100vh\""));
    assert!(index.contains("./fandhe_gallery.js"));
    assert!(index.contains("rel=\"manifest\""));
    assert!(ops.contains(&Op::Write {
        path: web.join("manifest.webmanifest"),
        within: app.target_dir.clone(),
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
            env: vec![],
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
            within: t.join("target"),
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

// ---- Android（Issue #1423）----

const KEYSTORE: &str = "/ks/secret-keystore-4d1.jks";
const ALL_TRIPLES: [&str; 4] = [
    "aarch64-linux-android",
    "armv7-linux-androideabi",
    "x86_64-linux-android",
    "i686-linux-android",
];

fn android_opts(action: Action, release: bool) -> Options {
    opts(action, Os::Android, release)
}

/// すべてそろった Android 環境。端末は 1 台（x86_64）。署名の変数は release のテストだけ足す。
fn aenv(signed: bool) -> Env {
    let mut e = env(Os::Macos, &["cargo-ndk", "gradle", "java", "adb"], "0");
    e.installed_targets = Some(ALL_TRIPLES.iter().map(|s| s.to_string()).collect());
    e.android = AndroidEnv {
        sdk: true,
        ndk: true,
        devices: vec!["emulator-5554".into()],
        serial_env: None,
        device_abi: Some("x86_64".into()),
        signing: if signed {
            [
                "FANDHE_ANDROID_KEYSTORE",
                "FANDHE_ANDROID_KEYSTORE_PASSWORD",
                "FANDHE_ANDROID_KEY_ALIAS",
            ]
            .map(String::from)
            .into()
        } else {
            BTreeSet::new()
        },
        keystore: signed.then(|| PathBuf::from(KEYSTORE)),
        keystore_file: signed,
    };
    e
}

fn shell(app: &App) -> PathBuf {
    app.target_dir
        .join("fandhe")
        .join(&app.name)
        .join("android")
}

fn written<'a>(ops: &'a [Op], rel: &str, shell: &Path) -> &'a str {
    ops.iter()
        .find_map(|o| match o {
            Op::Write { path, body, .. } if *path == shell.join(rel) => Some(body.as_str()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{rel} を書く手順がありません"))
}

#[test]
fn android_build_sequence_for_gallery() {
    let app = gallery(Os::Android);
    assert_eq!(app.application_id.as_deref(), Some("com.fandhe.androidapp"));
    let ops = prepare(&app, &android_opts(Action::Build, false), &aenv(false)).expect("通る");
    let sh = shell(&app);
    let jni = sh.join("app/src/main/jniLibs");
    // gen（Android の分だけ。出力先は殻の下で、アプリの gen/ ではない）→ app/src の作り直し → 殻の書き出し → cargo-ndk → gradle。
    assert!(
        matches!(&ops[0], Op::Gen { dir, within, files } if *dir == sh.join("gen") && *within == app.target_dir && files.contains_key("android/fandhe.gradle") && !files.keys().any(|k| k.starts_with("ios/")))
    );
    assert_ne!(sh.join("gen"), app.dir.join("gen"));
    assert_eq!(
        ops[1],
        Op::Reset {
            dir: sh.join("app/src"),
            within: app.target_dir.clone()
        }
    );
    assert!(ops[2..ops.len() - 3]
        .iter()
        .all(|o| matches!(o, Op::Write { path, within, .. } if path.starts_with(&sh) && *within == app.target_dir)));
    let steps = spawns(&ops);
    assert_eq!(
        steps.iter().map(|s| s.stage).collect::<Vec<_>>(),
        ["cargo-ndk", "gradle"]
    );
    assert_eq!(
        steps[0].args,
        [
            "ndk",
            "-t",
            "arm64-v8a",
            "-t",
            "armeabi-v7a",
            "-t",
            "x86_64",
            "--platform",
            "24",
            "-o",
            &jni.display().to_string(),
            "build",
            "--lib",
            "--manifest-path",
            &app.manifest.display().to_string()
        ]
    );
    assert_eq!(
        steps[1].args,
        [
            "-p",
            &sh.display().to_string(),
            ":app:assembleDebug",
            "--console=plain"
        ]
    );
    assert!(steps[1].env.is_empty());
    // 殻の中身の要点。
    let app_gradle = written(&ops, "app/build.gradle.kts", &sh);
    assert!(app_gradle.contains("applicationId = \"com.fandhe.androidapp\""));
    assert!(app_gradle.contains(&format!(
        "apply(from = \"{}\")",
        sh.join("gen/android/fandhe.gradle").display()
    )));
    let main = written(
        &ops,
        "app/src/main/java/com/fandhe/platform/MainActivity.java",
        &sh,
    );
    assert!(
        main.contains("extends GameActivity")
            && main.contains("System.loadLibrary(FandheLibrary.NAME)")
    );
    assert!(
        written(&ops, "app/src/main/AndroidManifest.xml", &sh).contains("@style/FandheAppTheme")
    );
}

#[test]
fn android_run_uses_the_device_abi_and_starts_the_activity() {
    let app = gallery(Os::Android);
    let ops = prepare(&app, &android_opts(Action::Run, false), &aenv(false)).expect("通る");
    let steps = spawns(&ops);
    assert_eq!(
        steps.iter().map(|s| s.stage).collect::<Vec<_>>(),
        [
            "cargo-ndk",
            "gradle",
            "adb-install",
            "adb-start",
            "adb-check"
        ]
    );
    let ndk = &steps[0].args;
    assert_eq!(ndk.iter().filter(|a| *a == "-t").count(), 1);
    assert!(ndk.windows(2).any(|w| w == ["-t", "x86_64"]));
    let apk = shell(&app).join("app/build/outputs/apk/debug/app-debug.apk");
    assert_eq!(
        steps[2].args,
        [
            "-s",
            "emulator-5554",
            "install",
            "-r",
            &apk.display().to_string()
        ]
    );
    assert_eq!(
        steps[3].args,
        [
            "-s",
            "emulator-5554",
            "shell",
            "am",
            "start",
            "-W",
            "-n",
            "com.fandhe.androidapp/com.fandhe.platform.MainActivity"
        ]
    );
    assert_eq!(
        steps[4].args,
        [
            "-s",
            "emulator-5554",
            "shell",
            "pidof",
            "com.fandhe.androidapp"
        ]
    );
    // ANDROID_SERIAL があればそれを使う（複数台でも通る）。
    let mut e = aenv(false);
    e.android.devices = vec!["a1".into(), "b2".into()];
    e.android.serial_env = Some("b2".into());
    let ops = prepare(&app, &android_opts(Action::Run, false), &e).expect("通る");
    assert!(spawns(&ops)[2]
        .args
        .starts_with(&["-s".to_string(), "b2".to_string()]));
}

#[test]
fn android_release_requires_signing_and_passes_only_the_keystore_path() {
    let app = gallery(Os::Android);
    // build --release は .aab、run --release は .apk。パスワードは載せず keystore だけ絶対パスで上書きする。
    let ops = prepare(&app, &android_opts(Action::Build, true), &aenv(true)).expect("通る");
    let steps = spawns(&ops);
    assert!(steps[0].args.contains(&"--release".to_string()));
    assert_eq!(steps[1].args[2], ":app:bundleRelease");
    assert_eq!(
        steps[1].env,
        [("FANDHE_ANDROID_KEYSTORE".to_string(), KEYSTORE.into())]
    );
    let ops = prepare(&app, &android_opts(Action::Run, true), &aenv(true)).expect("通る");
    assert_eq!(spawns(&ops)[1].args[2], ":app:assembleRelease");
    // 署名の殻は環境変数だけを読み、値を持たない。
    let g = written(&ops, "app/build.gradle.kts", &shell(&app));
    assert!(
        g.contains("System.getenv(\"FANDHE_ANDROID_KEYSTORE_PASSWORD\")") && !g.contains(KEYSTORE)
    );
    // 1 つでも欠ければ、欠けた名前だけを挙げて止まる（値は出さない）。
    for missing in [
        "FANDHE_ANDROID_KEYSTORE",
        "FANDHE_ANDROID_KEYSTORE_PASSWORD",
        "FANDHE_ANDROID_KEY_ALIAS",
    ] {
        let mut e = aenv(true);
        e.android.signing.remove(missing);
        let p = errs(prepare(&app, &android_opts(Action::Build, true), &e));
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].contains(missing) && !p[0].contains(KEYSTORE), "{p:?}");
    }
    // debug は署名なしで通る。
    assert!(prepare(&app, &android_opts(Action::Build, false), &aenv(false)).is_ok());
}

#[test]
fn android_prerequisites_are_all_collected_one_line_each() {
    let mut app = gallery(Os::Android);
    app.cdylib = None;
    app.application_id = None;
    let mut e = env(Os::Macos, &[], "0");
    e.installed_targets = Some(BTreeSet::new());
    let p = errs(prepare(&app, &android_opts(Action::Build, true), &e));
    // cdylib・application-id・SDK・NDK・cargo-ndk・gradle・java・target 3 つ・署名。
    assert_eq!(p.len(), 11, "{p:?}");
    for want in [
        "cdylib",
        "applicationId",
        "ANDROID_HOME",
        "ANDROID_NDK_HOME",
        "cargo-ndk",
        "gradle",
        "java",
        "--release",
    ] {
        assert!(p.iter().any(|l| l.contains(want)), "{want}: {p:?}");
    }
    assert_eq!(
        p.iter().filter(|l| l.contains("rustup target add")).count(),
        3
    );
    // run は adb と端末も要る。
    let p = errs(prepare(&app, &android_opts(Action::Run, false), &e));
    assert!(p.iter().any(|l| l.starts_with("adb がありません")), "{p:?}");
    assert!(p.iter().any(|l| l.contains("端末がありません")), "{p:?}");
}

#[test]
fn android_device_selection_branches() {
    let app = gallery(Os::Android);
    let run = android_opts(Action::Run, false);
    let with = |f: &dyn Fn(&mut AndroidEnv)| {
        let mut e = aenv(false);
        f(&mut e.android);
        errs(prepare(&app, &run, &e))
    };
    let p = with(&|a| a.devices.clear());
    assert!(p.len() == 1 && p[0].contains("端末がありません"), "{p:?}");
    let p = with(&|a| a.devices = vec!["a".into(), "b".into()]);
    assert!(p.len() == 1 && p[0].contains("ANDROID_SERIAL"), "{p:?}");
    let p = with(&|a| a.serial_env = Some("zzz".into()));
    assert!(
        p.len() == 1 && p[0].contains("見つかりません") && !p[0].contains("zzz"),
        "{p:?}"
    );
    let p = with(&|a| a.device_abi = None);
    assert!(p.len() == 1 && p[0].contains("ABI"), "{p:?}");
    let p = with(&|a| a.device_abi = Some("mips".into()));
    assert!(p.len() == 1 && p[0].contains("対応していません"), "{p:?}");
    // 端末の ABI の target だけが要る。
    let mut e = aenv(false);
    e.installed_targets = Some(BTreeSet::from(["aarch64-linux-android".to_string()]));
    let p = errs(prepare(&app, &run, &e));
    assert!(
        p.len() == 1 && p[0].contains("x86_64-linux-android"),
        "{p:?}"
    );
    // rustup が無ければ target は検査しない。
    e.installed_targets = None;
    assert!(prepare(&app, &run, &e).is_ok());
    // build は端末を見ない。
    let mut e = aenv(false);
    e.android.devices.clear();
    e.commands.remove("adb");
    assert!(prepare(&app, &android_opts(Action::Build, false), &e).is_ok());
}

#[test]
fn android_errors_do_not_leak_environment_values() {
    let app = gallery(Os::Android);
    let mut e = aenv(true);
    e.android.signing.remove("FANDHE_ANDROID_KEY_ALIAS");
    e.android.devices.clear();
    e.android.serial_env = Some("serial-secret-77".into());
    e.commands.clear();
    let all = errs(prepare(&app, &android_opts(Action::Run, true), &e));
    assert!(all.len() > 3);
    assert!(
        all.iter().all(|l| !l.contains(SECRET)
            && !l.contains(KEYSTORE)
            && !l.contains("serial-secret-77")),
        "{all:?}"
    );
}

#[test]
fn android_shell_versions_match_versions_env_and_platforms_android() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let read =
        |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let versions = read("scripts/android/versions.env");
    let var = |k: &str| {
        versions
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .map(|v| v.trim_matches('"').to_string())
            .unwrap_or_else(|| panic!("{k} がありません"))
    };
    let app = gallery(Os::Android);
    let ops = prepare(&app, &android_opts(Action::Build, false), &aenv(false)).expect("通る");
    let sh = shell(&app);
    let app_gradle = written(&ops, "app/build.gradle.kts", &sh);
    let root_gradle = written(&ops, "build.gradle.kts", &sh);
    let sdk = var("PLATFORM_PACKAGE")
        .trim_start_matches("platforms;android-")
        .to_string();
    let tools = var("BUILD_TOOLS_PACKAGE")
        .trim_start_matches("build-tools;")
        .to_string();
    let min = var("ANDROID_MIN_API");
    let agp = var("AGP_VERSION");
    assert!(app_gradle.contains(&format!("compileSdk = {sdk}\n")));
    assert!(app_gradle.contains(&format!("targetSdk = {sdk}\n")));
    assert!(app_gradle.contains(&format!("buildToolsVersion = \"{tools}\"")));
    assert!(app_gradle.contains(&format!("minSdk = {min}\n")));
    assert!(root_gradle.contains(&format!("version \"{agp}\"")));
    assert!(spawns(&ops)[0]
        .args
        .windows(2)
        .any(|w| w == ["--platform", min.as_str()]));
    // platforms/android の殻と同じ値。
    let pa = read("platforms/android/app/build.gradle.kts");
    for want in [
        format!("compileSdk = {sdk}\n"),
        format!("targetSdk = {sdk}\n"),
        format!("buildToolsVersion = \"{tools}\""),
        format!("minSdk = {min}\n"),
        "applicationId = \"com.fandhe.androidapp\"".to_string(),
        "JavaVersion.VERSION_17".to_string(),
    ] {
        assert!(pa.contains(&want), "platforms/android: {want}");
        assert!(app_gradle.contains(&want), "殻: {want}");
    }
    assert!(read("platforms/android/build.gradle.kts").contains(&format!("version \"{agp}\"")));
    // abiFilters と同じ 3 ABI。
    assert!(pa.contains("arm64-v8a,armeabi-v7a,x86_64"));
}

#[test]
fn parses_adb_devices_output() {
    let out =
        "List of devices attached\nemulator-5554\tdevice\nR58\tunauthorized\nabc\toffline\n\n";
    assert_eq!(parse_adb_devices(out), ["emulator-5554"]);
    assert!(parse_adb_devices("List of devices attached\n\n").is_empty());
    // デーモン起動の案内行が前に付いても拾う。
    let out = "* daemon started successfully\nList of devices attached\n1234\tdevice usb:1\n";
    assert_eq!(parse_adb_devices(out), ["1234"]);
}

/// bin も iOS の設定も持たない Android 専用アプリ。
fn android_only(tag: &str, android_toml: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("run-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("作成");
    std::fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"droid-app\"\nversion = \"0.3.0\"\nedition = \"2021\"\n\n\
             [lib]\ncrate-type = [\"cdylib\"]\n\n[workspace]\n\n{android_toml}"
        ),
    )
    .expect("書き込み");
    std::fs::write(root.join("src/lib.rs"), "").expect("書き込み");
    root.join("Cargo.toml")
}

#[test]
fn android_generation_is_limited_to_android() {
    let m = android_only(
        "droid",
        "[package.metadata.fandhe-app.android]\napplication-id = \"com.example.droid\"\n",
    );
    // 通常の gen は iOS の必須条件（bin・bundle-id）で止まるが、run / build は Android の分だけ生成する。
    assert!(cargo_fandhe::generate(Some(&m)).is_err());
    let app = load(&m, Os::Android).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(app.application_id.as_deref(), Some("com.example.droid"));
    let files = app.android_gen.as_ref().expect("生成する");
    assert!(files.contains_key("android/fandhe.gradle"));
    assert!(files["android/java/com/fandhe/platform/FandheLibrary.java"].contains("\"droid_app\""));
    assert!(files
        .keys()
        .all(|k| !k.starts_with("ios/") && !k.starts_with("macos/")));
    let mut e = aenv(false);
    e.installed_targets = None;
    let ops = prepare(&app, &android_opts(Action::Build, false), &e).expect("通る");
    assert!(written(&ops, "app/build.gradle.kts", &shell(&app)).contains("versionName = \"0.3.0\""));
}

#[test]
fn android_application_id_is_optional_in_gen_but_validated_and_required_to_build() {
    let m = android_only("droid-noid", "");
    let app = load(&m, Os::Android).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(app.application_id, None);
    let p = errs(prepare(
        &app,
        &android_opts(Action::Build, false),
        &aenv(false),
    ));
    assert!(p.len() == 1 && p[0].contains("application-id"), "{p:?}");
    for bad in [
        "com",
        "com.1x",
        "com.ex-ample.app",
        "com..app",
        "com.a\\\"b",
        "com.a$b",
    ] {
        let m = android_only(
            "droid-bad",
            &format!("[package.metadata.fandhe-app.android]\napplication-id = \"{bad}\"\n"),
        );
        let e = load(&m, Os::Android).expect_err(bad);
        assert!(e[0].contains("application-id"), "{bad}: {e:?}");
    }
}

#[test]
fn android_keystore_file_must_exist() {
    let app = gallery(Os::Android);
    let mut e = aenv(true);
    e.android.keystore_file = false;
    let p = errs(prepare(&app, &android_opts(Action::Build, true), &e));
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(
        p[0].contains("FANDHE_ANDROID_KEYSTORE") && !p[0].contains(KEYSTORE),
        "{p:?}"
    );
    // debug では見ない。
    assert!(prepare(&app, &android_opts(Action::Build, false), &e).is_ok());
}

#[test]
fn android_uses_gradle_bat_on_windows_hosts() {
    let app = gallery(Os::Android);
    let opts = android_opts(Action::Build, false);
    let mut e = aenv(false);
    e.host = Some(Os::Windows);
    let p = errs(prepare(&app, &opts, &e));
    assert!(
        p.len() == 1 && p[0].starts_with("gradle.bat がありません"),
        "{p:?}"
    );
    e.commands.insert("gradle.bat".into());
    e.commands.remove("gradle");
    let ops = prepare(&app, &opts, &e).expect("通る");
    assert_eq!(spawns(&ops)[1].program, "gradle.bat");
    // Windows 以外は gradle。
    let ops = prepare(&app, &opts, &aenv(false)).expect("通る");
    assert_eq!(spawns(&ops)[1].program, "gradle");
}

#[test]
fn android_requires_the_gen_output() {
    let mut app = gallery(Os::Android);
    app.android_gen = None;
    let p = errs(prepare(
        &app,
        &android_opts(Action::Build, false),
        &aenv(false),
    ));
    assert!(p.len() == 1 && p[0].contains("gen 出力"), "{p:?}");
}

#[test]
fn android_app_name_is_xml_escaped() {
    let mut app = gallery(Os::Android);
    app.name = "a&b<c>\"d'".into();
    let ops = prepare(&app, &android_opts(Action::Build, false), &aenv(false)).expect("通る");
    let x = written(&ops, "app/src/main/res/values/strings.xml", &shell(&app));
    assert!(x.contains("a&amp;b&lt;c&gt;&quot;d&apos;"), "{x}");
}

#[test]
fn step_env_reaches_the_child_process() {
    #[cfg(unix)]
    {
        let sh = |want: &str| {
            vec![Op::Spawn(Step {
                stage: "sh",
                program: "sh".into(),
                args: vec!["-c".into(), "test \"$FANDHE_TEST_X\" = v".into()],
                env: vec![("FANDHE_TEST_X".into(), want.into())],
            })]
        };
        assert!(execute(&sh("v")).is_ok());
        assert!(execute(&sh("w")).is_err());
    }
}

#[test]
fn detect_env_reads_rust_targets_for_web_and_android_only() {
    use cargo_fandhe::run::detect_env;
    if std::process::Command::new("rustup")
        .arg("--version")
        .output()
        .is_err()
    {
        return; // rustup が無い環境では観測されない（検査は飛ばす仕様）
    }
    for os in [Os::Web, Os::Android] {
        let e = detect_env(os, Action::Build);
        assert!(e.installed_targets.is_some(), "{os:?}");
    }
    assert!(detect_env(Os::Macos, Action::Build)
        .installed_targets
        .is_none());
}

#[test]
fn android_run_leaves_the_apps_own_gen_alone() {
    // 印の無い利用者の gen/ があっても、run / build は触れず、出力は殻の下の gen/ へ出る。
    let m = android_only(
        "droid-ownedgen",
        "[package.metadata.fandhe-app.android]\napplication-id = \"com.example.droid\"\n",
    );
    let own = m.parent().expect("dir").join("gen");
    std::fs::create_dir_all(&own).expect("作成");
    std::fs::write(own.join("keep.txt"), "mine").expect("書き込み");
    let app = load(&m, Os::Android).unwrap_or_else(|e| panic!("{e:?}"));
    let mut e = aenv(false);
    e.installed_targets = None;
    let ops = prepare(&app, &android_opts(Action::Build, false), &e).expect("通る");
    let sh = shell(&app);
    // gen・殻の書き出しまでを実行する（cargo-ndk 以降は外す）。
    let upto = ops
        .iter()
        .position(|o| matches!(o, Op::Spawn(_)))
        .expect("spawn");
    execute(&ops[..upto]).expect("通る");
    assert_eq!(
        std::fs::read_to_string(own.join("keep.txt")).expect("読む"),
        "mine"
    );
    assert_eq!(std::fs::read_dir(&own).expect("読む").count(), 1);
    let gradle = std::fs::read_to_string(sh.join("gen/android/fandhe.gradle")).expect("gen の出力");
    assert!(
        gradle.contains(&sh.join("gen").display().to_string()),
        "{gradle}"
    );
    assert!(sh
        .join("app/src/main/java/com/fandhe/platform/MainActivity.java")
        .is_file());
    // 実行し直しても通る（印のある殻の gen/ は置き換わる）。
    execute(&ops[..upto]).expect("2 回目も通る");
}

#[test]
fn write_refuses_paths_outside_or_through_symlinks() {
    let t = scratch("write");
    let within = t.join("target");
    std::fs::create_dir_all(&within).expect("作成");
    let w = |path: PathBuf| {
        execute(&[Op::Write {
            path,
            within: within.clone(),
            body: "x".into(),
        }])
    };
    let e = w(t.join("other/a.txt")).expect_err("配下の外は拒否");
    assert!(e.contains("配下ではない"), "{e}");
    assert!(!t.join("other").exists());
    #[cfg(unix)]
    {
        let victim = t.join("victim");
        std::fs::create_dir_all(&victim).expect("作成");
        std::os::unix::fs::symlink(&victim, within.join("fandhe")).expect("link");
        let e = w(within.join("fandhe/a.txt")).expect_err("経路の symlink は拒否");
        assert!(e.contains("symlink"), "{e}");
        assert!(!victim.join("a.txt").exists());
        // 書き込み先のファイル自身が symlink でも拒否する。
        let target = t.join("victim/orig.txt");
        std::fs::write(&target, "orig").expect("書き込み");
        std::fs::create_dir_all(within.join("ok")).expect("作成");
        std::os::unix::fs::symlink(&target, within.join("ok/f.txt")).expect("link");
        assert!(w(within.join("ok/f.txt")).is_err());
        assert_eq!(std::fs::read_to_string(&target).expect("読む"), "orig");
    }
    assert!(w(within.join("ok2/b.txt")).is_ok());
}

/// `application-attributes` は殻の `<application>` の既定をキー単位で置き換え、足す。gen の overlay には出ない（Issue #1202）。
#[test]
fn android_application_attributes_override_shell_defaults_and_stay_out_of_overlay() {
    let m = android_only(
        "droid-attrs",
        "[package.metadata.fandhe-app.android]\napplication-id = \"com.example.droid\"\n\
         application-attributes = { localeConfig = \"@xml/locale_config\", icon = \"@mipmap/ic_launcher\", label = \"A&B\" }\n\
         res = [\"android/res\"]\n",
    );
    std::fs::create_dir_all(m.parent().unwrap().join("android/res")).expect("作成");
    let app = load(&m, Os::Android).unwrap_or_else(|e| panic!("{e:?}"));
    let ops = prepare(&app, &android_opts(Action::Build, false), &aenv(false)).expect("通る");
    let man = written(&ops, "app/src/main/AndroidManifest.xml", &shell(&app));
    assert!(
        man.contains("android:localeConfig=\"@xml/locale_config\""),
        "{man}"
    );
    assert!(
        man.contains("android:icon=\"@mipmap/ic_launcher\""),
        "{man}"
    );
    assert!(!man.contains("sym_def_app_icon"), "{man}");
    assert!(man.contains("android:label=\"A&amp;B\""), "{man}");
    // 置き換えなかった既定は残る。
    assert!(
        man.contains("android:allowBackup=\"false\"")
            && man.contains("android:theme=\"@style/FandheAppTheme\""),
        "{man}"
    );
    // gen の overlay には出ない。アプリの res は Gradle の srcDirs に入る。
    let Op::Gen { files, .. } = &ops[0] else {
        panic!("先頭は Gen のはず")
    };
    assert!(!files["android/AndroidManifest.xml"].contains("localeConfig"));
    assert!(files["android/fandhe.gradle"].contains("/android/res"));
}

/// 属性を書かなければ殻の Manifest は従来のまま。予約キー（overlay・ビルド種別の持ち分）は拒否する。
#[test]
fn android_application_attributes_default_and_reserved_keys() {
    let m = android_only(
        "droid-attrs-none",
        "[package.metadata.fandhe-app.android]\napplication-id = \"com.example.droid\"\n",
    );
    let app = load(&m, Os::Android).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(app.application_attributes.is_empty());
    let ops = prepare(&app, &android_opts(Action::Build, false), &aenv(false)).expect("通る");
    let man = written(&ops, "app/src/main/AndroidManifest.xml", &shell(&app));
    assert!(
        man.contains("android:icon=\"@android:drawable/sym_def_app_icon\""),
        "{man}"
    );
    for (k, hint) in [
        ("dataExtractionRules", "backup-exclusions"),
        ("fullBackupContent", "backup-exclusions"),
        ("debuggable", "ビルド種別"),
    ] {
        let m = android_only(
            "droid-attrs-bad",
            &format!(
                "[package.metadata.fandhe-app.android]\napplication-id = \"com.example.droid\"\n\
                 application-attributes = {{ {k} = \"x\" }}\n"
            ),
        );
        let e = load(&m, Os::Android).expect_err(k).join("\n");
        assert!(e.contains(k) && e.contains(hint), "{k}: {e}");
    }
}
