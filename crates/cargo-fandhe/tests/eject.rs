//! `cargo fandhe gen --eject` と `eject/` の差し替えのテスト（Issue #1202）。
//! 実際の cargo を通す単独アプリ（`[workspace]` を自分で持つ）を一時ディレクトリに作って確かめる。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cargo_fandhe::run::{
    execute, load_app, prepare, Action, AndroidEnv, App, Env, Op, Options, Os,
};
use cargo_fandhe::{eject, generate, write};

/// cdylib・bin・iOS の bundle-id・Android の application-id を持つ単独アプリ。`extra` は追加の TOML。
fn app(tag: &str, extra: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("eject-{tag}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [lib]\ncrate-type = [\"cdylib\", \"rlib\"]\n\n[workspace]\n\n\
             [package.metadata.fandhe-app.ios]\nbundle-id = \"com.example.app\"\n\
             [package.metadata.fandhe-app.android]\napplication-id = \"com.example.app\"\n{extra}"
        ),
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    root.join("Cargo.toml")
}

fn dir(m: &Path) -> &Path {
    m.parent().unwrap()
}

fn rels(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn err(r: Result<impl std::fmt::Debug, Vec<String>>) -> String {
    r.expect_err("拒否されるはず").join("\n")
}

#[test]
fn eject_writes_with_replaced_comment_and_never_overwrites() {
    let m = app("write", "");
    let w = eject(
        Some(&m),
        &rels(&["ios/Info.plist", "android/AndroidManifest.xml"]),
    )
    .unwrap();
    assert_eq!(w.len(), 2);
    let plist = fs::read_to_string(dir(&m).join("eject/ios/Info.plist")).unwrap();
    assert!(
        plist.contains("<!-- cargo fandhe gen の eject で書き出した。`eject/` のこのファイルを消せば生成へ戻る -->"),
        "{plist}"
    );
    assert!(!plist.contains("編集しない"), "{plist}");
    let generated = generate(Some(&m));
    // 書き出した内容どおりに生成が使う（先頭コメントも eject のもの）。
    let g = generated.unwrap();
    assert_eq!(g.files["ios/Info.plist"], plist);
    assert!(
        g.notes.iter().any(|n| n == "eject 済み: ios/Info.plist"),
        "{:?}",
        g.notes
    );
    assert!(g
        .notes
        .iter()
        .any(|n| n == "eject 済み: android/AndroidManifest.xml"));
    // 既にあれば上書きしない。
    fs::write(dir(&m).join("eject/ios/Info.plist"), "mine").unwrap();
    let e = err(eject(Some(&m), &rels(&["ios/Info.plist"])));
    assert!(e.contains("既にあります"), "{e}");
    assert_eq!(
        fs::read_to_string(dir(&m).join("eject/ios/Info.plist")).unwrap(),
        "mine"
    );
}

#[test]
fn eject_rejects_unsupported_paths_with_the_list_and_writes_nothing() {
    let m = app("unsupported", "");
    for bad in [
        "android/fandhe.gradle",
        "android/java/com/fandhe/platform/FandheLibrary.java",
        "ios/nothing.plist",
        "web/manifest.webmanifest",
        "../Cargo.toml",
        "macos/Info.plist", // macOS は bundle-id も機能も無く生成対象にない
    ] {
        let e = err(eject(Some(&m), &rels(&["ios/Info.plist", bad])));
        assert!(
            e.contains("書き出せるパス")
                && e.contains("ios/Info.plist")
                && e.contains("android/AndroidManifest.xml"),
            "{bad}: {e}"
        );
    }
    assert!(!dir(&m).join("eject").exists());
}

/// 受け入れ条件: 書き出して編集したあとに TOML を変えても、そのファイルは変わらない。消せば生成へ戻る。
#[test]
fn edited_eject_file_survives_toml_changes_and_deleting_restores_generation() {
    let m = app("survive", "");
    eject(Some(&m), &rels(&["ios/Info.plist"])).unwrap();
    let p = dir(&m).join("eject/ios/Info.plist");
    let edited = fs::read_to_string(&p)
        .unwrap()
        .replace("</dict>", "<key>Mine</key><string>1</string></dict>");
    fs::write(&p, &edited).unwrap();
    // TOML を変えても（追記を足しても）eject したファイルが勝つ。
    let toml = fs::read_to_string(&m).unwrap();
    fs::write(
        &m,
        format!("{toml}\n[package.metadata.fandhe-app.ios.info-plist]\nTomlKey = \"x\"\n"),
    )
    .unwrap();
    let g = generate(Some(&m)).unwrap();
    assert_eq!(g.files["ios/Info.plist"], edited);
    assert!(!g.files["ios/Info.plist"].contains("TomlKey"));
    // gen の書き出し（ディスク）も同じ内容。
    write(&g.gen_dir, &g.files).unwrap();
    assert_eq!(
        fs::read_to_string(dir(&m).join("gen/ios/Info.plist")).unwrap(),
        edited
    );
    // 消すと生成へ戻る。
    fs::remove_file(&p).unwrap();
    let g = generate(Some(&m)).unwrap();
    assert!(
        g.files["ios/Info.plist"].contains("TomlKey")
            && !g.files["ios/Info.plist"].contains("Mine")
    );
    assert!(g.files["ios/Info.plist"].contains("編集しない"));
    assert!(g.notes.iter().all(|n| !n.starts_with("eject 済み")));
}

#[test]
fn eject_dir_rejects_stray_files_symlinks_and_non_utf8() {
    let m = app("stray", "");
    let d = dir(&m).join("eject");
    // 綴り違い・対象外のパス。
    for (rel, body) in [
        ("ios/info.plist", &b"x"[..]),
        ("android/fandhe.gradle", b"x"),
        ("README.md", b"x"),
    ] {
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join(rel).parent().unwrap()).unwrap();
        fs::write(d.join(rel), body).unwrap();
        let e = err(generate(Some(&m)));
        assert!(
            e.contains(&format!("eject/{rel}")) && e.contains("書き出せるパス"),
            "{e}"
        );
    }
    // 非 UTF-8。
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(d.join("ios")).unwrap();
    fs::write(d.join("ios/Info.plist"), [0xff, 0xfe]).unwrap();
    assert!(err(generate(Some(&m))).contains("UTF-8"));
    #[cfg(unix)]
    {
        // ファイルの symlink。
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("ios")).unwrap();
        fs::write(dir(&m).join("real.plist"), "x").unwrap();
        std::os::unix::fs::symlink(dir(&m).join("real.plist"), d.join("ios/Info.plist")).unwrap();
        assert!(err(generate(Some(&m))).contains("symlink"));
        // eject/ 自身の symlink。
        let _ = fs::remove_dir_all(&d);
        let other = dir(&m).join("other");
        fs::create_dir_all(&other).unwrap();
        std::os::unix::fs::symlink(&other, &d).unwrap();
        assert!(err(generate(Some(&m))).contains("symlink"));
        // 書き出し先の途中が symlink なら --eject も拒否し、リンク先へ書かない。
        fs::remove_file(&d).unwrap();
        fs::create_dir_all(&d).unwrap();
        std::os::unix::fs::symlink(&other, d.join("ios")).unwrap();
        assert!(err(eject(Some(&m), &rels(&["ios/Info.plist"]))).contains("symlink"));
        assert!(!other.join("Info.plist").exists());
    }
}

#[test]
fn android_load_app_applies_eject_and_ignores_other_os_dirs() {
    let m = app("android", "");
    let load = || {
        load_app(
            &Options {
                action: Action::Build,
                os: Some(Os::Android),
                release: false,
                manifest: Some(m.clone()),
            },
            Os::Android,
        )
    };
    let d = dir(&m).join("eject");
    fs::create_dir_all(d.join("android")).unwrap();
    fs::write(d.join("android/AndroidManifest.xml"), "<manifest/>\n").unwrap();
    // iOS 側は綴り違いでも、今回の対象 OS ではないので止めない。
    fs::create_dir_all(d.join("ios")).unwrap();
    fs::write(d.join("ios/typo.plist"), "x").unwrap();
    let app = load().unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(
        app.android_gen.unwrap()["android/AndroidManifest.xml"],
        "<manifest/>\n"
    );
    // 対象 OS（android）の綴り違いは止める。
    fs::write(d.join("android/Manifest.xml"), "x").unwrap();
    assert!(load().is_err());
}

#[test]
fn cli_exit_codes_and_end_to_end() {
    let m = app("cli", "");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_cargo-fandhe"))
            .args(args)
            .arg("--manifest-path")
            .arg(&m)
            .output()
            .unwrap()
    };
    let code = |args: &[&str]| run(args).status.code();
    assert_eq!(code(&["gen", "--eject"]), Some(2));
    assert_eq!(
        code(&["gen", "--os", "web", "--eject", "ios/Info.plist"]),
        Some(2)
    );
    assert_eq!(code(&["check", "--eject", "ios/Info.plist"]), Some(2));
    assert_eq!(code(&["gen", "--eject", "--os", "web"]), Some(2));
    assert_eq!(code(&["gen", "--bogus"]), Some(2));
    // 対象外のパスは 1（一覧つき）。
    let o = run(&["gen", "--eject", "android/fandhe.gradle"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("書き出せるパス"));
    // 書き出して、gen をやり直しても残る。
    assert_eq!(
        code(&[
            "gen",
            "--eject",
            "ios/Info.plist",
            "--eject",
            "ios/App.entitlements"
        ]),
        Some(1)
    ); // entitlements は無い
    assert_eq!(code(&["gen", "--eject", "ios/Info.plist"]), Some(0));
    let p = dir(&m).join("eject/ios/Info.plist");
    fs::write(&p, "<plist/>\n").unwrap();
    assert_eq!(code(&["gen"]), Some(0));
    assert_eq!(
        fs::read_to_string(dir(&m).join("gen/ios/Info.plist")).unwrap(),
        "<plist/>\n"
    );
    fs::remove_file(&p).unwrap();
    assert_eq!(code(&["gen"]), Some(0));
    assert!(fs::read_to_string(dir(&m).join("gen/ios/Info.plist"))
        .unwrap()
        .contains("CFBundleName"));
}

#[test]
fn conflict_errors_point_to_eject() {
    let m = app(
        "hint",
        "[package.metadata.fandhe-app.ios.info-plist]\nCFBundleName = \"other\"\n",
    );
    let e = err(generate(Some(&m)));
    assert!(e.contains("--eject ios/Info.plist"), "{e}");
}

/// iOS の bundle-id が無いアプリでも、`android/` だけの eject は Android の分だけ生成して通る。
#[test]
fn android_only_eject_works_without_ios_bundle_id() {
    let m = app("android-only", "");
    let toml = fs::read_to_string(&m)
        .unwrap()
        .replace("bundle-id = \"com.example.app\"", "");
    fs::write(&m, toml).unwrap();
    assert!(generate(Some(&m)).is_err());
    eject(Some(&m), &rels(&["android/AndroidManifest.xml"])).unwrap();
    assert!(dir(&m).join("eject/android/AndroidManifest.xml").is_file());
    // ios/ を混ぜると全 OS を生成するので、従来どおり bundle-id で止まる。
    assert!(eject(
        Some(&m),
        &rels(&["android/AndroidManifest.xml", "ios/Info.plist"])
    )
    .is_err());
}

/// `.` で始まる名前（.DS_Store・エディタの退避・ロックの symlink）は種別を見ずに飛ばす。
#[test]
fn eject_dir_ignores_dot_files() {
    let m = app("dotfiles", "");
    let d = dir(&m).join("eject/ios");
    fs::create_dir_all(&d).unwrap();
    fs::write(dir(&m).join("eject/.DS_Store"), "x").unwrap();
    fs::write(d.join(".Info.plist.swp"), [0xff]).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("host.1234", d.join(".#Info.plist")).unwrap();
    fs::create_dir_all(dir(&m).join("eject/.git")).unwrap();
    fs::write(dir(&m).join("eject/.git/x"), "x").unwrap();
    let g = generate(Some(&m)).unwrap();
    assert!(g.notes.iter().all(|n| !n.starts_with("eject 済み")));
}

#[test]
fn eject_dir_depth_is_limited() {
    let m = app("deep", "");
    let d = dir(&m).join("eject/a/b/c/d/e/f/g/h/i");
    fs::create_dir_all(&d).unwrap();
    assert!(err(generate(Some(&m))).contains("深すぎます"));
}

/// cdylib の無いアプリは Android を生成しないので、`eject/android/` はエラー。理由（cdylib）を添える。
#[test]
fn eject_android_without_cdylib_is_an_error_with_the_reason() {
    let m = app("nocdylib", "");
    let toml = fs::read_to_string(&m).unwrap().replace("\"cdylib\", ", "");
    fs::write(&m, toml).unwrap();
    let d = dir(&m).join("eject/android");
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("AndroidManifest.xml"), "<manifest/>").unwrap();
    let e = err(generate(Some(&m)));
    assert!(
        e.contains("eject/android/AndroidManifest.xml") && e.contains("cdylib"),
        "{e}"
    );
    // `--eject android/...` だけを指定したときも、一覧が空になる理由を添える。
    let e = err(eject(Some(&m), &rels(&["android/AndroidManifest.xml"])));
    assert!(
        e.contains("書き出せるパス: なし") && e.contains("cdylib"),
        "{e}"
    );
}

/// eject 済みでも検証は走る。eject のあとに食い違う追記を足すと失敗する。
#[test]
fn validation_still_runs_for_ejected_files() {
    let m = app("validate", "");
    eject(Some(&m), &rels(&["ios/Info.plist"])).unwrap();
    generate(Some(&m)).unwrap();
    let toml = fs::read_to_string(&m).unwrap();
    fs::write(
        &m,
        format!("{toml}\n[package.metadata.fandhe-app.ios.info-plist]\nCFBundleName = \"other\"\n"),
    )
    .unwrap();
    assert!(err(generate(Some(&m))).contains("CFBundleName"));
}

// ---- 殻の重ね置き（`eject/android/shell/`、run・build の Android）----

fn build_opts(m: &Path) -> Options {
    Options {
        action: Action::Build,
        os: Some(Os::Android),
        release: false,
        manifest: Some(m.to_path_buf()),
    }
}

fn load_android(m: &Path) -> Result<App, Vec<String>> {
    load_app(&build_opts(m), Os::Android)
}

fn android_env() -> Env {
    Env {
        host: Some(Os::Macos),
        commands: ["cargo-ndk", "gradle", "java"].map(String::from).into(),
        installed_targets: None,
        wasm_bindgen_version: None,
        android: AndroidEnv {
            sdk: true,
            ndk: true,
            ..AndroidEnv::default()
        },
        cargo: "cargo".into(),
    }
}

/// 殻の置き場（`<target_dir>/fandhe/<package>/android`）。
fn shell_root(app: &App) -> PathBuf {
    app.target_dir
        .join("fandhe")
        .join(&app.name)
        .join("android")
}

/// load → prepare → cargo-ndk の前まで実行する。手順と殻の置き場を返す。
fn run_until_ndk(m: &Path) -> (Vec<Op>, PathBuf) {
    let app = load_android(m).unwrap_or_else(|e| panic!("{e:?}"));
    let ops = prepare(&app, &build_opts(m), &android_env()).unwrap_or_else(|e| panic!("{e:?}"));
    let upto = ops.iter().position(|o| matches!(o, Op::Spawn(_))).unwrap();
    execute(&ops[..upto]).unwrap_or_else(|e| panic!("{e}"));
    (ops, shell_root(&app))
}

fn put(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

#[test]
fn shell_overlay_replaces_same_path_and_adds_new_files_after_the_shell() {
    let m = app("shell-basic", "");
    let ov = dir(&m).join("eject/android/shell");
    put(&ov, "app/build.gradle.kts", b"// mine\nversionCode = 2\n");
    put(&ov, "app/google-services.json", b"{}");
    // png などのバイナリ（UTF-8 でない）もそのまま写す。
    let png = [0x89, b'P', b'N', b'G', 0xff, 0x00];
    put(&ov, "app/src/main/res/mipmap-hdpi/ic_launcher.png", &png);
    let (ops, sh) = run_until_ndk(&m);
    let read = |rel: &str| fs::read(sh.join(rel)).unwrap();
    assert_eq!(read("app/build.gradle.kts"), b"// mine\nversionCode = 2\n");
    assert_eq!(read("app/google-services.json"), b"{}");
    assert_eq!(read("app/src/main/res/mipmap-hdpi/ic_launcher.png"), png);
    // 置かなかった殻のファイルは従来どおり。
    assert!(String::from_utf8(read("settings.gradle.kts"))
        .unwrap()
        .contains("include(\":app\")"));
    // 殻の Write がすべて済んでから重ね、cargo-ndk の直前に置く。
    let overlay = ops
        .iter()
        .position(|o| matches!(o, Op::Overlay { .. }))
        .unwrap();
    let last_write = ops
        .iter()
        .rposition(|o| matches!(o, Op::Write { .. }))
        .unwrap();
    let ndk = ops.iter().position(|o| matches!(o, Op::Spawn(_))).unwrap();
    assert!(last_write < overlay && overlay + 1 == ndk);
    // 重ね置きが無いアプリは Overlay を出さない。
    let plain = app("shell-none", "");
    let a = load_android(&plain).unwrap();
    assert!(a.shell_overlay.is_empty());
    let ops = prepare(&a, &build_opts(&plain), &android_env()).unwrap();
    assert!(!ops.iter().any(|o| matches!(o, Op::Overlay { .. })));
}

#[test]
fn shell_overlay_files_removed_from_eject_disappear_and_gradle_caches_stay() {
    let m = app("shell-prune", "");
    let ov = dir(&m).join("eject/android/shell");
    put(&ov, "app/build.gradle.kts", b"// mine\nversionCode = 2\n");
    put(&ov, "app/google-services.json", b"{}");
    put(&ov, "extra/keep.txt", b"k");
    let (_, sh) = run_until_ndk(&m);
    assert!(sh.join("app/google-services.json").is_file());
    // Gradle が作るもの（と、利用者が重ねていない手元のファイル）。
    for rel in [
        "build/o.txt",
        "app/build/o.txt",
        ".gradle/c.bin",
        "app/mine.txt",
    ] {
        put(&sh, rel, b"x");
    }
    fs::remove_file(ov.join("app/google-services.json")).unwrap();
    fs::remove_file(ov.join("app/build.gradle.kts")).unwrap();
    let (_, sh) = run_until_ndk(&m);
    assert!(!sh.join("app/google-services.json").exists());
    // 殻の同名ファイルは、重ね置きを外すと殻の版へ戻る。
    let g = fs::read_to_string(sh.join("app/build.gradle.kts")).unwrap();
    assert!(g.contains("versionCode = 1"), "{g}");
    assert!(sh.join("extra/keep.txt").is_file());
    for rel in [
        "build/o.txt",
        "app/build/o.txt",
        ".gradle/c.bin",
        "app/mine.txt",
    ] {
        assert!(sh.join(rel).is_file(), "{rel} は消さない");
    }
    // 全部外すと控えも消え、重ねたファイルは残らない。
    fs::remove_dir_all(dir(&m).join("eject")).unwrap();
    let (_, sh) = run_until_ndk(&m);
    assert!(!sh.join("extra/keep.txt").exists() && !sh.join(".fandhe-overlay").exists());
    assert!(sh.join("build/o.txt").is_file());
}

#[test]
fn shell_overlay_handles_a_directory_turning_into_a_file_and_back() {
    let m = app("shell-dir-to-file", "");
    let ov = dir(&m).join("eject/android/shell");
    put(&ov, "extra/a.txt", b"a");
    let (_, sh) = run_until_ndk(&m);
    assert!(sh.join("extra/a.txt").is_file());
    // ディレクトリだった `extra` を通常ファイルに変える。前回写した空の `extra/` が残っていると写せない。
    fs::remove_dir_all(ov.join("extra")).unwrap();
    put(&ov, "extra", b"file");
    let (_, sh) = run_until_ndk(&m);
    assert_eq!(fs::read(sh.join("extra")).unwrap(), b"file");
    // 戻しても通る。
    fs::remove_file(ov.join("extra")).unwrap();
    put(&ov, "extra/a.txt", b"b");
    let (_, sh) = run_until_ndk(&m);
    assert_eq!(fs::read(sh.join("extra/a.txt")).unwrap(), b"b");
}

#[test]
fn shell_overlay_record_naming_a_directory_does_not_get_stuck() {
    // 写し先が既存のディレクトリで写せなかった等で、控えにディレクトリの名前が残った状態。
    let m = app("shell-record-dir", "");
    let ov = dir(&m).join("eject/android/shell");
    put(&ov, "app/x.txt", b"x");
    let (_, sh) = run_until_ndk(&m);
    fs::write(sh.join(".fandhe-overlay"), r#"["app"]"#).unwrap();
    put(&sh, "app/build/o.txt", b"x");
    let (_, sh) = run_until_ndk(&m);
    // ディレクトリは消さずに飛ばし、Gradle の出力も残る。
    assert!(sh.join("app/build/o.txt").is_file() && sh.join("app/x.txt").is_file());
}

#[test]
fn shell_overlay_dir_input_over_a_shell_file_recovers_after_removal() {
    // 殻の通常ファイルと同名のディレクトリを入力に置く置き間違い。写せずに止まるが、入力を外せば次は通る。
    let m = app("shell-dir-over-file", "");
    let ov = dir(&m).join("eject/android/shell");
    put(&ov, "settings.gradle.kts/x.txt", b"x");
    let app_ = load_android(&m).unwrap_or_else(|e| panic!("{e:?}"));
    let ops = prepare(&app_, &build_opts(&m), &android_env()).unwrap_or_else(|e| panic!("{e:?}"));
    let upto = ops.iter().position(|o| matches!(o, Op::Spawn(_))).unwrap();
    assert!(execute(&ops[..upto]).is_err());
    fs::remove_dir_all(dir(&m).join("eject")).unwrap();
    let (_, sh) = run_until_ndk(&m);
    assert!(sh.join("settings.gradle.kts").is_file() && !sh.join(".fandhe-overlay").exists());
}

#[test]
fn shell_overlay_missing_entry_still_cleans_its_empty_parent() {
    // 写す途中で失敗し、控えにはあるがファイルが無く、空の親だけ残った状態。
    let m = app("shell-missing-entry", "");
    let ov = dir(&m).join("eject/android/shell");
    put(&ov, "keep.txt", b"k");
    let (_, sh) = run_until_ndk(&m);
    fs::create_dir_all(sh.join("extra")).unwrap();
    fs::write(sh.join(".fandhe-overlay"), r#"["keep.txt","extra/a.txt"]"#).unwrap();
    put(&ov, "extra", b"file");
    let (_, sh) = run_until_ndk(&m);
    assert_eq!(fs::read(sh.join("extra")).unwrap(), b"file");
}

#[test]
fn shell_overlay_rejects_gen_and_special_files_with_guidance() {
    let m = app("shell-gen", "");
    let ov = dir(&m).join("eject/android/shell");
    put(&ov, "gen/android/fandhe.gradle", b"x");
    let e = err(load_android(&m));
    assert!(
        e.contains("eject/android/shell/gen/android/fandhe.gradle")
            && e.contains("gen/ は置けません")
            && e.contains("--eject android/"),
        "{e}"
    );
    // 大文字小文字だけ違う `Gen/` も拒否する（大文字小文字を区別しないファイルシステムでは gen と同じ場所になる）。
    fs::remove_dir_all(dir(&m).join("eject")).unwrap();
    put(&ov, "Gen/android/x.txt", b"x");
    let e = err(load_android(&m));
    assert!(
        e.contains("Gen/android/x.txt") && e.contains("gen/ は置けません"),
        "{e}"
    );
    // `--eject android/shell/...` には、target の殻からコピーする案内を添える。
    let e = err(eject(
        Some(&m),
        &rels(&["android/shell/app/build.gradle.kts"]),
    ));
    assert!(
        e.contains("コピー") && e.contains("eject/android/shell/"),
        "{e}"
    );
}

#[cfg(unix)]
#[test]
fn shell_overlay_rejects_symlinks() {
    let m = app("shell-symlink", "");
    let ov = dir(&m).join("eject/android/shell");
    let real = dir(&m).join("real");
    put(&real, "a.txt", b"x");
    // ファイル。
    put(&ov, "app/ok.txt", b"ok");
    std::os::unix::fs::symlink(real.join("a.txt"), ov.join("app/link.txt")).unwrap();
    assert!(err(load_android(&m)).contains("symlink"));
    // 途中のディレクトリ。
    fs::remove_file(ov.join("app/link.txt")).unwrap();
    std::os::unix::fs::symlink(&real, ov.join("linked")).unwrap();
    assert!(err(load_android(&m)).contains("symlink"));
    // 置き場そのもの・その親。
    fs::remove_dir_all(dir(&m).join("eject")).unwrap();
    fs::create_dir_all(dir(&m).join("eject/android")).unwrap();
    std::os::unix::fs::symlink(&real, ov).unwrap();
    assert!(err(load_android(&m)).contains("symlink"));
    let ej = dir(&m).join("eject");
    fs::remove_dir_all(&ej).unwrap();
    std::os::unix::fs::symlink(&real, &ej).unwrap();
    assert!(err(load_android(&m)).contains("symlink"));
}

#[test]
fn shell_overlay_ignores_dot_names_and_limits_depth() {
    let m = app("shell-dot", "");
    let ov = dir(&m).join("eject/android/shell");
    put(&ov, ".DS_Store", b"x");
    put(&ov, "app/.build.gradle.kts.swp", &[0xff]);
    put(&ov, ".git/x", b"x");
    #[cfg(unix)]
    std::os::unix::fs::symlink("host.1234", ov.join("app/.#lock")).unwrap();
    assert!(load_android(&m).unwrap().shell_overlay.is_empty());
    // Java のパッケージが入る深さは通し、上限を超えたら拒否する。
    put(
        &ov,
        "app/src/main/java/com/example/my/app/ui/screens/Foo.java",
        b"x",
    );
    assert_eq!(load_android(&m).unwrap().shell_overlay.len(), 1);
    let deep: String = (0..17).map(|i| format!("d{i}/")).collect();
    put(&ov, &format!("{deep}x"), b"x");
    assert!(err(load_android(&m)).contains("深すぎます"));
}

/// `eject/android/shell/` は run・build の入力。gen（生成物の差し替え）は綴り違いの扱いに巻き込まない。
#[test]
fn shell_overlay_is_not_treated_as_a_stray_eject_file_by_gen() {
    let m = app("shell-gen-ok", "");
    put(&dir(&m).join("eject/android/shell"), "app/x.txt", b"x");
    let g = generate(Some(&m)).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(g.notes.iter().all(|n| !n.starts_with("eject 済み")));
}

#[test]
fn shell_overlay_record_with_escaping_entries_deletes_nothing() {
    let m = app("shell-record", "");
    put(&dir(&m).join("eject/android/shell"), "a.txt", b"x");
    let (_, sh) = run_until_ndk(&m);
    let outside = sh.parent().unwrap().join("victim.txt");
    fs::write(&outside, "v").unwrap();
    fs::write(sh.join("a.txt"), "x").unwrap();
    let abs = serde_json::to_string(&outside.display().to_string()).unwrap();
    for bad in [
        "[\"a.txt\",\"../victim.txt\"]".to_string(),
        format!("[\"a.txt\",{abs}]"),
        "not json".to_string(),
    ] {
        fs::write(sh.join(".fandhe-overlay"), &bad).unwrap();
        let app = load_android(&m).unwrap();
        let ops = prepare(&app, &build_opts(&m), &android_env()).unwrap();
        let upto = ops.iter().position(|o| matches!(o, Op::Spawn(_))).unwrap();
        assert!(execute(&ops[..upto]).is_err(), "{bad}");
        assert!(outside.is_file() && sh.join("a.txt").is_file(), "{bad}");
    }
}

/// 控えの項目の途中が symlink なら Prune が、写し先の途中が symlink なら Overlay が、何も書かず消さずに止まる。
#[cfg(unix)]
#[test]
fn shell_overlay_prune_and_overlay_stop_at_symlinks() {
    let m = app("shell-symlink-ops", "");
    let ov = dir(&m).join("eject/android/shell");
    put(&ov, "app/a.txt", b"x");
    let (_, sh) = run_until_ndk(&m);
    let outside = sh.parent().unwrap().join("outside");
    put(&outside, "a.txt", b"victim");
    let ops_for = || {
        let app = load_android(&m).unwrap();
        let ops = prepare(&app, &build_opts(&m), &android_env()).unwrap();
        let upto = ops.iter().position(|o| matches!(o, Op::Spawn(_))).unwrap();
        ops[..upto].to_vec()
    };
    // Prune: 控えの項目 `app/a.txt` の `app` が symlink。
    fs::remove_dir_all(sh.join("app")).unwrap();
    std::os::unix::fs::symlink(&outside, sh.join("app")).unwrap();
    let e = execute(&ops_for()[..1]).expect_err("止まる");
    assert!(e.contains("symlink"), "{e}");
    assert_eq!(fs::read(outside.join("a.txt")).unwrap(), b"victim");
    // Overlay: 写し先の途中（`app`）が symlink。控えを消して Prune を通し、Overlay だけを実行する。
    fs::remove_file(sh.join(".fandhe-overlay")).unwrap();
    let ops = ops_for();
    let at = ops
        .iter()
        .position(|o| matches!(o, Op::Overlay { .. }))
        .unwrap();
    let e = execute(&ops[at..=at]).expect_err("止まる");
    assert!(e.contains("symlink"), "{e}");
    assert_eq!(fs::read(outside.join("a.txt")).unwrap(), b"victim");
}
