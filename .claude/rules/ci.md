---
paths:
  - ".github/workflows/**"
  - ".github/actions/**"
---

# CI 規約

## ランナー（必須）

- GitHub Actions のジョブは GitHub ホストランナー（`ubuntu-latest`）で実行する。public リポジトリでは標準ランナーが無料で、fork PR のコードを社内の runner で動かさないため（組織のランナー方針: Fandhe-AI/actions の `docs/runner-policy.md`）。reviewer / linter が `runs-on` を検査する
- 例外は `ai-review.yml` の codex review ジョブのみ（呼び出し先の既定で codex 専用 self-hosted runner。マシンのログインに依存するため）。wrapper に `runs-on` を書かない。fork PR は呼び出し先の preflight・wrapper の `if`・GitHub の fork PR 承認ポリシー（`all_external_contributors`）で止める。これ以外に self-hosted を足さない

## 運用

- workflow の作成・変更は build-pipeline-builder へ委譲する（delegation-impl.md 参照）
- シークレット・認証情報の取り扱いは security.md に従う（ログ出力禁止・GitHub Secrets 経由のみ）。fork からの pull_request には Secrets が渡らない前提で、Secrets を要するジョブは fork で skip する
- トリガー・permissions は最小権限で設計する（`pull_request_target` で PR 側のコードを実行しない）
- GitHub ホストランナーは毎回まっさらな VM で、ツールチェーン・キャッシュはジョブ間で共有されない。必要なものはジョブ内で導入するか「キャッシュ方針」に従って復元する

## runner 方針

- 本リポジトリは **public**。`ci.yml`・`update-external.yml`・`ai-review.yml` の preflight / post_feedback は `ubuntu-latest`、codex review のみ self-hosted（上記の例外）
- Windows・macOS の実機が要る確認は CI に置かず、手元で行う

## 必須チェックと集約ジョブ

- ruleset `main-protection` の required status checks は次のジョブ名（`name:`）を参照する: `cargo fmt --check` / `cargo clippy` / `cargo test` / `android NDK/Gradle 導入` / `android クロスビルド` / `ci-complete` / `codex / preflight` / `codex / review` / `codex / post_feedback` / `Cursor Bugbot`
- これらのジョブ名を変える・ジョブを削除するときは ruleset 側の更新が先に必要（人間の作業。勝手に変えるとマージ不能になる）
- ジョブを追加したら `ci.yml` の `ci-complete` の `needs` と判定ループに必ず追加する（skipped は `changes` が `heavy=false` を返した test / android-* のみ許容）

## 発火条件の方針

- `ci.yml` は `pull_request`（base: main）と `push`（main のみ）。同一ブランチの二重実行を作らない
- `concurrency` は PR のみ `cancel-in-progress: true`、main への push は取り消さない（`${{ github.event_name == 'pull_request' }}`）
- **必須チェックを含む workflow に workflow 単位の `paths` / `paths-ignore` を付けない**（条件不一致だと check が pending のままマージ不能になる）。絞りたい場合は `ci.yml` の `changes` ジョブのようにジョブ内で変更検知し、ジョブ単位で skipped にして `ci-complete` で許容する。許可リストは「ビルド・テストに影響しないもの」だけを列挙し、判定不能なら重い側に倒す。変更ファイルは `git diff --no-renames` で取り、リネームの削除元も判定に含める（`docs/ai/SCHEMA.md` は crates が `include_str!` するため含めない）
- draft PR は skip しない（エージェントが draft で CI を確認する運用があるため）。schedule は `update-external.yml` の日次のみ。頻度を上げない

## ジョブ・workflow を追加するときのチェックリスト

1. 新しい workflow ファイルを増やさず、既存 `ci.yml` へジョブを追加できないか先に検討する
2. 不要な `needs` を作らない（前提導入は冪等セルフヒールで自己完結させ、並列にする）
3. `ci-complete` の `needs` と判定ループへ追加する。必須にする場合は ruleset 更新をユーザーへ依頼する
4. `actions/checkout` は既存と同じ SHA 固定・`persist-credentials: false`。履歴が不要なら `fetch-depth` は既定（1）のまま。submodule は取得しない
5. ロジックを YAML に書き込まず `scripts/` のスクリプトや `make` ターゲットを呼ぶ（`.claude/rules/makefile.md`）
6. lefthook と同じ検査を二重に置かない（`.claude/rules/git-hooks.md`。CI 側が正）
7. 所要時間の目安: fmt/clippy/capabilities は 1 分未満、test は約 3〜5 分、android クロスビルドは約 10〜23 分。これを大きく超える追加は理由を PR 本文に書く
8. キャッシュを使う場合は「キャッシュ方針」に従う（効果を実測し、保存は main への push のみ。確認結果を PR 本文に書く）

## キャッシュ方針

現状: android の 2 ジョブだけが `actions/cache` で `~/.fandhe/android`（NDK・SDK）を復元する（key は `versions.env` と `setup-android-env.sh` のハッシュ）。Rust の registry・target はキャッシュしていない。キャッシュを追加する場合は本節に従う。

### 前提（容量と課金）

- リポジトリあたりの上限は 10GB（Org 設定も 10GB）。超えると最終アクセスの古い順に追い出され、7 日間アクセスの無いキャッシュは削除される
- Org の Actions 予算には上限がある（超過時は停止）ため、10GB を超える上限は使えない。上げると 10GB を超えた時点でキャッシュが読み取り専用になる。上限・予算の変更はユーザー判断

### 入れる前に効果を測る

- キャッシュは復元時間がかかり、効果が無い場合がある。他リポの実例: mirror-ui は pnpm キャッシュの復元に 1 ジョブ平均 75 秒かけていたが `downloaded 0` で効果ゼロだったため撤去。team-hub は `Swatinem/rust-cache` が `./target` を保存対象にしていたが runner の `CARGO_TARGET_DIR` が別の場所で registry しか保存されておらず、さらに後処理が共有 `CARGO_HOME` の crate を消していたため撤去
- キャッシュを足す・残す前に、ジョブログで効果を確かめる（`Downloaded` の件数、`Compiling` の件数＝依存の再ビルド数、復元にかかった秒数）。効果が無ければ入れない・残さない

### 保存は main への push だけ

- PR から保存したキャッシュは `refs/pull/<N>/merge` スコープに入り、その PR からしか読めないのに上限を共有して main のキャッシュを追い出す
- `actions/cache` は `actions/cache/restore`（全イベント）と `actions/cache/save` に分け、save には `if: github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.<id>.outputs.cache-hit != 'true'` を付け、`key: ${{ steps.<id>.outputs.cache-primary-key }}` で保存する（`<id>` は restore ステップの `id`）
- `Swatinem/rust-cache` を使うなら `save-if: ${{ github.ref == 'refs/heads/main' }}` を渡す
- Fandhe-AI/actions の `rust-base-ci.yml` を `cache: true` で呼ぶなら `cache-save: ${{ github.ref == 'refs/heads/main' }}` も渡す（共通側の `cache-save` の既定は true で、PR からも保存する）
- `actions/setup-node` の `cache: pnpm` など、`setup-*` 系 action の一体型キャッシュは PR でも保存する。使うなら保存を main に限れる restore / save の分離に置き換える

### キーの設計

- キーは中身のハッシュ（lockfile 等）にする。`github.sha`・`github.run_id` など毎回変わる値は入れない。入れる場合は `restore-keys` の前方一致で直近を拾う設計にし、保存は main に限る
- `hashFiles('**/Cargo.lock')` のような再帰 glob は使わず、ルートの `Cargo.lock`（ビルド対象に本当に入るものだけ）を指定する。本リポにも `.agents/skills/make/samples/projects/` 配下にスキルのサンプル（`Cargo.lock` 3 件・`pnpm-lock.yaml` 2 件）と `benchmarks/memory/react-native/package-lock.json` があり、再帰 glob はこれらを拾う。依存が変わらないのにスキル更新でキーが変わり全キャッシュを作り直す事故が fandhe-browser・fandhe-frontend・fandhe-db・fandhe-container・template-frontend-react_router で起きた
- `target/` などビルド成果物をキャッシュするなら、キーにツールチェーンの版（`rustc -vV` の `release`・`commit-hash`）を入れる。`rust-toolchain.toml` は版を固定しているが、キーには実際に使われた `rustc` の値を入れる方が確実。版を入れないと toolchain 更新後も古い成果物に exact hit し続け、exact hit では再保存されないため依存を毎回全再ビルドする（fandhe-db で依存 135 crate を毎回再ビルドしていた）
- `restore-keys` で前方一致を許し、キーが変わっても cold にならないようにする

### サイズを抑える

- 1 世代の合計が 10GB に余裕をもって収まるようにする。キャッシュの種類を増やすときは 1 件のサイズ × 種類 × OS・ターゲット（android クロスビルド等）を見積もる
- `target/` を保存するなら、保存前に workspace メンバーの成果物を prune し、`CARGO_INCREMENTAL=0`、CI 限定で `CARGO_PROFILE_DEV_DEBUG=line-tables-only` 等で縮める
- 上限に達しても、毎回読まれる main の最新キャッシュは最後まで残る（LRU）。キーの切り替えで古い世代が追い出されても新しい世代が保存済みならミスにならない。この状態を保てていれば上限を上げる必要はない

### 計測

```bash
gh api repos/Fandhe-AI/fandhe-multi-platform/actions/cache/usage                      # 件数・合計サイズ
gh cache list -R Fandhe-AI/fandhe-multi-platform -L 100                                # key・サイズ・最終アクセス
gh cache list -R Fandhe-AI/fandhe-multi-platform -L 100 --json id,key,ref,sizeInBytes  # ref（main か PR か）
```

- ジョブログの `Cache restored from key:`（primary key と一致なら exact、それ以外は partial）・`Cache not found`・`Cache saved with key:` で hit / miss と保存を確かめる
- 不要なキャッシュは `gh cache delete <id>` で消せるが、共有状態なので対象（key・ref）を確認してから消す

### PR のチェックリスト

ワークフローにキャッシュを追加・変更する PR では、次を確認したことを PR 本文に書く。

- [ ] 効果を実測した（復元秒数・依存の再ビルド数。効果が無ければ入れない・残さない）
- [ ] 保存は main への push に限っている
- [ ] キーはルートの lockfile のハッシュで、成果物を含むならツールチェーンの版を含み、`restore-keys` がある
- [ ] 1 世代の合計サイズを見積もり、10GB に余裕をもって収まる

## 計測方法と棚卸し結果

```bash
gh run list --workflow ci.yml --limit 30 --json databaseId,conclusion,createdAt,updatedAt,event
gh run view <run-id> --json jobs --jq '.jobs[]|[.name,.conclusion,.startedAt,.completedAt]|@tsv'
```

GitHub ホストランナーへの移行後の実測は未取得（移行前の self-hosted では `ci.yml` 中央値 約 9.8 分・最大 約 25 分、クリティカルパスは `android クロスビルド`）。
