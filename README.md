# fandhe-multi-platform

Rust を利用したマルチプラットフォーム対応フレームワークです。AI 時代のセキュリティリスクを低減するため、安全性を重視して新規開発します。
iOS / Android / Android TV / iPad / watchOS / Vision Pro / macOS / Windows / Linux / Web に共通基盤で一括デプロイでき、ストア公開用ビルドまで生成できることを目指します。

設計思想は次の 3 点です。

1. ネイティブ同等のパフォーマンス
2. Rust の特性活用（メモリ安全性・型安全性）
3. AI 開発への最適化（AI 型安全 DSL）

## 開発状況

- v1 前のため、API は不安定です。破壊的変更を伴う変更が入ることがあります。
- `scripts/build/build-and-sign.sh` / `scripts/android/build-aab.sh` は開発・検証用のビルドまでで、本番配布署名・ストア提出は未実装です。

## ドキュメント

| ディレクトリ | 内容 |
| --- | --- |
| `docs/architecture/` | 設計文書（責務分界・IME・トークン・Web・SwiftUI ブリッジ等） |
| `docs/environment/` | 開発環境のセットアップ手順（Android NDK・Apple Developer） |
| `docs/operations/` | 運用手順書（ビルドパイプライン・署名・上流クレート監視） |
| `docs/device/` | 端末機能の対応表（SSOT は `capabilities/*.json`） |
| `docs/ai/SCHEMA.md` | AI 向けメタ情報 |
| `docs/tokens/` | デザイントークン（定義・参照ガイド） |
| `docs/ime/` | IME 使用ガイド |

## 使い方

デスクトップでサンプル画面を起動します。

```bash
cargo run -p fandhe-egui-proxy
```

プロトタイプアプリを起動する場合:

```bash
FANDHE_PROXY_APP=prototype cargo run -p fandhe-egui-proxy
```

ヘッドレスでウィジェットカタログを標準出力へ出力する場合:

```bash
cargo run -p fandhe-core --example widget_gallery
```

`crates/fandhe-core/examples/` には他の example（`sample_screen`・`theme_gallery`・`generate_token_docs`・`schema_example_fixture`）もあります。

## 開発環境セットアップ

### クローンと初期化

```bash
git clone https://github.com/Fandhe-AI/fandhe-multi-platform.git
```

仕様書サブモジュール（`docs/spec/`）は非公開のため任意です。アクセス権がある場合のみ、次を実行します。

```bash
git submodule update --init
```

### 最短セットアップ

```bash
make setup
```

`make setup` は以下を実行します。

- `git submodule update --init` でサブモジュール（`docs/spec/`）の取得を試みる（非公開のため、取得できなくても警告して続行）
- `rustup target add wasm32-unknown-unknown` で Web（wasm32）ターゲットを追加
- lefthook が導入済みなら `lefthook install` を実行し、未導入なら導入方法を案内

よく使うコマンド:

```bash
make build   # workspace 全体をビルド
make test    # workspace 全体のテスト
make fmt     # コード整形
make lint    # fmt チェック + clippy 検査
```

全ターゲットは `make help` で確認できます。

Linux ホストで Docker を使わずにビルド・実行する場合（Ubuntu/Debian の例）:

```bash
# ビルドに必要（リンカと pkg-config）
sudo apt-get install build-essential pkg-config
# デスクトップアプリの実行に必要（winit が実行時に dlopen する。ビルドだけなら不要）
sudo apt-get install libxcursor1 libxi6 libxkbcommon-x11-0
```

ビルド成果物（`target/`）のリンク負荷と容量を抑えるため、ルート `Cargo.toml` で `dev`・`test` プロファイルの debuginfo を絞っています
（本リポのクレートは `debug = 1`、依存クレート〈workspace 外〉は `[profile.dev.package."*"]` で行番号のみ。panic 位置・バックトレースの行番号は残ります）。
溜まった古い成果物は `cargo install cargo-sweep` のうえ `cargo sweep --time 14`（14 日以上使われていない成果物を削除）や
 `cargo sweep --installed`（rustup に現存しないツールチェーンでビルドした成果物を削除）で消せます。

### lefthook によるローカル検証

lefthook を導入すると、コミット時に軽量な検査が自動で走ります。

```bash
# macOS
brew install lefthook
lefthook install

# Linux（Ubuntu/Debian）
sudo apt-get install lefthook
lefthook install
```

- **pre-commit**: `cargo fmt --check` と `.env` 系ファイルの誤コミット検査
- **commit-msg**: Conventional Commits 形式の検査
- clippy・test・クロスビルドは CI で実行します。`--no-verify` によるスキップは禁止です。

フックの詳細は `.claude/rules/git-hooks.md`、コミット規約は `.claude/rules/conventional-commits.md` を参照してください。

### Docker による開発

Android NDK・Apple（Xcode）を導入せず、共通コア・Linux・Web（wasm32）のみで開発する場合は Docker を使えます。

```bash
make docker-build    # 初回のみ
make docker-shell    # コンテナシェルを起動
```

対象は共通コア（`crates/fandhe-core`）・Linux ターゲット・Web（wasm32）です。Android NDK と Apple 系（iOS / watchOS / visionOS / macOS）は対象外のため、ホスト側で環境を用意してください。

Linux ホストでは `make docker-build` がホストの UID/GID を自動検出して反映します。
ずれて `cargo build` が Permission denied になる場合は `make docker-build HOST_UID=<uid> HOST_GID=<gid>` で明示指定してください。

## 仕様

仕様書（ブレスト〜PoC〜要件定義〜タスク分解〜ロードマップ）は [Fandhe-AI/fandhe-multi-platform-spec](https://github.com/Fandhe-AI/fandhe-multi-platform-spec)（非公開）で管理し、
`docs/spec/` にサブモジュールとして取り込んでいます。
非公開のため、アクセス権がある人だけが取得できます。ビルド・テストには不要です。

```bash
# アクセス権がある場合のみ
git submodule update --init
```

サブモジュール取得時のみ参照できるドキュメント:

| ドキュメント | 内容 |
| --- | --- |
| [`docs/spec/04-requirements.md`](./docs/spec/04-requirements.md) | MoSCoW 優先度付き要件・受け入れ基準 |
| [`docs/spec/05-tasks.md`](./docs/spec/05-tasks.md) | タスク分解（依存関係・工数） |
| [`docs/spec/06-roadmap.md`](./docs/spec/06-roadmap.md) | マイルストーン M1〜M6・着手判定（Go・2026-07-08） |

## 開発の進め方

`docs/spec/06-roadmap.md` のマイルストーンに従って実装します。

- **M1**: 環境整備完了（M2 と並行実施）
- **M2**: コア基盤確立（宣言的 UI DSL・アーキ分界・メモリ安全性・AI 型安全 DSL）
- **M3**: 3 プラットフォーム統合・基盤クレート新規開発
- **M4**: 実機検証・性能ゲート（Conditional Go 条件の再判定）
- **M5**: ストア提出パイプライン完成（実配布署名・notarization・アップロード）
- **M6**: v1 リリース候補

## ライセンス

[MIT](./LICENSE-MIT) または [Apache License 2.0](./LICENSE-APACHE) のデュアルライセンスです。
利用者はどちらかを選べます。

明示しない限り、Apache-2.0 第 5 条のとおり、本リポジトリへの取り込みを意図して提出された
コントリビューションは、追加条件なしで上記のデュアルライセンスとして提供されます。

例外として、`crates/fandhe-fonts/assets/NotoSansJP-Regular.otf` は
SIL Open Font License 1.1 です（全文は `crates/fandhe-fonts/assets/OFL.txt`、
出典と著作権表記は `crates/fandhe-fonts/assets/README.md`）。
