---
paths:
  - "lefthook.yml"
---

# Git hooks（lefthook）編集規約

## 目的

フックはコミットのたびに実行されるため、遅いフックはそのまま開発速度の低下になる。
フックはローカルでの早期検知専用であり、CI の代替ではない。**開発速度を落とさないことを最優先**し、
重い検査・網羅的な検査は CI（`.github/workflows/ci.yml`）に任せる。

## 時間予算

| フック | 予算 | 判定方法 |
| --- | --- | --- |
| pre-commit | 全ジョブ合計 1 秒未満 | `lefthook run pre-commit --all-files` の `summary: (done in N seconds)` |
| commit-msg | 即時（外部コマンド・ネットワーク不可） | 実コミット時に表示される commit-msg の summary（メッセージファイルを引数に取るため `--all-files` では計測しない） |
| pre-push・post-checkout・post-merge 等 | 原則追加しない | 追加する場合は理由と実測値を PR 本文に記載する |

`--all-files` は全追跡ファイルを対象にする最悪ケースであり、staged ファイルに限定した検査は実際のコミットではこれより速い。

## フックに置かないもの（→ `ci.yml` へ）

- テストスイート（`cargo test`）・ビルド・クロスビルド（Android / wasm32 等）
- `cargo clippy`（CI の `clippy` ジョブがホスト全体と wasm32 の両方で実行している）
- リポジトリ全体を走査する lint / fmt チェック（`make lint` 全体の呼び出しを含む）
- 依存パッケージのインストール・ネットワークアクセス
- `ci.yml` のジョブ（`fmt` / `clippy` / `test` / `android-env` / `android-build` / `capabilities`）と同じコマンドを同じ範囲で実行するもの

手元で CI 相当を確認したい場合は `make lint`（`cargo fmt --check` + `cargo clippy -D warnings`）と
`make test` を実行する。ローカルでの確認の規約自体（`.claude/rules/delegation-impl.md` の検収規約、
test-runner・linter による `cargo test` / `cargo clippy` の確認）は変わらず、フックで強制しないだけである。

## フックに置いてよいもの

- `{staged_files}` と `glob` / `exclude` で対象を絞った 1 秒未満の検査
- CI ではコミット単位で検知できない、または push 後では遅いもの
  - `forbid-env-files`（pre-commit）: `.env` 系ファイルは push 前に止める必要があり、CI にも同等の検査がない
  - `conventional-commit-format`（commit-msg）: CI にはコミットメッセージを検査するジョブがない

### 明示的な例外

- pre-commit の `rust-fmt-check`（`cargo fmt --all -- --check`）は CI の `fmt` ジョブと同じコマンドで、
  かつ workspace 全体を対象にするため、上記「フックに置かないもの」に該当する。それでも次の理由で維持する
  （2026-10-11 計測・決定）
  - `cargo fmt` はクレートごとの `Cargo.toml`（edition）を見て整形するため、`{staged_files}` に絞ると
    edition 判定がずれて誤検出する（`crates/` 配下は edition 2021 と 2024 が混在している）。
    そのため `glob: "*.rs"` で「`.rs` がステージされたときだけ実行する」形に留めている
  - 計測値: `lefthook run pre-commit --all-files` の summary で 0.70 秒（rust-fmt-check 0.70 秒、
    forbid-env-files 0.02 秒、lefthook 2.1.9・rustfmt 1.88.0、Apple Silicon の macOS）で予算内に収まる
  - 実行範囲が staged ファイルに依存しないため、`.rs` を含む実際のコミットでも所要時間は `--all-files` と同程度になる。
    crate の追加などで 1 秒を超えた場合は、フックから外して CI の `fmt` ジョブに任せる

## 書き方の規約

- `pre-commit` の `parallel: true` を維持する（直列化しない）
- 既存の `commands:` 形式とコメントの文体（各コマンドの直前に目的・根拠を日本語で書く）に合わせる
- `glob` / `exclude` で対象を絞り、外部取得物の `.agents/skills/**`・`.claude/skills/**` を検査対象にしない
- 条件付き実行が必要な場合は lefthook の `skip` / `only` を使ってよい
  （`.claude/skills/lefthook/references/examples/skip.md`）

## 検査を追加するときの判断手順

1. その検査は `ci.yml` で既に実施されているか
    - 実施済みで、staged 限定にしても 1 秒以上かかる → フックに入れない
2. staged ファイルに限定できるか
    - できない → CI へ置く
3. CI へ置く場合は `ci.yml` にジョブを追加し、集約ジョブ `ci-complete` の `needs` と結果判定にも必ず追加する
    - workflow の変更は `.claude/rules/ci.md`（GitHub ホストランナー原則）に従い、build-pipeline-builder へ委譲する
4. 編集後に以下を実行し、summary の合計時間をユーザーへ報告する（1 秒以上なら構成を見直す）

```bash
time lefthook run pre-commit --all-files
```

## バイパス禁止

遅いからといって `--no-verify` や `LEFTHOOK=0` で回避しない（`.claude/rules/conventional-commits.md`）。
遅いフックはフック側を直す（検査を CI へ移す・staged 限定にする・対象 glob を絞る）。
