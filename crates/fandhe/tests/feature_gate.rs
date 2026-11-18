//! 選ばなかった機能を呼ぶとコンパイルエラーになることの検査（Issue #1199 の受け入れ条件）。
//!
//! crate doc の例（`fandhe::clipboard::platform_clipboard()`）は、feature `clipboard` が無いときだけ
//! `compile_fail` になる。CI の `cargo test --all-features` では feature が有効で通常の例として
//! コンパイルされるため、ここで feature なしの doc test を別のビルド先で走らせる。

use std::process::Command;

#[test]
fn disabled_capability_does_not_compile() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    // 外側の `cargo test` がビルド先をロックしているため、別のビルド先を使う。
    let target_dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("feature-gate");
    let output = Command::new(cargo)
        .args([
            "test",
            "-p",
            "fandhe",
            "--doc",
            "--no-default-features",
            "--target-dir",
        ])
        .arg(&target_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo を起動する");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "feature なしの doc test が失敗した:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // compile_fail の例が実際に走った（feature なしで出力に現れる）ことも確かめる。
    assert!(
        stdout.contains("compile fail"),
        "compile_fail の例が走っていない:\n{stdout}"
    );
}
