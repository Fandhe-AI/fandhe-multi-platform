# 共通/プラットフォーム別コードの分類・計測規則（確定版）

## 1. 位置づけ

本書は issue #53（TASK-2.2-S6、親 issue #47 = TASK-2.2 の sub-issue 6/6）の成果物であり、
`docs/architecture/prototype-app-design.md` 6 章（issue #48 = TASK-2.2-S1）の「分類方針」を
確定版に昇格したものである。

REQ-2 の受け入れ基準（`docs/spec/04-requirements.md`）は「複数画面（3〜5 画面）規模の
プロトタイプで共通コードベース比率を再計測し、結果を記録する」ことを求めている。
PoC-1（`docs/spec/03-poc/ui-rendering/README.md`）の最小デモ実測値は 70/126 行 ≈ 55.6%
（成功基準 70% 未達）であり、画面数増加による固定費希釈での改善見込みを実測で裏付ける
再計測（TASK-2.3、M3 Conditional Go 条件 2）が必要になる。

本 issue のスコープは再計測そのもの（TASK-2.3 の成果物）ではなく、その前段としての
「分類の確定」と「計測を機械的・再現可能に行うツールの整備」である。

## 2. 母集団と分類

| 区分 | 対象 | 分子 | 分母 |
|------|------|------|------|
| 共通コード | `crates/fandhe-prototype/src/**/*.rs` | ○ | ○ |
| プラットフォーム別コード | 分類 SSOT（`scripts/metrics/code-classification.env`）に列挙したファイル全体、および既存プラットフォームファイル内のマーカー区間 | × | ○ |
| 母集団外 | `fandhe-core`（フレームワーク本体）、`fandhe-egui-proxy` の既存コード（マーカー区間外）、Cargo.toml・HTML・スクリプト、`tests/` 配下の結合テスト | — | — |

- **共通コード**: TASK-2.2-S1（issue #48）で切り出した独立クレート `crates/fandhe-prototype` に
  隔離済み。`fandhe-core` のみに依存し `#![forbid(unsafe_code)]` を付与している。
- **プラットフォーム別コード**: プロトタイプ描画起動のためにプラットフォーム側
  （`fandhe-egui-proxy` 等）へ追加する差分。以下の 2 通りで表現する。
  1. **専用ファイル化**: 差分が独立した `.rs` ファイルに切り出せる場合、
     `code-classification.env` の `PLATFORM_FILES` に REPO_ROOT からの相対パスを追記する
     （`measure-code-ratio.sh` が REPO_ROOT へ cd した上で `find` 出力の相対パスと
     突き合わせるため。絶対パスを書くと突き合わせが不一致になり、ファイル全体と
     マーカー区間の双方で二重計上されるおそれがある）。
     ファイル全体をプラットフォーム別コードとして計上する。
  2. **マーカー区間**: 既存プラットフォームファイルへ直接追記する場合、当該差分を
     ```
     // fandhe-prototype:begin
     ...
     // fandhe-prototype:end
     ```
     のコメント対で囲む。走査対象ディレクトリは `code-classification.env` の
     `PLATFORM_MARKER_DIRS` に列挙する。区間内（begin/end 行自体は含まない）を
     プラットフォーム別コードとして計上する。
- **フレームワーク本体（`fandhe-core`）は母集団外**。共通コードの定義上「複数プラットフォームで
  共有できる資産」の効果を測るのはプロトタイプ層（`fandhe-prototype`）であり、
  フレームワーク本体はどのプラットフォームでも必須の基盤として別に扱う（PoC-1 と同じ整理）。
- **注意（PoC-1 との前提差）**: 「母集団外」の扱いは
  `fandhe-core` に限らず、`fandhe-egui-proxy` の**既存コード（マーカー区間外、`ProxyApp` や
  `render_screen` 等）**にも及ぶ。PoC-1（`docs/spec/03-poc/ui-rendering/README.md`）は
  「デモ 1 本分の起動・ブリッジコード全体」をネイティブ + Web の 2 プラットフォームに配線した
  状態で分母に計上しているのに対し、本書のマーカー方式は `fandhe-prototype` の配線コスト
  （新規差分）のみを、デスクトップ 1 プラットフォームに配線した状態で計上する。
  分母除外分の内訳・配線プラットフォーム数のいずれについても PoC-1 と厳密には同一条件ではない
  ため、両者の比率を単純比較する際は前提の違いに注意する。

## 3. 行数の数え方

- **対象は `.rs` のみ**（PoC-1 互換）。`Cargo.toml` / `index.html` / ビルドスクリプト等は
  対象外と明記する。
- **主指標 = 物理行**（`wc -l` 相当）。PoC-1 の 55.6% と直接比較可能にするための指標。
- **参考指標 = 実効行**（空行・コメント行〔trim 後 `//` 始まり〕を除いた行）。
  本リポジトリは日本語コメント必須規約（`code-comment-style.md`）でコメント比重が大きいため、
  物理行だけでは共通コード側が不利に見えるバイアスが生じ得る。実効行を併記して評価する。
- **テストコードは除外**:
  - `tests/`（結合テスト）配下は対象外。
  - ファイル内 `#[cfg(test)]` は「`#[cfg(test)]` の行からファイル末尾まで」を除外する
    （`coding-rust.md` の「ユニットテストは同一ファイル末尾の `#[cfg(test)] mod tests`」慣例に依拠）。
  - `#[cfg(test)]` が 1 ファイルに複数回出現した場合は計測規則が想定する慣例から外れるため、
    計測ツールは fail-closed（エラー終了）する。
- **マーカー区間の begin/end 不対応**（片方のみ・入れ子）も fail-closed とする。
- 70% 判定にどちらの指標（物理行/実効行）を採用するかの最終判断は TASK-2.3 に委ねる。
  本 issue は両指標を出力できる状態まで準備するに留める。

## 4. 計測ツール

- **分類 SSOT**: `scripts/metrics/code-classification.env`
  - `COMMON_SRC_DIRS`: 共通コード対象ディレクトリ（現状 `crates/fandhe-prototype/src` のみ）
  - `PLATFORM_FILES`: プラットフォーム別コード（専用ファイル）の REPO_ROOT からの相対パス一覧
    （該当ファイルを切り出した時点で追記する。現状は空）
  - `PLATFORM_MARKER_DIRS`: マーカー区間走査対象ディレクトリ
    （現状 `crates/fandhe-egui-proxy/src`）
  - `scripts/android/versions.env`・`scripts/audit/unsafe-audit.sh` の
    `FORBID_EXEMPT_CRATES` と同じ SSOT 流儀（分類基準の変更を diff で追跡可能にする）。
- **計測スクリプト**: `scripts/metrics/measure-code-ratio.sh`
  - `bash scripts/metrics/measure-code-ratio.sh` で共通/プラットフォーム別の
    ファイル別内訳・小計・総計・比率（物理行/実効行の 2 指標）を表示する。
  - `--markdown` オプションで Markdown 表を追加出力する（TASK-2.3 のレポート転記用）。
  - プラットフォーム側が 0 行の場合は「計測準備段階」の注意を表示して
    正常終了する（exit 0）。
  - 異常系（`#[cfg(test)]` 複数出現・マーカー不対応・分類 SSOT の変数未定義）は exit 1。
  - `unsafe-audit.sh` と同方針で **CI へは組み込まない**（ローカル実行のみ）。
    読み取り専用で、リポジトリへの書き込み・システム設定変更・ネットワークアクセスを行わない。

## 5. プラットフォーム別差分の追加手順

`fandhe-egui-proxy` に追加する起動・切替差分は、以下のいずれかで
本計測ツールの対象に含める必要がある。

1. 差分を専用ファイルに切り出し、`code-classification.env` の `PLATFORM_FILES` へ
   REPO_ROOT からの相対パスを追記する（推奨。マーカーより境界が明確でレビューしやすい）。
2. 既存ファイルへ直接追記する場合は `// fandhe-prototype:begin` / `:end` で囲み、
   走査対象ディレクトリが `PLATFORM_MARKER_DIRS` に含まれていることを確認する
   （`crates/fandhe-egui-proxy/src` は登録済み）。

## 6. TASK-2.3 への申し送り（実行手順の想定）

1. `crates/fandhe-prototype` を 3〜5 画面規模に拡張する（本書のスコープ外）。
2. `bash scripts/metrics/measure-code-ratio.sh --markdown` を実行する。
3. 出力を `_/verification/task-2.3-code-ratio.md`（リポジトリ管理外の想定パス）へ転記し、
   PoC-1 の 55.6% との比較・70% 判定に採用する指標（物理行/実効行）の考察を追記する。
4. 70% 未達の場合は TASK-2.4（KMP 型価値定義修正）の要否をユーザーへ報告する。

## 7. スコープ外（本 issue に含まないもの）

- 実際の比率再計測と結果記録（TASK-2.3 の成果物）。
- 70% 未達時の KMP 型価値定義修正（TASK-2.4）。
- 画面・ナビゲーション実装、およびプラットフォーム側の差分を切り出した際の
  `code-classification.env` へのエントリ追記（当該作業側または TASK-2.3 時点で実施）。
- 計測スクリプトの CI 組み込み（`unsafe-audit.sh` と同方針で見送り。必要になった時点で別途検討）。
