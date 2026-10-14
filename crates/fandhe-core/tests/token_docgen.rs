//! `token::docgen` の鮮度検証・結合テスト（TASK-8.4-S20、#176）。
//!
//! `crates/fandhe-core/tests/platform_theme.rs` の前例に倣い、
//! `fandhe_core::*` の再エクスポートのみを経由して公開 API を固定化する。
//! 本ファイルの主眼は「コミット済み `docs/tokens/design-token-reference.md`
//! が現在のコードから生成される内容と一致すること」（設計文書 §4 の
//! 整合性・OWASP A08 対策、security.md）の CI 強制であり、値レンダリング・
//! エスケープ等の単体検証は `crates/fandhe-core/src/token/docgen.rs` の
//! `#[cfg(test)] mod tests` が担う。

use fandhe_core::{default_token_doc_sections, generate_token_reference};

/// `examples/generate_token_docs.rs` と同じ書き出し先（リポジトリルート直下の
/// `docs/tokens/design-token-reference.md`）。`CARGO_MANIFEST_DIR` は
/// `crates/fandhe-core` を指すため、そこから 2 階層上がってルートへ出る。
const COMMITTED_REFERENCE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/tokens/design-token-reference.md"
);

#[test]
fn generated_reference_matches_committed_file() {
    let sections =
        default_token_doc_sections().expect("既定トークンセットの構築に失敗しました（実装不備）");
    let generated = generate_token_reference(&sections)
        .expect("Markdown リファレンスの生成に失敗しました（実装不備）");

    let committed = std::fs::read_to_string(COMMITTED_REFERENCE_PATH).unwrap_or_else(|error| {
        panic!(
            "{COMMITTED_REFERENCE_PATH} の読み込みに失敗しました: {error}\n\
             次のコマンドで生成してからコミットしてください:\n\
             cargo run -p fandhe-core --example generate_token_docs"
        )
    });

    assert_eq!(
        generated, committed,
        "docs/tokens/design-token-reference.md がコード上のトークン定義と乖離しています。\n\
         次のコマンドで再生成し、差分をコミットしてください:\n\
         cargo run -p fandhe-core --example generate_token_docs"
    );
}

#[test]
fn default_token_doc_sections_resolves_without_error_via_generate_token_reference() {
    // `default_token_doc_sections` が返す全セクションについて、参照解決を
    // 含む生成が最後までエラーなく完了することを確認する（既定トークン
    // セットの実装不備・S16 のプラットフォーム別上書き追加時の破壊を検出）。
    let sections =
        default_token_doc_sections().expect("既定トークンセットの構築に失敗しました（実装不備）");
    let markdown = generate_token_reference(&sections)
        .expect("既定セクションからの生成は常にエラーなく完了するべき");

    assert!(markdown.contains("パレット"));
    assert!(markdown.contains("iOS ダーク差分"));
    assert!(markdown.contains("Android ダーク差分"));
}
