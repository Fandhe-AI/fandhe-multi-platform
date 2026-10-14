# Rust コーディング規約

## 安全性（本リポジトリの設計思想の中核）

- 共通コアのクレートには `#![forbid(unsafe_code)]` を付ける
- FFI 境界（プラットフォーム統合レイヤ）で `unsafe` が不可避な場合:
  - 最小範囲の関数・モジュールに閉じる
  - `// SAFETY:` コメントで安全性根拠を必ず明記する
  - 安全な公開 API でラップし、`unsafe` を呼び出し側へ漏らさない

## エラーハンドリング

- ライブラリクレート: `thiserror` 等による型付きエラー。`unwrap()` / `expect()` を本実装コードで使わない
  （テスト・例外的な初期化不変条件のみ `expect("理由")` を許容)
- panic を API 契約にしない。回復可能なエラーは `Result` で返す

## クレート構成

- workspace 構成とし、クレート名は `fandhe-` 接頭辞で統一する
- 二層アーキテクチャを クレート境界で表現する（共通コア ⇔ プラットフォーム最適化レイヤ）
- 共通コアからプラットフォーム固有クレートへ依存しない（依存方向は 最適化レイヤ → コア の一方向）
- feature フラグは加法的（additive）に設計し、feature の組み合わせでビルドが壊れないようにする

## スタイル

- `cargo fmt` 準拠（rustfmt 既定設定。変更する場合は rustfmt.toml をリポジトリに置く）
- `cargo clippy --all-targets --all-features` の警告ゼロを維持する
- 命名: クレート・モジュールは snake_case、型・トレイトは UpperCamelCase、Rust API Guidelines に従う
- 公開 API には `///` ドキュメンテーションコメント必須（code-comment-style.md 参照）

## テスト

- ユニットテストは同一ファイルの `#[cfg(test)] mod tests`、結合テストは `tests/` に置く
- クロスターゲットに関わる変更は対象ターゲットのビルド確認（`cargo build --target ...`）まで行う

## 依存管理

- 依存クレートは workspace の `[workspace.dependencies]` で一元管理する
- バージョンを固定しすぎない（キャレット既定）。セキュリティ勧告のあるバージョンは避ける
