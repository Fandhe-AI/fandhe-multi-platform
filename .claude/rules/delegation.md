# 委譲ルール（調査・設計フェーズ)

## 原則

main（このセッション）はオーケストレーションに徹し、ファイルの読み込み・横断検索・外部調査は
subagent へ委譲して main のコンテキスト消費を抑える。

- 複数ファイルにまたがる調査・「どこにあるか」探し → **explorer**
- 外部仕様（wgpu / SwiftUI バインド / NDK / ストア要件等）の調査 → **reference-researcher**
- main が直接 Read してよいのは、対象ファイルが特定済みで 1〜2 ファイルに閉じる場合のみ

## パスベース切り替え表

| 対象パス | 調査担当 | 備考 |
|---------|---------|------|
| `crates/`（共通コア） | explorer | 結果は core-builder への実装指示に使う |
| `platforms/` / ターゲット別ディレクトリ | explorer | platform-builder / web-builder への前段調査 |
| `.github/workflows/` / ビルドスクリプト | explorer | build-pipeline-builder への前段調査 |
| `docs/spec/`（サブモジュール・読み取り専用) | explorer | REQ-N / TASK-N.M / M1〜M6 の該当箇所特定 |
| リポジトリ外（公式ドキュメント・crates.io） | reference-researcher | 一次情報の URL を必ず添えさせる |
| `.claude/`（Agent・Rules・Skills） | main 直接可 | 小規模なため委譲不要 |

## model 配分

| 用途 | model |
|------|-------|
| 複雑な横断判断・アーキテクチャ設計 | opus または fable（fable は Opus 上位の最上位 tier。大規模設計・複雑な横断判断に限定） |
| 調査・生成・実装・レビュー | sonnet |
| 機械的集計・lint・ドキュメント更新 | haiku |

## 設計フェーズ

- アーキテクチャ設計・責務分界の判断は main（または opus / fable の Plan 系 Agent）で行い、
  調査材料の収集は explorer / reference-researcher に委譲する
- 設計判断の根拠は `docs/spec/` の要件・PoC 判定に紐付けて記録する
