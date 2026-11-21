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

/// 機能 crate が crate ルートから出す起動前の登録（`register`）を、入口（`src/entry/lifecycle.rs`）が feature つきで
/// 呼んでいることの検査（Issue #1200）。起動前の登録は初めて使われるより前に要るため、登録表へ遅らせられない。
///
/// 入口が呼ばないと、利用者は機能ごとの手順を自分で書く必要が出る（`#[fandhe::main]` だけで動く前提が崩れる）。
/// crate ルートの `pub use …` と `pub fn` の名前をテキストで拾う近似で、OS の `cfg` は見ない。
#[test]
fn every_before_launch_entry_is_called_by_the_entry() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lifecycle =
        std::fs::read_to_string(root.join("src/entry/lifecycle.rs")).expect("lifecycle.rs を読む");
    let mut missing = Vec::new();
    for (id, dir) in capability_crates(root) {
        let stem = id.replace('-', "_");
        let lib = read(&dir.join("src/lib.rs"));
        if root_exported_names(&lib).iter().any(|n| n == "register") {
            let call = format!("fandhe_cap_{stem}::register(");
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

/// 終了処理・再生成時の破棄の口（`shutdown`・`stop_shared_*`・`reset_*`）を持つ機能 crate が、自分で登録表
/// （`fandhe_runtime::teardown`）へ登録していることの検査（Issue #1453）。
///
/// feature で選んでも直接依存で選んでも同じ口を通るよう、入口（`src/entry/lifecycle.rs`）からは呼ばない。
/// 登録は「初めて使われたとき」に行うため、次の関数の本体の先頭で `register_teardown()` を呼ぶことも検査する
/// （呼ばない入口から使われると、その機能の片付けが走らない）。
///
/// - crate ルートから出す関数（定義のファイルを問わない）と、`driver.rs`・`lib.rs` の行頭の `pub fn`（effect を含む）
/// - 公開型（`driver.rs`・`lib.rs` の行頭の `pub struct` / `pub enum` と、crate ルートが名前で出す型）の `impl` の `fn`
///   （trait の実装を含む。`Debug`・`Display`・`Error`・`From`・`Clone`・`Default`・`Drop` などの std の trait は除く）
///
/// `contract.rs` の型（`pub use contract::*` で出る）は対象にしない。契約の型と代わりの実装（`Unsupported*`・
/// デスクトップの `CommandTts` など）の置き場で、登録表が片付ける static の状態に触れないため。
/// `pub use … as 別名` は名前の照合を外れるため、見つけたら失敗にする。テキストで拾う近似で、OS の `cfg` は見ない。
#[test]
fn every_teardown_entry_is_registered_by_the_crate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lifecycle =
        std::fs::read_to_string(root.join("src/entry/lifecycle.rs")).expect("lifecycle.rs を読む");
    let mut missing = Vec::new();
    for (id, dir) in capability_crates(root) {
        let stem = id.replace('-', "_");
        let lib = read(&dir.join("src/lib.rs"));
        let exported = root_exported_names(&lib);
        let teardown: Vec<&String> = exported.iter().filter(|n| is_teardown_name(n)).collect();
        if teardown.is_empty() {
            continue;
        }
        if let Some(alias) = pub_use_aliases(&lib).first() {
            missing.push(format!(
                "{id}: `pub use … as` は検査を外れるため使わない（{alias}）"
            ));
        }
        let sources = crate_sources(&dir);
        // `register_teardown` の本体（コメントと空白を除く）。登録の呼び出しと各関数の登録を見る。
        let registration: String = sources
            .iter()
            .filter_map(|(_, t)| {
                let code = strip_comments(t);
                code.find("fn register_teardown()")
                    .and_then(|at| fn_body(&code, at).map(|b| b.to_string()))
            })
            .collect::<String>()
            .split_whitespace()
            .collect();
        if !registration.contains("fandhe_runtime::teardown::register(") {
            missing.push(format!(
                "{id}: register_teardown の本体で fandhe_runtime::teardown::register を呼ぶ"
            ));
        }
        if !registration.contains(&format!("Teardown::new(\"{id}\")")) {
            missing.push(format!("{id}: Teardown::new(\"{id}\") で登録する"));
        }
        for name in &teardown {
            let event = if name.starts_with("reset_") {
                "on_android_recreate"
            } else {
                "on_exit"
            };
            if !registration.contains(&format!(".{event}(crate::{name})")) {
                missing.push(format!("{id}: .{event}(crate::{name}) を登録する"));
            }
            if lifecycle.contains(&format!("fandhe_cap_{stem}::{name}(")) {
                missing.push(format!(
                    "{id}: 入口から {name} を直接呼ばない（登録表と二重になる）"
                ));
            }
        }
        let public_types = public_types(&lib, &sources, &exported);
        for (path, text) in &sources {
            let code = strip_comments(text);
            let file = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
            let all_pub = file == "driver.rs" || file == "lib.rs";
            let mut entries: Vec<(String, &str)> = top_level_pub_fns(&code)
                .into_iter()
                .filter(|(name, _)| !is_teardown_name(name) && (all_pub || exported.contains(name)))
                .collect();
            for (ty, methods) in impl_methods(&code) {
                if public_types.contains(&ty) {
                    entries.extend(
                        methods
                            .into_iter()
                            .map(|(name, body)| (format!("{ty}::{name}"), body)),
                    );
                }
            }
            for (name, body) in entries {
                let body = body.trim_start();
                if !body.starts_with("register_teardown();")
                    && !body.starts_with("crate::driver::register_teardown();")
                {
                    missing.push(format!(
                        "{id}: {} の {name} の先頭で register_teardown() を呼ぶ",
                        path.display()
                    ));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "登録表への登録の不足:\n{}",
        missing.join("\n")
    );
}

/// media-playback → audio-session の停止順（音声セッションの上で再生する機能を先に止める）を、登録順に依らず
/// 段階で保つことの検査（Issue #1453。段階の順そのものは `fandhe_runtime::teardown` のテストが固定する）。
/// コメントは見ず、`register_teardown` の本体の `.stage(…)` を照合する。
#[test]
fn audio_session_stops_after_its_users() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let shared = |id: &str| {
        crate_sources(&root.join("..").join(format!("fandhe-cap-{id}")))
            .iter()
            .any(|(_, t)| {
                let code = strip_comments(t);
                code.find("fn register_teardown()")
                    .and_then(|at| fn_body(&code, at))
                    .is_some_and(|b| {
                        b.split_whitespace()
                            .collect::<String>()
                            .contains(".stage(fandhe_runtime::teardown::Stage::Shared)")
                    })
            })
    };
    assert!(shared("audio-session"));
    assert!(!shared("media-playback"));
    assert!(!shared("now-playing-controls"));
}

/// 公開型の名前: `driver.rs`・`lib.rs` の行頭の `pub struct` / `pub enum` と、crate ルートが名前で出す型。
fn public_types(
    lib: &str,
    sources: &[(std::path::PathBuf, String)],
    exported: &[String],
) -> Vec<String> {
    let mut types: Vec<String> = exported
        .iter()
        .filter(|n| n.starts_with(|c: char| c.is_ascii_uppercase()))
        .cloned()
        .collect();
    let driver = sources
        .iter()
        .filter(|(p, _)| p.file_name().is_some_and(|f| f == "driver.rs"))
        .map(|(_, t)| t.as_str());
    for text in std::iter::once(lib).chain(driver) {
        for line in text.lines() {
            for prefix in ["pub struct ", "pub enum "] {
                if let Some(rest) = line.strip_prefix(prefix) {
                    types.push(ident(rest));
                }
            }
        }
    }
    types
}

/// 行頭の `impl …` ブロックの (型名, [(関数名, 本体)])。std の trait の実装は除く。
fn impl_methods(code: &str) -> Vec<(String, Vec<(String, &str)>)> {
    const STD_TRAITS: [&str; 14] = [
        "Debug",
        "Display",
        "Error",
        "From",
        "Clone",
        "Default",
        "Drop",
        "PartialEq",
        "Eq",
        "Hash",
        "PartialOrd",
        "Ord",
        "AsRef",
        "Deref",
    ];
    let mut out = Vec::new();
    let mut offset = 0;
    for line in code.split_inclusive('\n') {
        let at = offset;
        offset += line.len();
        let Some(rest) = line.strip_prefix("impl") else {
            continue;
        };
        if !rest.starts_with([' ', '<']) {
            continue;
        }
        let header = skip_generics(rest.split('{').next().unwrap_or("").trim_start());
        let (trait_name, ty) = match header.split_once(" for ") {
            Some((t, ty)) => (Some(t), ty),
            None => (None, header),
        };
        if let Some(t) = trait_name {
            // `From<a::B>` の汎用引数を除いてから最後の区切りを取る。
            let path = t.trim().split('<').next().unwrap_or("");
            let last = path.rsplit("::").next().unwrap_or("");
            if STD_TRAITS.contains(&last) {
                continue;
            }
        }
        let Some(block) = fn_body(code, at) else {
            continue;
        };
        let mut methods = Vec::new();
        let mut inner = 0;
        for l in block.split_inclusive('\n') {
            let start = inner;
            inner += l.len();
            let Some(sig) = ["    pub fn ", "    pub(crate) fn ", "    fn "]
                .iter()
                .find_map(|p| l.strip_prefix(p))
            else {
                continue;
            };
            if let Some(body) = fn_body(block, start) {
                methods.push((ident(sig), body));
            }
        }
        out.push((ident(ty.trim()), methods));
    }
    out
}

/// `impl<T> Foo<T>` の `<T>` を飛ばした残り。
fn skip_generics(header: &str) -> &str {
    if !header.starts_with('<') {
        return header;
    }
    let mut depth = 0usize;
    for (i, c) in header.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return header[i + 1..].trim_start();
                }
            }
            _ => {}
        }
    }
    header
}

/// 行頭の `pub fn <name>`（`const fn` を除く）の名前と本体（1 行の本体も扱う）。
fn top_level_pub_fns(code: &str) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for line in code.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("pub fn ")
            && let Some(body) = fn_body(code, offset)
        {
            out.push((ident(rest), body));
        }
        offset += line.len();
    }
    out
}

/// `at` から後ろで最初の `{` に対応する本体（括弧の内側）。宣言だけ（`;` が先）なら `None`。
/// 文字列中の括弧は数えない近似（コメントは [`strip_comments`] で先に除く）。
fn fn_body(code: &str, at: usize) -> Option<&str> {
    let rest = &code[at..];
    let open = rest.find('{')?;
    if rest.find(';').is_some_and(|semi| semi < open) {
        return None;
    }
    let mut depth = 0usize;
    for (i, c) in rest[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&rest[open + 1..open + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// 行コメント（`//` 以降。doc コメントを含む）を除く。文字列中の `//` も切る近似。
fn strip_comments(text: &str) -> String {
    text.lines()
        .map(|l| l.find("//").map_or(l, |i| &l[..i]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 先頭の識別子。
fn ident(s: &str) -> String {
    s.trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// `pub use … as 別名` の別名。
fn pub_use_aliases(lib: &str) -> Vec<String> {
    let code = strip_comments(lib);
    let mut aliases = Vec::new();
    let mut rest = code.as_str();
    while let Some(start) = rest.find("pub use ") {
        let after = &rest[start + "pub use ".len()..];
        let end = after.find(';').unwrap_or(after.len());
        for part in after[..end].split([',', '{', '}']) {
            if let Some((_, alias)) = part.split_once(" as ") {
                aliases.push(alias.trim().to_string());
            }
        }
        rest = &after[end..];
    }
    aliases
}

fn is_teardown_name(name: &str) -> bool {
    name == "shutdown" || name.starts_with("reset_") || name.starts_with("stop_shared_")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{} を読む: {e}", path.display()))
}

/// `crates/fandhe-cap-*` の (id, ディレクトリ)。
fn capability_crates(root: &Path) -> Vec<(String, std::path::PathBuf)> {
    let mut crates = Vec::new();
    for entry in std::fs::read_dir(root.join("..")).expect("crates/ を読む") {
        let entry = entry.expect("crates/ の項目");
        let name = entry.file_name().into_string().expect("UTF-8 の名前");
        if let Some(id) = name.strip_prefix("fandhe-cap-") {
            crates.push((id.to_string(), entry.path()));
        }
    }
    crates
}

/// 機能 crate の `src/` 配下の `.rs`（パスと中身）。
fn crate_sources(dir: &Path) -> Vec<(std::path::PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.join("src")];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("src/ を読む") {
            let path = entry.expect("src/ の項目").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = read(&path);
                out.push((path, text));
            }
        }
    }
    out
}

/// crate ルートの `pub use path::name;`・`pub use path::{a, b};`（複数行を含む）・`pub fn name` の名前。
fn root_exported_names(lib: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = lib;
    while let Some(start) = rest.find("pub use ") {
        let after = &rest[start + "pub use ".len()..];
        let end = after.find(';').unwrap_or(after.len());
        let item = &after[..end];
        let list = match (item.find('{'), item.rfind('}')) {
            (Some(a), Some(b)) if a < b => &item[a + 1..b],
            _ => item.rsplit("::").next().unwrap_or(""),
        };
        names.extend(
            list.split(',')
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty()),
        );
        rest = &after[end..];
    }
    for line in lib.lines() {
        if let Some(rest) = line.strip_prefix("pub fn ") {
            names.extend(rest.split(['(', '<']).next().map(|n| n.trim().to_string()));
        }
    }
    names
}
