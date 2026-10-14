#!/usr/bin/env bash
# Make を介さない操作一覧。`make help` は Makefile の `## ` コメントから
# 機械的に生成されるのに対し、本ファイルは手動で保守する一覧であり、
# 両者を自動で同期する仕組みは無い。Makefile のターゲットを追加・改名・
# 削除したら同じ変更で本ファイルも更新すること。
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./lib.sh
source "${DIR}/lib.sh"

reject_args "$(basename "$0")" "$@"

cat <<'EOF'
このリポジトリの操作と、Make を使わずに直接実行する方法:

  help            scripts/help.sh                          この一覧を表示する
  setup           scripts/setup.sh                         submodule 取得・wasm ターゲット追加・lefthook 導入
  build           cargo build --workspace                  workspace 全体をビルドする
  test            cargo test --workspace                   workspace 全体をテストする
  fmt             cargo fmt --all                          コード整形する
  lint            scripts/lint.sh                          cargo fmt --check + cargo clippy（警告をエラー扱い）
  wasm            scripts/web/build-wasm-proxy.sh          Web（wasm32）ターゲットをビルドする
  android         scripts/android/build-android.sh         Android クロスビルド
  verify-targets  scripts/build/verify-all-targets.sh      全対象プラットフォームのターゲット一括コンパイル確認
  docker-build    HOST_UID=$(id -u) HOST_GID=$(id -g) docker compose build dev
                                                           開発用 Docker イメージをビルドする
  docker-shell    HOST_UID=$(id -u) HOST_GID=$(id -g) docker compose run --rm dev bash
                                                           開発用 Docker コンテナでシェルを起動する
  clean           cargo clean                              ビルド成果物を削除する

`make <target>` は上記と同じコマンドを呼ぶだけの薄い入口であり、
複数行・条件分岐を要する処理の定義は scripts/ 側にのみ存在する。
EOF
