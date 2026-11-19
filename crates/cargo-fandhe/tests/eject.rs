//! `cargo fandhe gen --eject` と `eject/` の差し替えのテスト（Issue #1202）。
//! 実際の cargo を通す単独アプリ（`[workspace]` を自分で持つ）を一時ディレクトリに作って確かめる。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cargo_fandhe::run::{load_app, Action, Options, Os};
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
