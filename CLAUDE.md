# CLAUDE.md

## Overview

fandhe-multi-platform は Rust 製のマルチプラットフォーム対応フレームワーク。iOS / Android / Android TV / iPad / watchOS / Vision Pro / macOS / Windows / Linux / Web へ共通基盤で一括デプロイし、ストア公開用ビルドまで生成することを目指す。

設計思想: (1) ネイティブ同等のパフォーマンス、(2) Rust の特性活用（メモリ安全性・型安全性）、(3) AI 開発への最適化（AI 型安全 DSL）。

仕様書は [Fandhe-AI/fandhe-multi-platform-spec](https://github.com/Fandhe-AI/fandhe-multi-platform-spec) で管理し（非公開）、`docs/spec/` にサブモジュールとして取り込んでいる（アクセス権がある場合のみ `git submodule update --init`。ビルドには不要）。開発は `docs/spec/06-roadmap.md` のマイルストーン M1〜M6 に従う。

## Repository Structure

```
fandhe-multi-platform/
├── CLAUDE.md                    # 開発ガイドライン・委譲方針・規約・subagent 定義
├── README.md                    # プロジェクト概要・セットアップ手順
├── AGENTS.md                    # レビュー観点集（ai-review（provider: codex）が PR の base コミットの本書を基準に読む）
├── skills-lock.json             # 導入スキルの記録（skills add の履歴）
├── Cargo.toml                   # workspace ルート（8 クレート構成）
├── Makefile                     # よく使うコマンドの薄い入口（実体は scripts/ か cargo 1 行。.claude/rules/makefile.md、make skill）
├── lefthook.yml                 # Git hooks 設定（pre-commit・commit-msg）
├── Dockerfile                   # 開発用コンテナ定義（共通コア・Linux・wasm32）
├── compose.yaml                 # docker compose 定義（ローカル開発環境）
├── .editorconfig                # エディタ設定（改行・インデント・末尾空白の統一）
│
├── crates/
│   ├── fandhe-core/             # 共通コア（UI DSL・状態管理・AI 型安全 DSL、forbid(unsafe_code)）
│   │   ├── src/lib.rs           # 役割要約・モジュール地図（ui / state / token / screens）
│   │   └── src/screens/         # AI が生成した画面モジュールの置き場（`ui/mod.rs` には手を入れない）
│   ├── fandhe-device/           # 端末機能（保存・HTTP・カメラ）の抽象と共通実装（Phase 8 / #746、feature `device` で切り離し可能）
│   ├── fandhe-egui-proxy/       # egui による実描画バックエンド（デスクトップ・Android・iOS・Web）
│   ├── fandhe-fonts/            # CJK フォント資産（Noto Sans JP 埋め込み）
│   ├── fandhe-prototype/        # プロトタイプアプリ共通コード（TASK-2.2）
│   ├── fandhe-swiftui-bridge/   # SwiftUI 統合レイヤ（codec・ffi・error）
│   ├── fandhe-web-dom/          # Web ターゲット向け DOM 出力レンダラ（HTML・SEO・a11y）
│   └── fandhe-winit-ime/        # IME 統合レイヤ（winit ← → fandhe-core）
│
├── docs/
│   ├── spec/                    # 仕様書サブモジュール（読み取り専用・別リポ管理）
│   │                             # REQ-N / TASK-N.M・マイルストーン M1〜M6
│   ├── architecture/            # 設計文書（責務分界・IME・トークン・Web・SwiftUI ブリッジ等）
│   ├── environment/             # 開発環境のセットアップ手順
│   ├── device/                  # 端末機能の対応表（SSOT は capabilities/*.json、生成物は git 管理外）
│   ├── ai/                      # AI 向けメタ情報（SCHEMA.md）
│   ├── operations/              # 運用手順書（ビルドパイプライン・署名）
│   ├── ime/                     # IME 使用ガイド
│   └── tokens/                  # デザイントークン（定義・参照ガイド）
│
├── scripts/
│   ├── help.sh / setup.sh / lint.sh / lib.sh  # Makefile 入口の実体（help は Make 非依存の一覧）
│   ├── android/                 # Android ビルド・テスト・検証スクリプト
│   │                             # setup・build・verify・計測（フレームレート・メモリ等）
│   ├── apple/                   # Apple（watchOS・visionOS・iOS・macOS）ビルド・Rust ブリッジ
│   ├── audit/                   # コード品質監査（unsafe-audit.sh）
│   ├── build/                   # プラットフォーム横断ビルド・ストア提出パイプライン
│   ├── ios/                     # iOS シミュレータ向けビルド・実描画確認スクリプト
│   ├── device/                  # 端末機能の対応表生成・人手確認の issue 操作
│   ├── metrics/                 # 行数計測・メモリ計測・バイナリサイズ計測（計測専用 Gradle プロジェクト含む）
│   ├── pr/                      # PR マージガード（guarded-merge.sh・merge-guard-hook.sh）
│   ├── web/                     # Web（wasm32）ビルド・検証・性能計測スクリプト
│   └── windows/                 # Windows ビルド・計測スクリプト（PowerShell）
│
├── patches/                     # 依存クレートへの upstream 未取り込みパッチ（winit 0.30.13 向け IME 修正、docs/architecture/ime-known-constraints.md §3.6）
│
├── platforms/
│   ├── android/                 # Android Gradle プロジェクト（GameActivity ベース）
│   │   └── app/                 # MainActivity ラッパー + fandhe-egui-proxy cdylib
│   └── apple/                   # watchOS / visionOS / iOS Xcode プロジェクト（XcodeGen SSOT）
│       ├── FandheMemBaselineApp/ # iOS メモリ計測ベースラインアプリ（計測専用）
│       └── FandheMemNativeApp/   # iOS メモリ計測ネイティブ比較アプリ（計測専用）
│
├── benchmarks/
│   ├── coldstart/               # コールドスタート計測用ネイティブ SDK 比較アプリ
│   │   ├── ios-native/          # SwiftUI 最小アプリ（XcodeGen プロジェクト）
│   │   └── android-native/      # Jetpack Compose 最小アプリ（独立 Gradle プロジェクト）
│   └── memory/                  # React Native / Flutter 実測比較アプリ
│       ├── react-native/        # RN 最小比較アプリ（検証専用フィクスチャ）
│       └── flutter/             # Flutter 最小比較アプリ（検証専用フィクスチャ）
│
├── .github/
│   ├── CODEOWNERS               # ビルド基盤パス（Dockerfile・compose.yaml・Makefile・.github/ 等）への変更に人間レビューを必須化
│   └── workflows/               # CI ワークフロー（GitHub ホスト ubuntu-latest。codex review のみ self-hosted）
│
└── .claude/
    ├── agents/                  # カテゴリ別 subagent 定義ファイル
    │   ├── research/            # explorer, reference-researcher
    │   ├── implement/           # core-builder, platform-builder, web-builder, build-pipeline-builder
    │   ├── testing/             # test-runner
    │   ├── quality/             # reviewer, security-auditor, linter
    │   └── docs/                # docs-writer
    ├── rules/                   # 運用ルール（委譲・Rust・セキュリティ・CI 等）
    ├── skills/                  # 導入スキル（skills-lock.json で管理）
    ├── workflows/               # implement-issue-tree.js（skills への相対 symlink）
    └── settings.json            # Agent hooks（PreToolUse / PostToolUse）
```

## 委譲方針（必読）

main はオーケストレーションに徹し、調査・実装・レビューは subagent へ委譲して main のコンテキスト消費を抑える。詳細は `.claude/rules/delegation.md`（調査・設計）と `.claude/rules/delegation-impl.md`（作成・編集）を参照。

### パスベース切り替え表（要約）

| 対象 | 調査 | 実装 |
|------|------|------|
| 共通コア（UI DSL・状態管理・AI 型安全 DSL） | explorer | core-builder |
| プラットフォーム統合（iOS / Android / watchOS / visionOS / デスクトップ） | explorer | platform-builder |
| Web（wasm32 / WebGL2） | explorer | web-builder |
| ビルド・CI・配布（.github/ / 署名・ストア提出） | explorer | build-pipeline-builder |
| `docs/spec/`（読み取り専用） | explorer | —（spec リポ側で管理） |
| 外部仕様（wgpu / NDK / ストア要件等） | reference-researcher | — |

### model 配分表

| 用途 | model |
|------|-------|
| 複雑な横断判断・アーキテクチャ設計 | opus または fable（fable は Opus 上位の最上位 tier。大規模設計・複雑な横断判断に限定） |
| 調査・生成・実装・レビュー | sonnet |
| 機械的集計・lint・ドキュメント更新 | haiku |

## Sub-agents

| カテゴリ | subagent_type | model | 役割 |
|---------|--------------|-------|------|
| research | explorer | sonnet | コードベース・docs/spec 横断調査（読み取り専用） |
| research | reference-researcher | sonnet | 外部仕様調査（wgpu / SwiftUI バインド / NDK / ストア要件） |
| implement | core-builder | sonnet | 共通コア（宣言的 UI DSL・状態管理・AI 型安全 DSL） |
| implement | platform-builder | sonnet | プラットフォーム統合レイヤ（Metal / Vulkan 切替・SwiftUI バインド） |
| implement | web-builder | sonnet | Web ターゲット（wasm32 / WebGL2・Safari 対策） |
| implement | build-pipeline-builder | sonnet | クロスビルド・CI・ストア提出パイプライン |
| testing | test-runner | sonnet | cargo test・クロスターゲットビルド検証（修正はしない） |
| quality | reviewer | sonnet | 責務分界・unsafe 境界・API 設計レビュー（読み取り専用） |
| quality | security-auditor | sonnet | OWASP・メモリ安全性・秘密情報混入監査（読み取り専用） |
| quality | linter | haiku | clippy / fmt --check / frontmatter 検査 |
| docs | docs-writer | haiku | CLAUDE.md・README・アーキテクチャ文書の更新 |

## Rules

| ファイル | 内容 |
|---------|------|
| `.claude/rules/delegation.md` | 調査・設計フェーズの委譲原則・パスベース切り替え・model 配分 |
| `.claude/rules/delegation-impl.md` | 作成・編集フェーズの委譲マッピング・検収規約 |
| `.claude/rules/coding-rust.md` | Rust 規約（forbid(unsafe_code) 原則・クレート構成・エラーハンドリング） |
| `.claude/rules/security.md` | OWASP Top 10・秘密情報混入防止・unsafe 監査 |
| `.claude/rules/japanese-style.md` | 日本語出力スタイル（表記・コミット・コード内コメント） |
| `.claude/rules/conventional-commits.md` | Conventional Commits 詳細規約（type / scope / 破壊的変更） |
| `.claude/rules/code-comment-style.md` | コメント規約（役割・責務境界・呼び出し文脈の埋め込み） |
| `.claude/rules/out-of-scope-tracking.md` | スコープ外事項の追跡規約（Issue 確認 → 承認 → 起票） |
| `.claude/rules/ci.md` | CI 規約（**GitHub ホスト runner が原則**・必須チェック名・発火条件・ジョブ追加チェックリスト・キャッシュ方針） |
| `.claude/rules/device-bridge-contracts.md` | 端末機能の橋渡し PR の push 前自己点検（8 類型のチェック表） |
| `.claude/rules/makefile.md` | Makefile / scripts 編集規約（薄い入口・ターゲット追加手順・スクリプト規約） |
| `.claude/rules/git-hooks.md` | Git hooks（lefthook）編集規約（時間予算・CI との役割分担・明示的な例外） |
| `.claude/rules/public-repo.md` | 公開リポジトリの記載規約（Issue・PR・コミット・コード・自動投稿に書かないもの、検証記録の置き場所） |

## Current Skills

`skills-lock.json` で管理。導入元は Fandhe-AI/agent-cli-skills（ワークフロー系）と Fandhe-AI/agent-reference-skills（リファレンス系）。

### ローカル skill（skills-lock.json 管理外）

`expand-capability` — 対応表（`docs/device/capabilities/`）の「できそう」な端末機能を 1 件選び、issue 起票 → implement-issue-tree で実装 → PR 仕上げ → 表の更新確認まで回す。本リポジトリ固有のため upstream へは出さない。

### ワークフロー系

`create-commit` / `create-pr` / `create-issue` / `create-issue-tree` / `create-plan` / `implement-issue` / `implement-issue-tree` / `implement-review` / `implement-review-pr` / `update-issue-tree` / `update-docs` / `update-claude` / `update-reference` / `init-claude` / `comment-code` / `contribute-skill` / `sync-skills-lock`

### リファレンス系

- Rust / フレームワーク: `rust` / `fandhe-backend` / `fandhe-frontend`
- Android: `android-architecture` / `android-background-work` / `android-build-gradle` / `android-compose-components` / `android-compose-graphics-animation` / `android-compose-ui` / `android-data` / `android-media-camera` / `android-navigation` / `android-platform-core` / `android-testing` / `android-wear`
- Apple: `apple-app-services` / `apple-appkit` / `apple-data` / `apple-distribution` / `apple-graphics` / `apple-media` / `apple-ml` / `apple-spatial` / `apple-swift` / `apple-swiftui` / `apple-uikit`
- Windows: `windows-ai` / `windows-app-sdk` / `windows-data-storage` / `windows-design` / `windows-graphics-media` / `windows-interop-modernize` / `windows-packaging-publish` / `windows-platform-integration` / `windows-testing-performance` / `windows-winui-controls` / `windows-winui-ui`
- ツール: `github-docs` / `commitlint` / `lefthook` / `editorconfig`

## Conventions

- **日本語運用**: やりとり・報告・コメントは日本語（`japanese-style.md`）
- **Conventional Commits 厳守**: コミットは create-commit スキル経由。lefthook による自動検査（pre-commit: fmt チェック + .env 混入防止 / commit-msg: Conventional Commits 形式）を通す。clippy・test は CI（`ci.yml`）で実行しフックには置かない（`git-hooks.md`）。**`--no-verify` 禁止**
- **旧 Issue 番号**: 初回コミット `chore: 初回公開` に由来するコード・文書中の `#N` は旧リポジトリ（非公開・アーカイブ）の番号で、本リポジトリの Issue を指さない
- **CI は GitHub ホスト（ubuntu-latest）が原則**: codex review ジョブのみ self-hosted（`ci.md`）
- **セキュリティレビュー**: PR 作成前に security-auditor（OWASP Top 10）を通す
- **ユーザー承認フロー**: implement-issue は計画承認後に実装。スコープ外 Issue の起票・既存ファイルの上書きは承認後のみ
- **spec 準拠**: 要件・タスクは `docs/spec/`（REQ-N / TASK-N.M）に紐付けて進める。spec の変更はこのリポジトリでは行わない
- **実機検証**: 実機・証明書・外部手続きが必要なタスクは「人間担当」としてユーザーへ明示的に依頼する

## hooks（settings.json）

- **PreToolUse**（Bash）: `gh ... pr merge` を含むコマンドを `scripts/pr/merge-guard-hook.sh` が検知し、**origin/main の** `scripts/pr/guarded-merge.sh`（条件確認のみ・マージはしない）で CI ブロッカー 0・未解決スレッド 0・CODEOWNERS 対象 0・`mergeable`・`--match-head-commit` の SHA 一致を確認し、満たさなければ deny する（#949）。許可外オプション（`--admin` 等）・`-R` の不一致・変数展開・`cd` も deny
- **PostToolUse**（Edit|Write）: `.rs` ファイル編集後、rustfmt と jq が存在する場合のみ自動整形。整形は編集ファイルが属するクレートの `Cargo.toml` の `edition` に従う。edition が判定できない場合は整形をスキップする
