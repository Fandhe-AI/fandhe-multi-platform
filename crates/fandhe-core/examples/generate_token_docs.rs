//! デザイントークンリファレンス（`docs/tokens/design-token-reference.md`）を
//! コード上のトークン定義から生成し書き出す実行可能 example（TASK-8.4-S20、
//! #176）。
//!
//! `crates/fandhe-core/src/token/docgen.rs` が持つ純粋な生成ロジック（ファイル
//! I/O なし）をこの example が呼び出し、固定パスへ書き出す唯一の窓口を担う
//! （二層アーキテクチャ上、ファイル書き出しはコア本体の責務に含めない）。
//! `tests/token_docgen.rs`（鮮度検証テスト）はこの example と同じセクション
//! 構築関数（`default_token_doc_sections`）を共有するため、両者は常に同じ
//! 内容を生成する。
//!
//! 実行方法: `cargo run -p fandhe-core --example generate_token_docs`
//!
//! 書き出し先は `env!("CARGO_MANIFEST_DIR")` 起点の固定パスのみで、外部入力
//! （引数・環境変数）をパスへ展開しない（OWASP A03 対策、security.md）。

use std::fs;
use std::path::PathBuf;

use fandhe_core::token::{default_token_doc_sections, generate_token_reference};

fn main() {
    let sections =
        default_token_doc_sections().expect("既定トークンセットの構築に失敗しました（実装不備）");
    let markdown = generate_token_reference(&sections)
        .expect("Markdown リファレンスの生成に失敗しました（実装不備）");

    // `crates/fandhe-core` から見てリポジトリルート直下の `docs/tokens/` へ
    // 書き出す。外部入力を経由しない固定相対パスであり、`CARGO_MANIFEST_DIR`
    // という信頼済みのビルド時定数のみを起点とする。
    let output_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/tokens/design-token-reference.md");

    fs::write(&output_path, &markdown).unwrap_or_else(|error| {
        panic!(
            "{} への書き出しに失敗しました: {error}",
            output_path.display()
        )
    });

    println!("生成しました: {}", output_path.display());
}
