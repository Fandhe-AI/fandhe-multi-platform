# 委譲ルール（作成・編集フェーズ)

## 原則

コードの作成・編集は対象レイヤの builder Agent へ委譲する。main は指示の明確化・結果の検収・
ユーザーへの報告に徹する。1 つの変更が複数レイヤにまたがる場合はレイヤ単位に分割して順に委譲する。

## 委譲マッピング

| 変更対象 | 担当 Agent | 例 |
|---------|-----------|----|
| 共通コア（UI DSL・状態管理・AI 型安全 DSL） | core-builder | `Screen<S>` / `Node<S>` 実装、レイアウト |
| プラットフォーム統合（iOS / Android / watchOS / visionOS / macOS / Windows / Linux） | platform-builder | Metal / Vulkan 切替、SwiftUI バインド |
| Web（wasm32 / WebGL2） | web-builder | wasm ビルド、Safari 対策 |
| ビルド・CI・配布（.github/ / クロスビルド・署名） | build-pipeline-builder | cargo-ndk、ストア提出パイプライン |
| テスト実行・ビルド検証（コード修正なし） | test-runner | cargo test、クロスターゲット検証 |
| レビュー（読み取り専用） | reviewer / security-auditor | コミット前・PR 前レビュー |
| 機械的 lint | linter | clippy / fmt --check |
| ドキュメント（CLAUDE.md / README / アーキ文書） | docs-writer | スキル一覧更新、責務分界図 |

## 検収規約

- builder の報告には「変更ファイル一覧・ビルド / テスト結果・未解決事項」を含めさせる
- テスト失敗を残したまま完了扱いにしない（失敗はユーザーへそのまま報告する）
- スコープ外事項が報告された場合は out-of-scope-tracking.md のフローに載せる

## コミット・PR

- コミットは create-commit スキル、PR は create-pr スキルを使用する（Conventional Commits 厳守）
- コミット前に reviewer / security-auditor の確認を通す（implement-review スキル）
