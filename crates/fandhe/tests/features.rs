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
            (
                &lib,
                format!(
                    "\npub mod {stem} {{\n    pub use fandhe_cap_{stem}::*;\n    #[allow(dead_code, hidden_glob_reexports)]\n    mod driver {{}}\n}}\n"
                ),
            ),
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

/// 機能 crate が crate ルートから出す起動・終了の口（`register`・`shutdown`・`reset_*`・`stop_shared_*`）を、
/// 入口（`src/entry/lifecycle.rs`）が feature つきで呼んでいることの検査（Issue #1200）。
///
/// 入口が呼ばないと、利用者は機能ごとの手順を自分で書く必要が出る（`#[fandhe::main]` だけで動く前提が崩れる）。
/// crate ルートの `pub use …::{…}` と `pub fn` の名前をテキストで拾う近似で、OS の `cfg` は見ない。
#[test]
fn every_lifecycle_entry_is_called_by_the_entry() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lifecycle =
        std::fs::read_to_string(root.join("src/entry/lifecycle.rs")).expect("lifecycle.rs を読む");
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
        let lib = std::fs::read_to_string(root.join("..").join(&name).join("src/lib.rs"))
            .expect("機能 crate の lib.rs を読む");
        for item in root_exported_names(&lib) {
            let lifecycle_like = item == "register"
                || item == "shutdown"
                || item.starts_with("reset_")
                || item.starts_with("stop_shared_");
            if !lifecycle_like {
                continue;
            }
            let call = format!("fandhe_cap_{stem}::{item}(");
            let gated = format!("feature = \"{id}\"");
            if !lifecycle.contains(&call) || !lifecycle.contains(&gated) {
                missing.push(format!("{call}…) を feature \"{id}\" つきで呼ぶ"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "src/entry/lifecycle.rs に足りない呼び出し:\n{}",
        missing.join("\n")
    );
}

/// crate ルートの `pub use path::name;`・`pub use path::{a, b};`・`pub fn name` の名前。
fn root_exported_names(lib: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in lib.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("pub use ") {
            let rest = rest.trim_end_matches(';');
            let list = match (rest.find('{'), rest.rfind('}')) {
                (Some(a), Some(b)) if a < b => &rest[a + 1..b],
                _ => rest.rsplit("::").next().unwrap_or(""),
            };
            names.extend(
                list.split(',')
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty()),
            );
        } else if let Some(rest) = line.strip_prefix("pub fn ") {
            names.extend(rest.split(['(', '<']).next().map(|n| n.trim().to_string()));
        }
    }
    names
}
