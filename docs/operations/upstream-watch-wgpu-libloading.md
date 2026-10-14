# 運用ルール: `wgpu`/`libloading` の watchOS/visionOS 対応状況の継続監視

- 対応: REQ-3 受け入れ基準 5 項目目・NFR-6（クレート対応状況の監視）・TASK-3.6（M3）、Issue #74
- 前提タスク: TASK-3.1（Issue #56、CLOSED。検証結果は Issue #56 を参照）

## 1. 目的と位置付け

PoC-2（`docs/spec/03-poc/watchos-visionos/README.md`）で、`wgpu` は watchOS で Metal API
自体が非提供のため独自レンダラが原理的に非対応、visionOS も依存クレート `libloading` が
ターゲットの `dlopen` 定数を認識せずビルド不能である（2026-07-08 時点の観測。
`wgpu` 0.20 系・`libloading` 0.7.4 に基づく制約）。この制約は upstream の対応により将来
変化しうるため、REQ-3 は受け入れ基準 5 項目目として「`wgpu`/`libloading` の watchOS/visionOS
対応状況を継続的に監視する運用ルールを定めること」を求めている（NFR-6 の一部）。

本文書はその運用ルールを定める。NFR-6 は `wgpu`/`libloading`/`winit`/`accesskit` の
watchOS・visionOS・Web 対応状況の監視を対象とするが、本文書がカバーするのは TASK-3.6 の
スコープである `wgpu`/`libloading`（watchOS・visionOS）のみである。`winit`/`accesskit` の
watchOS/visionOS 対応が発展途上である制約を REQ-3 の網羅範囲・優先度へ反映する作業は
TASK-8.6（Issue #212）が別途担当するため、本文書では重複管理しない。

## 2. 監視対象と現状ベースライン

| クレート | 対象 OS | PoC-2 時点の制約 | 制約の性質 |
|---------|--------|-----------------|-----------|
| `wgpu` | watchOS | Metal API 自体が非提供のため独自レンダラは原理的に非対応 | OS 側の API 非提供（upstream 対応では解消不能な制約） |
| `wgpu` | visionOS | 依存クレート `libloading` 経由でビルド不能 | 依存クレートの制約に起因（upstream 対応で変化しうる） |
| `libloading` | visionOS | ターゲットの `dlopen` 定数を認識せずビルド不能 | クレート側の対応不足（upstream 対応で変化しうる） |

**バージョン確認**（本文書作成時点、`cargo tree` による解決結果。ルート `Cargo.toml` の
`[workspace.dependencies]` で `wgpu = "27"` を指定し、`eframe`/`egui_kittest` も同じ
27 系へ解決されることを確認済み）:

- `wgpu`: 27.0.1（PoC-2 時点の 0.20 系から大きく進行）
- `libloading`: 0.8.9（PoC-2 時点の 0.7.4 から進行）

**PoC-2 制約の現行版での再現確認**: 未実施。本リポジトリの開発環境は Linux（macOS 実機・SDK
なし）であり、TASK-3.1 で局所導入した Rust 1.96.0 ＋ 4 ターゲット（`aarch64-apple-watchos` /
`-watchos-sim` / `aarch64-apple-visionos` / `-visionos-sim`）を用いて
`fandhe-egui-proxy`（wgpu 依存の検証専用プロキシ）の visionOS 向けクロスビルドを試行したが、
`wayland-sys`（Linux 専用の依存経路）のビルドスクリプトが `pkg-config` のクロスコンパイル
未設定で失敗し、`libloading`/`dlopen` 定数の制約そのものを再現するところまで到達できなかった
（環境起因の失敗であり、PoC-2 制約の解消・継続いずれの確認にもならない）。したがって
「バージョンは進行しているが、制約が現行版でも継続しているかは未確認」というのが現時点の
正確な状態である。次回監視サイクルで、macOS 実機環境（TASK-18.1）が整備され次第、
`fandhe-core` 単体（TASK-3.1 実績あり）に加えて `fandhe-egui-proxy`（wgpu 依存経路）での
再現確認を優先的に実施する。

## 3. 監視方法

一次情報を次の順で確認する。

1. **wgpu**（gfx-rs/wgpu）
   - リポジトリ: https://github.com/gfx-rs/wgpu
   - 確認内容: watchOS / visionOS 関連の issue・PR・CHANGELOG（Metal バックエンドの
     watchOS 対応可否、visionOS ビルドの成否）
2. **libloading**（nagisa/rust_libloading）
   - リポジトリ: https://github.com/nagisa/rust_libloading
   - 確認内容: `aarch64-apple-visionos*` ターゲットでの `dlopen` 定数対応 issue・PR・
     CHANGELOG
3. **Rust ターゲット tier 状況**
   - rustc Platform Support: https://doc.rust-lang.org/rustc/platform-support.html
   - 確認内容: `aarch64-apple-watchos*` / `aarch64-apple-visionos*` の tier 変化（tier 3 の
     まま維持されているか、`rustup target add` のみで従来どおりコンパイル可能か）
4. **手元確認手順**
   - `scripts/apple/build-watchos-visionos-targets.sh` の再実行による `fandhe-core`
     コンパイル可否確認（TASK-3.1 成果物。局所ツールチェーン 1.96.0 を使用し、
     ワークスペース既定の `rust-toolchain.toml` 固定は変更しない）
   - `cargo update --dry-run`（ワークスペースルートで実行）による `wgpu`/`libloading` の
     新バージョン検知
   - macOS 実機環境が利用可能になった場合は、`fandhe-egui-proxy`（wgpu 依存経路）の
     visionOS 向けクロスビルドも合わせて確認する（本文書 §2 で未確認と記録した項目の解消）

## 4. 頻度とトリガー

- **定期**: M3・M4 の期間中は月 1 回確認する（`06-roadmap.md` の M3 リスク項が定める
  「M3・M4 の期間中も定期的に確認」に整合）。M5 以降は四半期 1 回に頻度を落とす。
- **随時**: 次のいずれかの事象を検知した時点で臨時に確認する。
  - `wgpu` または `libloading` のメジャー・マイナーリリース
  - 依存更新 PR（`cargo update` を伴う PR）の作成時
  - `.github/workflows/update-external.yml`（上流 `Fandhe-AI/actions` の reusable workflow を
    SHA 固定で呼び出す wrapper）が呼び出す上流ワークフローの対象範囲が拡張された場合、
    または本リポジトリ側に監視専用の別 workflow が新設された場合
    （現状は `docs/spec` サブモジュールと `.claude/skills` のみが対象で、cargo 系の
    自動検知は含まれない。本文書 §6 のスコープ外事項を参照）

## 5. 記録方法

本文書末尾の「監視ログ」表（日付・確認者・`wgpu` バージョン・`libloading` バージョン・
対応状況・判断・次アクション）へ追記する。エントリは削除せず追記のみとし、履歴を残す。

## 6. 変化検知時のアクション

- **対応改善**（watchOS への Metal API 提供、visionOS での `libloading`/`wgpu` ビルド成功等）
  を検知した場合: REQ-3 の Won't 事項「watchOS/visionOS の独自レンダラ一貫対応」
  （`04-requirements.md` の Won't 一覧）の再評価を、spec リポジトリ
  （Fandhe-AI/fandhe-multi-platform-spec）側の検討事項としてユーザーへ報告する。
  本リポジトリでは spec を変更しない（`out-of-scope-tracking.md` 準拠）。
- **対応悪化**（現状より制約が広がる等）を検知した場合: `06-roadmap.md` のリスクと対策表が
  定めるとおり、REQ-3 の網羅範囲（watchOS・visionOS のどちらを優先するか）の見直しを
  ユーザーへ提起する。
- いずれの場合も Issue の起票はユーザー承認後にのみ行う（`out-of-scope-tracking.md` の
  フローに従う）。

## 7. 役割分担

- **一次担当**: 人間（ユーザー）。監視サイクルの実施判断・spec リポジトリへの報告・
  Issue 起票の承認を担う。
- **Claude Code の役割**: 確認作業の実行支援（reference-researcher への外部調査委譲を含む）・
  監視ログ表への追記支援。監視結果の解釈・対応方針の決定は行わない。

## 8. REQ-3 受け入れ基準・M3 完了基準との整合表

| 出典 | 記載内容 | 本文書での対応 |
|------|---------|---------------|
| REQ-3 受け入れ基準 5 項目目（`04-requirements.md`） | `wgpu`/`libloading` の watchOS/visionOS 対応状況を継続的に監視する運用ルールを定める | 本文書 §2〜§6 で監視対象・方法・頻度・記録・アクションを定義 |
| NFR-6（`04-requirements.md`） | `wgpu`/`libloading`/`winit`/`accesskit` の watchOS・visionOS・Web 対応状況の継続監視 | 本文書は `wgpu`/`libloading`（watchOS・visionOS）のみを対象とし、`winit`/`accesskit` は TASK-8.6（Issue #212）が別途担当（§1 で明記、重複管理を回避） |
| `06-roadmap.md` M3 リスク項 | `wgpu`/`libloading` の watchOS/visionOS 対応状況は upstream 次第で変化しうるため、TASK-3.1〜3.4 の実装中に前提が変わるリスクがある。TASK-3.6 の監視ルールで早期検知を図る | 本文書 §4 の定期監視（M3・M4 期間中は月 1 回）で対応 |
| `06-roadmap.md` リスクと対策表 | `wgpu`/`libloading`/`accesskit` の watchOS/visionOS/Android 対応状況が upstream の変更により悪化する場合、TASK-3.6 の監視運用ルールに基づき M3・M4 の期間中も定期的に upstream の対応状況を確認し、悪化時は REQ-3 の網羅範囲を見直す | 本文書 §4（頻度）・§6（対応悪化時のアクション）で対応 |

## 9. スコープ外事項（ユーザー承認前提、`out-of-scope-tracking.md` 準拠）

- **監視の CI 自動化**: `wgpu`/`libloading` のバージョン検知ジョブを CI に追加すること。
  `.github/workflows/update-external.yml` は上流 `Fandhe-AI/actions` の reusable workflow を
  SHA 固定で呼び出す wrapper であり、`uses:` で呼び出すジョブへ直接ステップ・ジョブを
  追加することはできない。追加する場合は (a) 監視専用の別 workflow ファイルを本リポジトリに
  新設する、または (b) 上流 `Fandhe-AI/actions` の reusable workflow 側へ機能追加を提案する
  （全リポジトリへ配布されるため影響範囲が広い）、いずれかによる。現状の同ワークフローが
  呼び出す上流ワークフローが対象とするのは `docs/spec` サブモジュールと `.claude/skills`
  のみで、cargo 系のジョブは含まれていない。追加する場合も `ci.md`（GitHub ホスト runner・
  最小権限・fork PR での任意コード実行防止）に従う必要がある。本文書ではスコープ外の
  候補として記録するに留め、Issue 起票はユーザー承認後に行う。
- **PoC-2 制約の現行版での再現確認**: §2 のとおり、Linux 環境の制約により
  `fandhe-egui-proxy`（wgpu 依存経路）での visionOS 向けビルド確認が未実施。macOS 実機
  環境（TASK-18.1）整備後に優先実施する。

## 10. 監視ログ

| 日付 | 確認者 | `wgpu` | `libloading` | 対応状況 | 判断 | 次アクション |
|------|-------|--------|--------------|---------|------|-------------|
| 2026-07-30 | Claude Code（本文書作成時点の初回記録） | 27.0.1 | 0.8.9 | バージョンは PoC-2 時点（`wgpu` 0.20 系・`libloading` 0.7.4）から進行しているが、watchOS の Metal API 非提供・visionOS の `libloading`/`dlopen` 定数制約が現行版でも継続しているかは Linux 環境の制約により未確認 | 制約継続を前提に REQ-3 のハイブリッド構成（SwiftUI バインド）方針を維持 | macOS 実機環境整備後、`fandhe-egui-proxy` の visionOS クロスビルドで再確認する |
