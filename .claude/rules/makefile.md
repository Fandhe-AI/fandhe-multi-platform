---
paths:
  - "Makefile"
  - "scripts/*.sh"
  - "scripts/{android,web,build}/**"
---

# Makefile / scripts 編集規約

## 目的

Makefile は開発コマンドの**薄い入口**であり、処理の定義は `scripts/` に置く。Make を使えない環境でも同じ操作を
`scripts/help.sh` の一覧から直接実行できる状態を保つ。詳細は make skill（`.claude/skills/make`）を参照する。
`scripts/hooks/**` に相当するフック類は `.claude/rules/git-hooks.md` の担当（本ファイルの対象外）。

## 書き方

- レシピは `@scripts/<name>.sh` の 1 行呼び出し、または言語ネイティブの 1 行コマンド（`cargo build --workspace` 等）に限る
- 複数行・`if` / `for`・環境依存の分岐・`curl` などは `scripts/` のスクリプトへ移す
- 空ターゲット（`@true` 等）を置かない。呼び出し元が終了コードを信用してしまう
- GNU Make 3.81（macOS 標準）で動く書き方に限る（`.ONESHELL`・`.RECIPEPREFIX` 等は使わない）
- 既存ターゲット名・引数・環境変数（`HOST_UID` / `HOST_GID` 等）・終了コードは CI・lefthook・README が参照するため変えない

## ターゲットを追加・改名・削除するとき

1. `.PHONY` に追加し、`## ` コメントを付ける（`make help` はこのコメントから生成される）
2. `scripts/help.sh` の一覧を同じ変更で更新する（自動同期の仕組みは無い）
3. 呼び出し元を洗い出して更新する:
   `grep -rn "make " .github .claude docs README.md CLAUDE.md AGENTS.md lefthook.yml scripts`
4. `make help` と `make -n <target>` で確認する

## 新規スクリプトの規約

- 先頭に役割と使い方のコメントを書き、`set -euo pipefail` を置く。実行権限（`chmod +x`）を付ける
- `scripts/` 直下の入口スクリプトは `scripts/lib.sh`（`ROOT_DIR`・`reject_args`）を source する
- 引数を取らないスクリプトは `reject_args` で未知の入力を終了コード 2 で止める
- 出力メッセージは日本語（既存スクリプトに合わせる）。エラーは標準エラーへ出す
- `shellcheck -x -P SCRIPTDIR scripts/*.sh` を通す。`.editorconfig` に従う

## ターゲットの契約

| ターゲット | 契約 |
|-----------|------|
| `lint` | ソースを変更しない（`fmt --check` + `clippy`）。整形は `fmt` が行う |
| `clean` | `cargo clean` のみ。共有ディレクトリ（`~/.gradle`・Android SDK 等）を消さない |
| `setup` | 再実行しても安全（冪等）。lefthook 未導入は案内だけで失敗にしない |
| `docker-build` / `docker-shell` | `HOST_UID` / `HOST_GID` を compose へ透過する |

## 変更時のチェック

- `make help`・変更したターゲットの `make -n`・`shellcheck`・`editorconfig-checker` を実行する
- Makefile は CODEOWNERS（`/Makefile`）の対象。人間レビューが必須
