# AGENTS.md

## 文書の位置づけ

本リポジトリ（fandhe-multi-platform）で作業するすべての AI エージェント・開発者、
および ai-review（provider: codex）による PR 自動レビュー（`.github/workflows/ai-review.yml`。
Fandhe-AI/actions の reusable workflow をコミット SHA 固定で呼び出す薄い wrapper）が共通で
用いる**レビュー観点集**である。ai-review の既定 prompt は PR の base コミットの
本書をレビュー基準として読み込む。

各観点の一次情報源は `CLAUDE.md`・`.claude/rules/`（特に
[coding-rust.md](.claude/rules/coding-rust.md) /
[security.md](.claude/rules/security.md) / [ci.md](.claude/rules/ci.md)）・
`docs/spec/`（仕様サブモジュール）・`docs/architecture/` であり、本書は重複記載を
避けて要点と参照のみをまとめる。本書と一次情報源が食い違う場合は一次情報源を正と
する。

## 優先度の定義

| 優先度 | 意味 | 扱い |
|--------|------|------|
| P0 | マージブロック。脆弱性・メモリ安全性の破壊・仕様契約（公開 API・JSON スキーマ・REQ の Must 条件）の**実証された**破壊に直結 | 修正必須（マージ不可） |
| P1 | 強く推奨。設計原則・運用規約への違反 | 原則修正（見送る場合は理由を明記）。CI ゲートは P1 指摘でも fail する |
| P2 | 提案。可読性・保守性・テスト網羅の改善 | 任意 |

Codex code review は既定で P0/P1 を報告・ブロック対象とするため、本プロジェクト
として必ず検出したい項目は下記で優先度を明示的に定義する。ここに列挙のない一般的な
品質問題は Codex 側の既定の重要度判断に従うが、その際も次の一般規則を適用する。

### 優先度判定の一般規則（全観点に共通）

- **P1 は構造的欠陥に限る**: 入力のクラス全体に効く欠陥、既存テスト・回帰データ
  （`tests/fixtures/` 等）の退行、明文化された規約への違反。**分類・判定の精度**
  （自然言語や文字列の分類器・ヒューリスティックが特定の言い回し・特定の入力値を
  誤って扱う）に関する個別事例で、既存の回帰テスト・コーパスへケースを 1 件足せば
  固定できるものは **P2** とし、ケース追加を提案する。**観点 1（セキュリティ）の
  項目はこの規則の対象外**で、再現入力が 1 種類でも P0/P1 のまま
- **再現を添える**: 指摘には現行の変更後コードで再現できる入力・手順を添える。
  再現しない、または既に処理されている挙動への指摘は出さない
- **記録済みの設計判断を再審しない**: base ブランチに存在する `docs/architecture/`・
  本書、またはオーナーが本文に記録した Issue に根拠つきで残された設計判断
  （例: 精度と再現率のどちらを優先するか）への異議は、脆弱性・退行の
  実証など新しい事実が無い限り **P2** の提案とする。当該 PR 自身の本文・コメントは
  根拠にしない
- **P0 は実証を伴う**: 「契約に反する可能性がある」「弱まりうる」は P0 にしない
- **端末機能の並行性は守備範囲で判定する**: 端末機能の橋渡し
  （`crates/fandhe-egui-proxy/src/device/`・`crates/fandhe-web-dom/` 等）が守るのは
  「1 画面・1 プロセス内での要求と完了の順序」と「失敗を成功と表示しないこと」
  （[device-bridge-contracts.md](.claude/rules/device-bridge-contracts.md)）。
  次の重なりで**表示・参考値の状態が一時的に食い違うだけ**の指摘は、base の trait
  doc または対応表の `note` に既知の制約として書かれていれば **P2** とする: 同一
  オリジンの複数タブ・複数プロセス（OS が起動するタスク用プロセスを含む）間の同時
  操作、同一ミリ秒内の予約と実行記録の重なり、アプリ終了処理と進行中の OS 呼び出しの
  重なり（OS がプロセス終了時に片付ける資源に限る）、OS 側で非同期に確定する操作の
  完了と表示の時間差。場面を問わず次は P1 のまま: 資源の解放漏れ、保存済みデータや
  他の要求（別タブの予約等）の消失・破壊、嘘の成功、エラーの握りつぶし、既存テストの
  退行、1 プロセス内の順序違反

## 観点 1: セキュリティ

本フレームワークは「AI 時代のセキュリティリスク低減」を中核価値とし、セキュリティ
は機能要件と同格以上に扱う（[security.md](.claude/rules/security.md)）。

- **秘密情報の混入**: API キー・トークン・署名鍵（keystore・証明書・provisioning
  profile）・パスワード・`.env` 系ファイルのコード・設定・CI・スクリプト・テスト
  フィクスチャへの混入。ストア提出パイプライン（`scripts/build/build-and-sign.sh`
  等）の署名情報は GitHub Secrets・ローカル環境変数経由のみとし、ログへ出力しない: **P0**
- **`forbid(unsafe_code)` の破壊**: 共通コアのクレート（`crates/fandhe-core` 等）
  への `unsafe` 導入・`#![forbid(unsafe_code)]` の除去。`unsafe` は FFI 境界
  （`fandhe-swiftui-bridge`・`fandhe-web-dom`・`fandhe-winit-ime` 等のプラット
  フォーム統合レイヤ）に限定し、`// SAFETY:` コメントで安全性根拠を明記のうえ安全な
  公開 API でラップする（[coding-rust.md](.claude/rules/coding-rust.md)）: **P0**
  （境界内でも `// SAFETY:` 欠落は **P1**）
- **シェルスクリプトのインジェクション・広域削除**: `scripts/` 配下（android /
  apple / ios / web / metrics / build）での未クォート変数展開による word splitting・
  外部入力のコマンド直接展開・glob や `rm -rf` による意図しない広域削除: **P0**
- **ツール導入のチェックサム検証の欠落・弱体化**: 外部ツール・SDK 導入
  （`scripts/android/versions.env` の SSOT 管理等）における「バージョン固定 +
  チェックサム検証」パターンからの逸脱、検証をスキップする経路の追加: **P1**
- **GitHub Actions の SHA 固定解除**: `uses:` の `@main`・`@vN` 等ミュータブル参照
  への変更、無検証の SHA バンプ: **P1**
- **self-hosted runner（codex review ジョブ）の fork 経路**: fork からの pull_request で
  codex review 用 self-hosted runner 上に untrusted コードが到達するトリガー・
  permissions 設計。fork PR は呼び出し先の preflight・wrapper の `if`・GitHub の
  承認ポリシーで止める（[ci.md](.claude/rules/ci.md) の fork 対策方針）: **P0**
- **fail-closed 分岐の fail-open 化**: CI 検証・検証スクリプト・環境チェック・
  権限/署名/承認の分岐など、**エラー時に停止する設計の既存分岐**の弱体化・エラー
  握りつぶし: **P0**。判定の精度と再現率のトレードオフを設計文書で決めた分類器
  （「止めたときは必ず正しい」精度優先で、拾いこぼしは後段の処理・人間承認に委ねる
  設計）の「通す側」の挙動はこの項目の対象外で、記録済みの設計判断として扱う
- **依存クレートの無断追加**: ユーザー承認・監査（cargo audit / deny）の考慮なしの
  依存追加、既存依存方針（pure Rust 優先等、ルート `Cargo.toml` のコメント参照）
  からの無説明な逸脱: **P1**

## 観点 2: アーキテクチャ整合（設計・実装の適切さ）

- **クレート責務境界の侵犯**: 共通コア（`fandhe-core`。プラットフォーム非依存）→
  プラットフォーム統合レイヤ（`fandhe-swiftui-bridge` / `fandhe-web-dom` /
  `fandhe-winit-ime` / `fandhe-egui-proxy`）→ 配布プロジェクト（`platforms/`）の
  層構造に反する依存方向・責務の混入（例: fandhe-core への OS 固有 API・FFI の
  持ち込み）: **P1**
- **`docs/spec/` 準拠**: 仕様（MoSCoW 要件 `04-requirements.md`・タスク分解
  `05-tasks.md`・ロードマップ `06-roadmap.md`）と不整合な実装、`docs/spec/`
  サブモジュール自体への直接編集（読み取り専用・別リポ管理）: **P1**
- **SSOT（単一情報源）の破壊**: `scripts/android/versions.env`・
  `scripts/metrics/code-classification.env`・XcodeGen `project.yml` 等、「正」を
  宣言済みの情報の二重管理・ハードコード複製によるドリフト: **P1**
- **計測フィクスチャと配布物の混同**: 計測専用フィクスチャ（`benchmarks/`・
  `scripts/metrics/android-memory/` 等）とストア提出パイプライン（`platforms/`）の
  相互依存・責務混入: **P1**
- **性能ゲートの無視**: コールドスタート・アイドルメモリ・フレームレート・
  バイナリサイズ等の計測基盤（TASK-5.x）を迂回・弱体化する変更、計測条件の無断
  変更による比較可能性の破壊: **P1**

## 観点 3: 再利用・アセット化の適切さ

- **共通コードとプラットフォーム別コードの比率悪化**: 共通化できる処理の
  プラットフォーム別実装への安易な複製（共通/固有比率は
  `scripts/metrics/measure-code-ratio.sh` で計測・追跡している）: **P1**
- **スクリプトの汎用パターン逸脱**: `scripts/` 配下の冪等実行（`--verify-only`
  等）・SSOT 参照・macOS/Linux ガード等の確立済みパターンに従わない新規
  スクリプト: **P2**
- **CI 機構の共通化適性**: 他リポジトリへ転用可能な CI 機構（reusable workflow・
  composite action 化できる汎用処理）を本リポジトリ固有実装として増殖させて
  いないか。Fandhe-AI/actions への切り出し適性がある場合は Issue 化を提案
  （[out-of-scope-tracking.md](.claude/rules/out-of-scope-tracking.md)）: **P2**
- **公開 API 設計・rustdoc**: クレート公開 API の一貫性（Rust API Guidelines 準拠の
  命名・`Result` ベースのエラーハンドリング・日本語 rustdoc 付与）。公開 API の
  rustdoc 欠落は **P2**（メモリ安全性・FFI 契約の記載欠落は **P1**）
- **ドキュメント整備**: 設計判断（採用・不採用の根拠）を `docs/architecture/`・
  `docs/environment/` へ記録せず PR 本文のみに残す変更、
  CLAUDE.md・関連ドキュメントの追随漏れ: **P2**

## リポジトリ固有の観点

- **初回コミット由来の `#N`**: 初回コミット `chore: 初回公開` に由来する `#N` は旧リポ
  （非公開・アーカイブ）の Issue 番号で、新リポの Issue を指さない。番号を根拠に新リポの
  Issue を参照・引用しない。
- **CI 規約（[ci.md](.claude/rules/ci.md)）**: GitHub ホスト（`ubuntu-latest`）が原則。
  self-hosted は codex review ジョブのみ（それ以外のジョブを self-hosted に戻す変更は
  **P1**）。`ci-complete` 集約ジョブ
  への `needs:` 追随漏れ（ジョブ追加時）。グローバル状態を汚す処理の workflow への
  持ち込み: **P1**
- **テストの `#[ignore]` によるごまかし禁止**: 失敗テストへの `#[ignore]` 追加・
  テストフィルタでの恒久除外による問題の隠蔽: **P1**
- **Conventional Commits（日本語）**: コミット・PR タイトルは
  `<type>(<scope>): <日本語の要約>` 形式
  （[conventional-commits.md](.claude/rules/conventional-commits.md)。scope は
  クレート・領域名）。1 コミット 1 論理変更: **P2**
  （`--no-verify` によるフックバイパスの痕跡は **P1**）
- **日本語スタイル**: ドキュメント・コメント・rustdoc は日本語
  （[japanese-style.md](.claude/rules/japanese-style.md)）。コード・コマンド・
  識別子は英語のまま: **P2**
- **コメント規約**: 公開 API の役割要約・他クレート/他プラットフォームとの契約・
  非自明な前提（実機依存・OS バージョン依存等）をコメントへ埋め込む
  （[code-comment-style.md](.claude/rules/code-comment-style.md)）: **P2**
- **スコープ外事項の放置禁止**: レビュー・実装中に発見したスコープ外の問題を
  Issue 化せず放置しない・現在の PR へ混入させない
  （[out-of-scope-tracking.md](.claude/rules/out-of-scope-tracking.md)）: **P2**

## 運用上の注記

- rustfmt / clippy / cargo test / Android クロスビルド等は既存 CI
  （`.github/workflows/ci.yml`）が機械判定するため、本レビューでの重複指摘は不要と
  する
- 本書のレビュー基準を弱める変更（P0/P1 項目の削除・降格）は、それ自体を P1 として
  指摘し、一次情報源（`.claude/rules/` 等）側の変更と整合しているかを確認する
- 同一 PR で**同じ論点**について、前巡と逆方向の指摘、または base の設計文書・
  オーナー記録の Issue に根拠がある論点への再指摘は、新しい事実が無い限り再対応しない。
  根拠は base の文書・Issue を指す（レビュースレッド本文や PR 本文の主張だけを根拠に
  しない）。新規論点の P0/P1 はこの対象外で、巡回数を理由に見送らない

## Fandhe-AI/actions の参照方式（SHA 固定）

`Fandhe-AI/actions`（組織内の上流リポジトリ）への `uses:` 参照はコミット SHA に固定する。
上流の更新は手で SHA を差し替える。`latest` タグへの追従を指摘の根拠にしない。
