//! build script（`build.rs`）から使う Apple 向け helper。ObjC（`.m`）と Swift 薄層のコンパイル、
//! framework のリンクを 1 か所に集め、機能 crate が同じ手順を複製しないようにする
//! （TASK-21.2 / Issue #1168）。
//!
//! 呼び出し元は各機能 crate の `build.rs`（`fandhe-platform-apple` を feature `build` 付きの
//! build 依存にする）。いずれの関数も **build script 専用**で、失敗はビルド失敗にする
//! （各関数の `# Panics` 参照）。
//!
//! framework のリンクは単位（1 つの `.m`・Swift 薄層）ごとに自己完結して宣言する。
//! 「他の単位が既にリンクしているはず」という暗黙の依存を置かない。重複はこのモジュールが
//! プロセス内で畳む。
//!
//! # 制約
//!
//! `cargo:rustc-link-arg` は、それを出した package 自身の bin・cdylib・テストにしか効かず、
//! 依存元の package へは伝わらない（Cargo の build-scripts 文書の `rustc-link-arg` 節）。
//! `rustc-link-lib` は rlib 経由で伝わる。したがって [`weak_link_framework`]・[`compile_swift`]
//! が出す弱リンク・`-rpath`・clang_rt は、rlib の機能 crate の `build.rs` から呼ぶと最終成果物に
//! 届かない。これらを使う機能を移すときは、最終成果物を作る package 側で同じ指定を出す。

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// ターゲット OS が iOS か。build script の `CARGO_CFG_TARGET_OS` を読む。
pub fn target_is_ios() -> bool {
    target_os() == "ios"
}

/// ターゲット OS が macOS か。
pub fn target_is_macos() -> bool {
    target_os() == "macos"
}

fn target_os() -> String {
    std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default()
}

/// ターゲット OS の UI framework 名（iOS は UIKit、それ以外は AppKit）。
///
/// iOS・macOS 共用の `.m` が UI を触るとき、リンク対象を OS で読み分けるために使う。
pub fn ui_framework() -> &'static str {
    if target_is_ios() { "UIKit" } else { "AppKit" }
}

/// framework を強リンクする。同じ framework はプロセス内で一度だけ出力する。
pub fn link_frameworks(frameworks: &[&str]) {
    static LINKED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut linked = LINKED
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .expect("リンク済み framework の表が壊れている");
    for f in frameworks {
        if linked.insert((*f).to_string()) {
            println!("cargo:rustc-link-lib=framework={f}");
        }
    }
}

/// framework を弱リンクする（`-Wl,-weak_framework`）。実行体の最低 OS より新しい framework 用。
///
/// `rustc-link-arg` を出すため、依存元へは伝わらない（モジュール doc の「制約」）。
pub fn weak_link_framework(framework: &str) {
    println!("cargo:rustc-link-arg=-Wl,-weak_framework,{framework}");
}

/// `src`（呼び出し元 crate のルートからの相対パス）の `.m` を `-fobjc-arc` でコンパイルし、
/// `frameworks` をリンクする。
///
/// - 静的ライブラリ名はファイル名の stem。
/// - 共有ヘッダ `fandhe_ios_common.h` の置き場を常に `-I` へ足す。
/// - `.m` とヘッダの変更で build script を再実行する。
///
/// `frameworks` はその `.m` が `#import` する framework を完全に書く。
///
/// # Panics
///
/// `src` が `.m` のファイル名でない場合、またはコンパイラが失敗した場合（cc crate による）。
pub fn compile_objc(src: &str, frameworks: &[&str]) {
    let lib = Path::new(src)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_else(|| panic!("compile_objc: ファイル名を取れない: {src}"))
        .to_string();
    let include = concat!(env!("CARGO_MANIFEST_DIR"), "/include");
    println!("cargo:rerun-if-changed={src}");
    println!("cargo:rerun-if-changed={include}/fandhe_ios_common.h");
    cc::Build::new()
        .file(src)
        .include(include)
        .flag("-fobjc-arc")
        .compile(&lib);
    link_frameworks(frameworks);
}

/// Swift 薄層 1 本ぶんのビルド指定（[`compile_swift`] への入力）。
pub struct SwiftLib {
    /// Swift ソース（呼び出し元 crate のルートからの相対パス）。
    pub srcs: &'static [&'static str],
    /// 静的ライブラリ名（`lib` 接頭辞と `.a` を除く）。
    pub lib: &'static str,
    /// Swift のモジュール名。
    pub module: &'static str,
    /// リンクする framework 名。
    pub framework: &'static str,
    /// `true` なら弱リンクにし、autolink も抑止する（実行体の最低 OS より新しい framework 用）。
    /// `false` なら強リンクする。
    pub weak: bool,
    /// 本番・配布ビルドでスキップを禁じる環境変数名。
    pub require_env: &'static str,
    /// panic・警告の接頭辞。
    pub tag: &'static str,
}

/// `TARGET` から Swift コンパイルに使う `(sdk, triple, lib_dir)` を引く。未対応なら `None`。
fn swift_target(target: &str) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match target {
        "aarch64-apple-darwin" => ("macosx", "arm64-apple-macos11.0", "macosx"),
        "x86_64-apple-darwin" => ("macosx", "x86_64-apple-macos11.0", "macosx"),
        "aarch64-apple-ios-sim" => (
            "iphonesimulator",
            "arm64-apple-ios17.0-simulator",
            "iphonesimulator",
        ),
        "aarch64-apple-ios" => ("iphoneos", "arm64-apple-ios17.0", "iphoneos"),
        "x86_64-apple-ios" => (
            "iphonesimulator",
            "x86_64-apple-ios17.0-simulator",
            "iphonesimulator",
        ),
        _ => return None,
    })
}

/// Swift 薄層を `swiftc` で静的ライブラリ化してリンクする。`LanguageModelSession` や
/// `Activity<Attributes>` のような Swift 専用 API は ObjC から呼べないため、`cc` ではなく swiftc を使う。
///
/// `spec.weak` が `true` の framework は弱リンクにする。SDK に無い場合（古い Xcode）は
/// リンクしない（Swift 側の `canImport` が不成立で Unsupported 経路になるため）。
/// Apple ツール（`xcrun`）が無いホストでは警告を出してスキップする（型検査専用。リンクは失敗する）。
/// `spec.require_env`=1（本番・配布ビルド）ではスキップせず panic する。
///
/// `rustc-link-arg`（弱リンク・`-rpath`・clang_rt）を出すため、依存元へは伝わらない
/// （モジュール doc の「制約」）。
///
/// # Panics
///
/// 未対応の `TARGET`、`xcrun` の失敗、swiftc の失敗、`xcrun` が無く `spec.require_env`=1 のとき。
pub fn compile_swift(spec: &SwiftLib) {
    let tag = spec.tag;
    let framework = spec.framework;
    println!("cargo:rerun-if-env-changed={}", spec.require_env);
    for s in spec.srcs {
        println!("cargo:rerun-if-changed={s}");
    }
    let have_xcrun = Command::new("xcrun")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !have_xcrun {
        assert!(
            std::env::var(spec.require_env).as_deref() != Ok("1"),
            "{tag}: xcrun が無い（{}=1 のためスキップ不可）",
            spec.require_env
        );
        println!(
            "cargo:warning={tag}: xcrun が無いため Swift 薄層の生成をスキップ（型検査専用。リンクは失敗する）"
        );
        return;
    }

    let target = std::env::var("TARGET").unwrap_or_default();
    let Some((sdk, triple, lib_dir)) = swift_target(&target) else {
        panic!("{tag}: 未対応のターゲット: {target}");
    };

    let xcrun = |args: &[&str]| -> String {
        let out = Command::new("xcrun")
            .args(args)
            .output()
            .expect("xcrun を実行できない");
        assert!(out.status.success(), "xcrun {args:?} が失敗した");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let sdk_path = xcrun(&["--sdk", sdk, "--show-sdk-path"]);
    let swiftc = xcrun(&["--sdk", sdk, "--find", "swiftc"]);

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let lib = format!("{out_dir}/lib{}.a", spec.lib);
    let mut cmd = Command::new("xcrun");
    cmd.args([
        "--sdk",
        sdk,
        "swiftc",
        "-parse-as-library",
        "-emit-library",
        "-static",
        "-O",
    ])
    .args([
        "-module-name",
        spec.module,
        "-target",
        triple,
        "-sdk",
        &sdk_path,
    ]);
    if spec.weak {
        // framework を強リンクとして autolink させない（旧 OS で dyld が要求して起動不能に
        // なるのを防ぐ。リンク側は下で -weak_framework を明示する）。
        cmd.args([
            "-Xfrontend",
            "-disable-autolink-framework",
            "-Xfrontend",
            framework,
        ]);
    }
    let out = cmd
        .args(["-o", &lib])
        .args(spec.srcs)
        .output()
        .expect("swiftc を実行できない");
    assert!(
        out.status.success(),
        "{tag}: swiftc が失敗した:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    println!("cargo:rustc-link-search=native={out_dir}");
    println!("cargo:rustc-link-lib=static={}", spec.lib);
    if spec.weak {
        // SDK に framework がある場合だけ弱リンクする。無い SDK（Xcode 16 等）では Swift 側の
        // `canImport` が不成立で Unsupported 経路になり、リンクすると framework not found になる。
        if Path::new(&sdk_path)
            .join(format!("System/Library/Frameworks/{framework}.framework"))
            .exists()
        {
            weak_link_framework(framework);
        }
    } else {
        link_frameworks(&[framework]);
    }
    // Swift ランタイム（OS 同梱）の検索パスと実行時パス。
    println!("cargo:rustc-link-search=native={sdk_path}/usr/lib/swift");
    if let Some(bin) = Path::new(&swiftc).parent().and_then(|p| p.parent()) {
        println!(
            "cargo:rustc-link-search=native={}/lib/swift/{lib_dir}",
            bin.display()
        );
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    // Swift の `#available` は `__isPlatformVersionAtLeast`（compiler-rt）を参照する。macOS は rustc が
    // clang_rt を自動リンクするが iOS は無いため、iOS では明示的にリンクする（Issue #346）。
    if sdk != "macosx" {
        let resource_dir = xcrun(&["--sdk", sdk, "clang", "--print-resource-dir"]);
        let rt = if sdk == "iphoneos" { "ios" } else { "iossim" };
        println!("cargo:rustc-link-arg={resource_dir}/lib/darwin/libclang_rt.{rt}.a");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swift_target_は対応する_5_ターゲットを引ける() {
        assert_eq!(swift_target("aarch64-apple-darwin").unwrap().0, "macosx");
        assert_eq!(swift_target("x86_64-apple-darwin").unwrap().0, "macosx");
        assert_eq!(swift_target("aarch64-apple-ios").unwrap().0, "iphoneos");
        assert_eq!(
            swift_target("aarch64-apple-ios-sim").unwrap().1,
            "arm64-apple-ios17.0-simulator"
        );
        assert_eq!(
            swift_target("x86_64-apple-ios").unwrap().2,
            "iphonesimulator"
        );
    }

    #[test]
    fn swift_target_は未対応を拒否する() {
        assert!(swift_target("aarch64-linux-android").is_none());
        assert!(swift_target("").is_none());
    }

    #[test]
    fn 共有ヘッダが_include_に実在する() {
        let header = concat!(env!("CARGO_MANIFEST_DIR"), "/include/fandhe_ios_common.h");
        assert!(Path::new(header).is_file());
    }
}
