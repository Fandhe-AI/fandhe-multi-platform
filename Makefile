# 開発環境整備（REQ-18）: よく使う開発コマンドの入口を一元化する Makefile。
#
# Make は scripts/ 配下のスクリプトか言語ネイティブコマンド（cargo 等）を呼ぶだけの
# 薄い入口であり、処理の定義は scripts/ 側にのみ置く（各レシピは 1 行の呼び出しに限定する。
# TASK-1.5-S1・TASK-1.2・TASK-7.2 の既存実装を再利用する）。
# Make を使わない場合の同等コマンドは scripts/help.sh を参照。
#
# GNU Make 3.81（macOS 標準）で動作する書き方に限る（.ONESHELL・.RECIPEPREFIX 等は使わない）。
#
# 使い方: `make help`（既定ターゲット）でターゲット一覧を表示する。
# 一覧は下記の `## ` コメントから生成されるため、`## ` の無いターゲットは表示されない。
# ターゲットを追加・改名・削除したら scripts/help.sh も同じ変更で更新すること。
# 中身の無いターゲット（`@true` 等）は呼び出し元が終了コードを信用してしまうため置かない。
# 変更時は .claude/rules/makefile.md に従う。

.DEFAULT_GOAL := help

# コンテナ内ユーザーの UID/GID をホストに合わせるための変数（Cursor Bugbot 指摘・
# PR #512 コミット 9d5af38 是正）。Linux ホストでは bind mount（.:/work）に対して
# cargo が直接書き込むため、コンテナ内ユーザーの UID/GID がホストのリポジトリ
# 所有者と一致しないと Permission denied になる。既定でホストの実際の値
# （`id -u`/`id -g`）を自動検出して使う。
#
# 変数名を UID/GID ではなく HOST_UID/HOST_GID にしているのは、UID が bash/zsh の
# 予約済み read-only 変数であり `UID=1000 make ...` のような呼び出しがシェル側で
# エラーになるため（実機確認済み）。上書きしたい場合は
# `make docker-build HOST_UID=2000 HOST_GID=2000` のように Make 変数として渡す。
HOST_UID ?= $(shell id -u)
HOST_GID ?= $(shell id -g)

.PHONY: help setup build test fmt lint wasm android verify-targets docker-build docker-shell clean

help: ## このヘルプを表示する（Make 非依存の一覧: scripts/help.sh）
	@echo "利用可能なターゲット:"
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z0-9_-]+:.*## / {printf "  %-16s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

setup: ## 開発環境の初期セットアップ（submodule 取得・wasm ターゲット追加・lefthook 導入）。直接: scripts/setup.sh
	@scripts/setup.sh

build: ## workspace 全体を cargo build する
	cargo build --workspace

test: ## workspace 全体を cargo test する
	cargo test --workspace

fmt: ## cargo fmt --all でコード整形する
	cargo fmt --all

lint: ## cargo fmt --check と cargo clippy（警告をエラー扱い）で静的検査する。直接: scripts/lint.sh
	@scripts/lint.sh

wasm: ## Web（wasm32）ターゲットのビルド。直接: scripts/web/build-wasm-proxy.sh
	@scripts/web/build-wasm-proxy.sh

android: ## Android クロスビルド。直接: scripts/android/build-android.sh
	@scripts/android/build-android.sh

verify-targets: ## 全対象プラットフォームのターゲット一括コンパイル確認。直接: scripts/build/verify-all-targets.sh
	@scripts/build/verify-all-targets.sh

docker-build: ## 開発用 Docker イメージ（dev サービス）をビルドする（HOST_UID/HOST_GID でコンテナ内ユーザーをホストに合わせる）
	HOST_UID=$(HOST_UID) HOST_GID=$(HOST_GID) docker compose build dev

# イメージ未ビルド時に compose が既定 UID/GID(1000) で暗黙ビルドしないよう、
# docker-build と同じく HOST_UID/HOST_GID を透過する（Bugbot 指摘対応）
docker-shell: ## 開発用 Docker コンテナ（dev サービス）でシェルを起動する
	HOST_UID=$(HOST_UID) HOST_GID=$(HOST_GID) docker compose run --rm dev bash

clean: ## cargo clean でビルド成果物を削除する
	cargo clean
