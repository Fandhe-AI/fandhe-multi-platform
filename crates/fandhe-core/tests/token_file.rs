#![cfg(feature = "token-file")]
//! `token::file`（feature `token-file`）の公開 API 経由の結合テスト
//! （TASK-8.4-S17、#173）。
//!
//! `docs/architecture/token-resolution-architecture.md` §3.1〜§3.2・§5.3 の
//! 契約（参照文字列パースの集約・上限値・S18 前段の構文検証のみ）を、既存の
//! ビルトイントークンセット（`palette_token_set` 等）とのラウンドトリップ・
//! `TokenResolver::resolve` との接続妥当性の観点で確認する。網羅的なユニット
//! テストは `crates/fandhe-core/src/token/file.rs` 内 `#[cfg(test)] mod tests`
//! に置き、本ファイルは公開 API（`fandhe_core::*` の再エクスポート）が意図どおり
//! 機能することの結合テストに留める（`tests/token_resolver.rs` と同方針）。

use std::io::Write;

use fandhe_core::{
    SemanticColorRole, TokenCategory, TokenDefinition, TokenPath, TokenRef, TokenResolver,
    TokenSet, TokenValue, animation_token_set, border_token_set, load_token_set_file,
    palette_token_set, parse_token_set_json, resolve_border_color, semantic_color_entry,
    serialize_token_set_json, shadow_token_set, typography_token_set,
};

fn path(dotted: &str) -> TokenPath {
    TokenPath::from_dotted(dotted).expect("テスト用の正当なパス")
}

fn merged(sets: Vec<TokenSet>) -> TokenSet {
    let mut merged = TokenSet::new();
    for set in sets {
        merged.extend(set);
    }
    merged
}

#[test]
fn round_trips_palette_token_set() {
    let original = palette_token_set().expect("パレットは構築できる");
    let json = serialize_token_set_json(&original).expect("シリアライズできる");
    let parsed = parse_token_set_json(&json).expect("再パースできる");
    assert_eq!(original, parsed);
}

#[test]
fn round_trips_typography_token_set() {
    let original = typography_token_set().expect("構築できる");
    let json = serialize_token_set_json(&original).expect("シリアライズできる");
    let parsed = parse_token_set_json(&json).expect("再パースできる");
    assert_eq!(original, parsed);
}

#[test]
fn round_trips_border_token_set() {
    let original = border_token_set().expect("構築できる");
    let json = serialize_token_set_json(&original).expect("シリアライズできる");
    let parsed = parse_token_set_json(&json).expect("再パースできる");
    assert_eq!(original, parsed);
}

#[test]
fn round_trips_shadow_token_set() {
    let original = shadow_token_set().expect("構築できる");
    let json = serialize_token_set_json(&original).expect("シリアライズできる");
    let parsed = parse_token_set_json(&json).expect("再パースできる");
    assert_eq!(original, parsed);
}

#[test]
fn round_trips_animation_token_set() {
    let original = animation_token_set().expect("構築できる");
    let json = serialize_token_set_json(&original).expect("シリアライズできる");
    let parsed = parse_token_set_json(&json).expect("再パースできる");
    assert_eq!(original, parsed);
}

/// セマンティックカラー（エイリアス参照を含むセット）をシリアライズ →
/// 再パース → `TokenResolver::resolve` で解決できることを確認する
/// （実装計画 §5「S17 → resolver の接続妥当性」）。
#[test]
fn round_tripped_semantic_color_resolves_via_resolver() {
    let palette = palette_token_set().expect("パレットは構築できる");
    let palette_ref = TokenRef::new(path("color.palette.blue-500"));
    let (semantic_path, semantic_entry) =
        semantic_color_entry(SemanticColorRole::Primary, palette_ref)
            .expect("セマンティックエントリは構築できる");

    let mut base = merged(vec![palette]);
    base.insert(semantic_path.clone(), semantic_entry);

    let json = serialize_token_set_json(&base).expect("シリアライズできる");
    let parsed = parse_token_set_json(&json).expect("再パースできる");
    assert_eq!(base, parsed);

    let resolver = TokenResolver::new(parsed, TokenSet::new());
    let resolved = resolver
        .resolve(&TokenRef::new(semantic_path))
        .expect("ラウンドトリップ後も解決できる");
    assert_eq!(resolved.value().category(), TokenCategory::Color);
}

/// border の色参照を含むセットをラウンドトリップし、`resolve_border_color` で
/// 解決可能であることを確認する。
#[test]
fn round_tripped_border_color_reference_resolves() {
    let base = merged(vec![
        palette_token_set().expect("パレットは構築できる"),
        border_token_set().expect("ボーダーは構築できる"),
    ]);

    let json = serialize_token_set_json(&base).expect("シリアライズできる");
    let parsed = parse_token_set_json(&json).expect("再パースできる");

    let border_path = path("border.default");
    let definition = parsed.get(&border_path).expect("border.default は存在する");
    let TokenDefinition::Value {
        value: TokenValue::Border(border),
    } = definition
    else {
        panic!("border.default は Border 値であるべきです: {definition:?}");
    };

    let resolver = TokenResolver::new(parsed.clone(), TokenSet::new());
    resolve_border_color(&resolver, border).expect("色参照はラウンドトリップ後も解決できる");
}

#[test]
fn load_token_set_file_reads_valid_file() {
    let mut file = tempfile();
    write!(
        file,
        r##"{{ "color": {{ "primary": {{ "$type": "color", "$value": "#123456" }} }} }}"##
    )
    .expect("一時ファイルへの書き込みは成功する");
    let set = load_token_set_file(file.path()).expect("正当なファイルは読み込める");
    assert_eq!(set.len(), 1);
}

#[test]
fn load_token_set_file_rejects_missing_path() {
    let dir = std::env::temp_dir();
    let missing = dir.join("fandhe-core-token-file-test-does-not-exist.json");
    let err = load_token_set_file(&missing).expect_err("存在しないパスは拒否される");
    assert!(matches!(err, fandhe_core::TokenError::FileRead { .. }));
}

#[test]
fn load_token_set_file_rejects_oversized_file() {
    let mut file = tempfile();
    // MAX_TOKEN_FILE_BYTES（1 MiB）を超える巨大な JSON 文字列値を書き込む。
    let padding = "a".repeat(fandhe_core::MAX_TOKEN_FILE_BYTES as usize + 1);
    write!(
        file,
        r#"{{ "text": {{ "t": {{ "$type": "text", "$value": "{padding}" }} }} }}"#
    )
    .expect("一時ファイルへの書き込みは成功する");
    let err = load_token_set_file(file.path()).expect_err("サイズ上限超過は拒否される");
    assert!(matches!(err, fandhe_core::TokenError::FileRead { .. }));
}

#[test]
fn parse_token_set_json_rejects_malformed_json() {
    let err =
        parse_token_set_json("{ this is not valid json").expect_err("不正な JSON は拒否される");
    assert!(matches!(err, fandhe_core::TokenError::ParseError { .. }));
}

#[test]
fn parse_token_set_json_treats_type_bearing_object_as_single_token() {
    // "color" 直下に `$type` があるため、これ以上グループとして再帰せず単独の
    // トークンとして解釈される仕様（`walk_group` の判定基準）を確認する。
    let json = r##"{
        "color": { "$type": "color", "$value": "#000000" }
    }"##;
    let set = parse_token_set_json(json).expect("単一トークンとして解析できる");
    assert_eq!(set.len(), 1);
    assert!(set.contains_key(&path("color")));
}

/// テスト用の一時ファイルを作る簡易ヘルパー（外部クレートを追加せず
/// `std::env::temp_dir` + プロセス ID + テスト内カウンタで一意なパスを作る）。
fn tempfile() -> NamedTempFile {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "fandhe-core-token-file-test-{}-{}.json",
        std::process::id(),
        id
    ));
    NamedTempFile {
        file: std::fs::File::create(&path).expect("一時ファイルを作成できる"),
        path,
    }
}

/// 破棄時に自身を削除する最小限の一時ファイルラッパー（`tempfile` クレートを
/// 追加依存に含めないための自前実装）。
struct NamedTempFile {
    file: std::fs::File,
    path: std::path::PathBuf,
}

impl NamedTempFile {
    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Write for NamedTempFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl Drop for NamedTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
