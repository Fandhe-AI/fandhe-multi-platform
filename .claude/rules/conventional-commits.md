# Conventional Commits 規約

## 形式

```
<type>(<scope>): <subject>

<body>

<footer>
```

## type

| type | 用途 |
|------|------|
| feat | 機能追加 |
| fix | バグ修正 |
| docs | ドキュメントのみの変更 |
| style | 挙動に影響しない整形（fmt 等） |
| refactor | 機能変更を伴わない構造変更 |
| perf | 性能改善 |
| test | テストの追加・修正 |
| build | ビルドシステム・依存関係の変更 |
| ci | CI 設定の変更 |
| chore | その他（リポジトリ運用等） |

## scope

変更対象のレイヤ・クレートを書く。クレートが対象ならクレート名（`fandhe-` 接頭辞を除いた部分）を使う。

例: `feat(core): 宣言的 UI DSL の Screen/Node を追加`、`ci(android): cargo-ndk クロスビルドを追加`、
`chore(skills): スキル定義を更新`

## 破壊的変更

- subject の type に `!` を付け（`feat(core)!: ...`）、footer に `BREAKING CHANGE: 内容` を記載する

## 運用ルール

- 1 コミット 1 論理変更。無関係な変更を混ぜない
- **`--no-verify` は禁止**。pre-commit フックを必ず通す
- コミット作成は create-commit スキルを使用する（type/scope 推定・シークレット検査込み）
- Issue に紐づく場合は footer に `Refs: #<番号>` または `Closes: #<番号>` を記載する
