# Android NDK/Gradle 環境セットアップ手順書

## 1. 目的・spec 対応

- **対応 Issue**: #247（docs(build): TASK-11.4 Android NDK/Gradle インストール手順文書化）
- **spec 対応**: REQ-11（ストア提出パイプライン・CLI ベース・開発署名まで、
  `docs/spec/04-requirements.md` L147-159）受け入れ基準 3「Android NDK・Gradle の
  インストール手順を開発環境セットアップ手順として文書化する」/ TASK-11.4
  （`docs/spec/05-tasks.md` L364-370、想定 0.5 日）/ M4 完了基準「Android `.aab`
  パイプラインがエミュレータへインストール・起動できることを確認し、NDK/Gradle
  インストール手順が文書化されている」（`docs/spec/06-roadmap.md` L148）。
- **REQ-18（開発環境整備）との分担**: NDK・Gradle・JDK・SDK・bundletool・cargo-ndk の
  導入自動化は `scripts/android/setup-android-env.sh`（冪等インストーラ）・
  `scripts/android/versions.env`（バージョン・チェックサムの SSOT）が担う
  （TASK-18.3 / Issue #13）。本書はその導入スクリプトを「新規環境をゼロから整備する手順」
  として独立した文書にまとめるものであり、導入ロジック自体を再実装・重複記載はしない。
- **担当区分の扱い**: spec 上の担当は「人間（分野: 環境整備）」（`docs/spec/05-tasks.md` L370）。
  本手順書自体はエージェントが作成し、フレッシュ環境（未導入マシン）での手順書どおりの
  再現実行は人間担当として 6 章に明記する
  （CLAUDE.md「実機検証: 実機・証明書・外部手続きが必要なタスクは『人間担当』として
  ユーザーへ明示的に依頼する」）。

## 2. 前提条件

- **ホスト OS**: Linux x86_64（現行の `setup-android-env.sh` は Linux 専用設計。
  macOS / Windows ホストでの Android 環境構築は本書のスコープ外。7 章参照）。
- **基本ツール**: `curl` / `unzip` / `tar` / `sha256sum` / `sha1sum` / `flock` /
  `df`（いずれも一般的な Linux ディストリビューションに標準搭載）。
- **rustup**: リポジトリルートの `rust-toolchain.toml`（1.88.0 系）に対応する
  ツールチェーンを導入済みであること。未導入でも `setup-android-env.sh` 自体は失敗せず、
  rustup/cargo に依存する手順（rustup Android ターゲット追加・cargo-ndk 導入）のみを
  skip する（3 章参照）。
- **ディスク空き容量**: NDK 込みで概ね 5GB 以上の空き容量（インストーラが実ダウンロード
  発生時に自動チェックする）。
- **権限**: sudo 不要。導入先はすべて `${FANDHE_ANDROID_ROOT:-${HOME}/.fandhe/android}`
  （既定は HOME 配下）に閉じており、システム領域（`/usr` 等）へは一切書き込まない
  （`.claude/rules/ci.md` の「グローバル状態を汚す処理を書かない」方針に準拠）。
  シェル profile（`~/.zshrc` 等）の書き換えも行わない。

## 3. インストール手順

以下の 3 ステップを順に実行する（単一パス）。

```bash
# 1. JDK・Android SDK commandline-tools・NDK・Gradle・bundletool・cargo-ndk を導入
#    （導入済みコンポーネントは自動 skip される冪等設計）
bash scripts/android/setup-android-env.sh

# 2. 導入先への環境変数（JAVA_HOME / ANDROID_HOME / ANDROID_NDK_HOME / PATH 等）を反映
source scripts/android/android-env.sh

# 3. rustup 経由の Android クロスコンパイルターゲットを追加
#    （手順 1 の setup-android-env.sh 内で rustup 導入済みなら自動実行済み。
#     rustup を手順 1 の後に導入した場合は明示的に再実行する）
bash scripts/android/setup-android-env.sh
```

### 導入されるコンポーネント

`setup-android-env.sh` が導入するコンポーネントは以下のとおり。個別バージョン番号は
陳腐化を防ぐため本書へ重複記載せず、`scripts/android/versions.env` を単一の SSOT として
参照する（一次情報 URL・チェックサム取得根拠も同ファイルのコメントに記載済み）。

- JDK（Eclipse Temurin LTS）
- Android SDK commandline-tools（`sdkmanager`）
- Android SDK パッケージ: `platform-tools` / `platforms;android-*` / `build-tools;*` / NDK
- Gradle
- bundletool（`.aab` を実機/エミュレータへインストールするための単一 jar。4 章参照）
- cargo-ndk（rustup/cargo 導入済みの場合のみ。`cargo install --locked` でサプライ
  チェーン改変を検知しつつ導入）
- rustup Android ターゲット（`aarch64-linux-android` / `armv7-linux-androideabi` /
  `x86_64-linux-android`。rustup 導入済みの場合のみ）

### 設計上の前提（手順の再現性に関わる要点）

- **冪等性**: 各コンポーネントは「期待バージョンが導入済みなら skip」の設計であり、
  同じコマンドを再実行しても安全（中断後の再実行・CI との共通経路として設計されている）。
- **完了マーカー方式**: ダウンロード系コンポーネントは一時ディレクトリへ展開してから
  最終パスへ atomic mv する。中断による半展開状態を「導入済み」と誤認しない設計のため、
  導入途中でプロセスが中断されても、手順の再実行だけで安全に復旧できる。
- **並列実行時の直列化**: `FANDHE_ANDROID_ROOT`（既定 `${HOME}/.fandhe/android`）単位で
  `flock` により導入・検証全体を直列化する。同一ホスト上で複数の導入プロセスが
  同時に走っても、共有 SDK ルートへの競合書き込みは発生しない。

## 4. 検証手順

### 4.1 導入結果の受け入れ確認

```bash
bash scripts/android/setup-android-env.sh --verify-only
```

インストールを一切行わず、導入済みバージョンが `versions.env` の期待値と一致するかのみを
検査する。ローカルでの受け入れ確認と CI（5 章）の検証が同一コードパスで実行される設計
であるため、本コマンドが exit 0 で完了すれば、新規環境の整備がその CI と同じ基準で
完了したとみなせる。

### 4.2 スモーク確認（ビルドパイプラインへの導線）

導入した環境が実際にビルドに使えることは、以下の既存パイプラインで確認できる
（詳細な使い方は `docs/operations/android-build-pipeline.md` 参照）。

```bash
scripts/android/build-android.sh          # fandhe-core のみのクロスビルド
scripts/android/build-aab.sh              # .aab 生成（未署名）
```

## 5. CI（GitHub Actions）での利用

`.github/workflows/ci.yml` の `android-env` ジョブは、本書の手順と同一の
`scripts/android/setup-android-env.sh` を「導入 → `--verify-only` 検証」の順で
同一ジョブ内で連続実行する（ローカル・CI で導入経路を分岐させない設計）。

- GitHub ホスト runner は使い捨て VM のため、導入先 `~/.fandhe/android` を `actions/cache`
  （キーは `versions.env` と `setup-android-env.sh` のハッシュ）で復元する。キャッシュミス時のみ
  スクリプトが取得する。`flock` は同一ホスト上の並列実行（ローカル）向けに残している。
- 「導入済みならコンポーネント単位で skip」の冪等設計により、初回以降のジョブ実行は
  数 GB のダウンロード・展開を繰り返さない。

## 6. 実機/エミュレータ接続の前提（本書のスコープ外）

adb・実機/エミュレータへの端末接続そのものは本手順書のスコープ外とする。以下を参照する。

- `docs/operations/android-build-pipeline.md`（4.4 節、`verify-aab-install.sh` の使い方）
- `scripts/android/verify-aab-install.sh`（`.aab` を bundletool で `.apks` へ変換し
  接続端末へインストールする検証スクリプト本体）

**フレッシュ環境での再現実行（人間担当）**: 本書の手順（3 章のインストール・4 章の検証）を
未導入のフレッシュ環境で実際に一から実行し、`--verify-only` が exit 0 になることを
確認する作業は、実機・外部手続きに準じる環境整備作業としてユーザーが実施する
（CLAUDE.md の実機検証方針に従う）。

## 7. トラブルシューティング / 更新手順

- **中断時の再実行**: 導入途中でプロセスが中断された場合も、3 章のコマンドをそのまま
  再実行すればよい（完了マーカー方式により半展開状態を誤って「導入済み」と判定しない）。
- **並列実行時のロック待ち**: 同一 `FANDHE_ANDROID_ROOT` に対して複数プロセスが同時に
  導入を試みた場合、後発プロセスは `flock` の待機（既定 `FANDHE_ANDROID_LOCK_TIMEOUT_SEC`
  環境変数で上書き可能）に入る。ロック保持側が完了すれば自動的に進行する。
- **ライセンス同意**: Android SDK のライセンス同意は `setup-android-env.sh` が非対話
  （`yes` パイプ）で自動的に行う。手動介入は不要。
- **バージョン更新時の運用**: コンポーネントのバージョンを上げる場合は
  `scripts/android/versions.env` のみを更新する。更新時は当該コンポーネントの一次情報
  （公式リリースページ・Maven リポジトリ等）からバージョンとチェックサムを再取得し、
  同ファイルのコメントに取得日・根拠を追記する（`versions.env` 冒頭のコメント規約に従う）。
  本書 3 章の手順・コマンド自体は更新不要（バージョン非依存の手順として設計している）。

## 8. 受け入れ基準との整合確認

| 本書の該当箇所 | 対応する spec 記述 |
|---|---|
| 3 章（インストール手順）・4 章（検証手順） | REQ-11 受け入れ基準「Android NDK・Gradle のインストール手順を開発環境セットアップ手順として文書化する」（`docs/spec/04-requirements.md` L157） |
| 本書全体 | TASK-11.4「内容: Android NDK・Gradle のインストール手順を開発環境セットアップ手順として文書化する」「成果物: 環境セットアップ手順書」（`docs/spec/05-tasks.md` L364-368） |
| 本書全体（TASK-11.3 の検証結果と合わせて） | M4 完了基準「Android `.aab` パイプラインがエミュレータへインストール・起動できることを確認し、NDK/Gradle インストール手順が文書化されている」（`docs/spec/06-roadmap.md` L148。前半のエミュレータ確認は TASK-11.3・Issue #246 で達成済み、後半の文書化を本書で達成） |

## 9. スコープ外（out-of-scope）

- macOS / Windows ホストでの Android 環境構築（現行の `setup-android-env.sh` が
  Linux x86_64 前提の設計であるため）。必要になった場合は
  `.claude/rules/out-of-scope-tracking.md` のフローに従い、ユーザー承認後に別 Issue として
  検討する。
- 実機/エミュレータへのインストール・起動確認そのもの（TASK-11.3・Issue #246 のスコープ。
  6 章参照）。
- 開発用署名鍵の生成・管理（`docs/operations/android-aab-signing.md` のスコープ）。
