//! 入口 crate の feature が全機能 crate を漏れなく選べることの検査（Issue #1199）。
//!
//! 機能 crate（`crates/fandhe-cap-<id>`）を足したら、`Cargo.toml` の feature・optional 依存と
//! `src/lib.rs` の再公開を同じ id で足す。足し忘れをここで落とす。

use std::path::Path;

#[test]
fn every_capability_crate_has_a_feature_and_reexport() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml を読む");
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).expect("src/lib.rs を読む");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(root.join("..")).expect("crates/ を読む") {
        let name = entry
            .expect("crates/ の項目")
            .file_name()
            .into_string()
            .expect("UTF-8 の名前");
        let Some(id) = name.strip_prefix("fandhe-cap-") else {
            continue;
        };
        let stem = id.replace('-', "_");
        let checks = [
            (&manifest, format!("\n{id} = [\"dep:fandhe-cap-{id}\"]\n")),
            (
                &manifest,
                format!(
                    "\nfandhe-cap-{id} = {{ path = \"../fandhe-cap-{id}\", optional = true }}\n"
                ),
            ),
            (&lib, format!("feature = \"{id}\"")),
            (&lib, format!("\npub use fandhe_cap_{stem} as {stem};\n")),
        ];
        for (text, needle) in checks {
            if !text.contains(&needle) {
                missing.push(format!("{id}: {}", needle.trim()));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "入口 crate に足りない宣言:\n{}",
        missing.join("\n")
    );
}
