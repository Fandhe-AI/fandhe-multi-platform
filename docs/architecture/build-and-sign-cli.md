# ビルド CLI（`build-and-sign`）の構成・引数設計

## 1. 目的・位置づけ

本書は TASK-11.1-S1（`docs/spec/05-tasks.md` L343〜348、Issue #220、親 #219）の成果物であり、
REQ-11（`docs/spec/04-requirements.md` L147〜156）が要求する「単一コマンドから iOS/Android の
開発署名（ad-hoc 署名）までを CLI ベースで自動生成できる」パイプラインの CLI 構成・引数・終了コード
体系を設計し、それを固定化する CLI 骨格（`scripts/build/build-and-sign.sh`）として実装したものです。

- **`docs/spec/` との関係**: `docs/spec/` は読み取り専用サブモジュールのため、本書は本リポジトリ側
  （`docs/architecture/`）の新規ドキュメントとして作成しています。spec 自体の変更はこのリポジトリでは
  行いません（既存の `docs/architecture/*.md` と同じ置き場・体裁）。
- **前提**: PoC-3（`docs/spec/03-poc/store-submission/`）で iOS シミュレータ版 `.ipa`（ad-hoc 署名）
  生成の手順を実証済みです。
- 本 CLI（`scripts/build/build-and-sign.sh`）は、引数解析・前提検査・終了コード体系・`--dry-run` を
  自身で担い、実際の `.app`/`.ipa` 生成・ad-hoc 署名処理は `scripts/ios/build-simulator-app.sh`
  （TASK-11.1-S2/S3、#221・#222）へフェーズ単位で委譲する構成をとります
  （「2. CLI 配置と全体構成」「4. 終了コード体系」参照）。macOS 実環境での動作確認は
  TASK-11.1-S4（#223、人間担当）の範囲です（「8. スコープ外・申し送り」参照）。

## 2. CLI 配置と全体構成

- エントリポイント: **`scripts/build/build-and-sign.sh`**。
  - REQ-11 は「Android・Windows へも同型で拡張する」と規定するため、プラットフォームを第 1 位置引数の
    サブコマンドとする（TASK-11.2 で `android` を追加できる構造。未対応値は usage エラーとして拒否する）。
  - iOS 固有の実処理は **`scripts/ios/build-simulator-app.sh`**（TASK-11.1-S2/S3、#221・#222）へ
    委譲します（TASK-11.1、Issue #486 で配線）。同スクリプトは位置引数によるフェーズ選択
    （`build` / `bundle` / `sign` / `ipa` / `verify`。引数なしは従来どおり全フェーズ実行）に対応し、
    CLI は各フェーズを個別に呼び出して失敗を終了コード（4/5/6）へマップします。

### フェーズ分割（関数単位で固定化）

| 関数 | 役割 | 委譲先・状態 |
|------|------|---------|
| `check_environment` | 前提検査（OS・必須コマンドの存在確認。非 macOS + `FANDHE_IOS_APP_BINARY` 指定時は構造検証モードへ切り替え） | 本 CLI が自前で判定 |
| `cross_build_rust` | `build-simulator-app.sh build` へ委譲（Rust クロスビルド） | `build-simulator-app.sh` |
| `build_app_bundle` | `build-simulator-app.sh bundle` へ委譲（`.app` 組み立て） | `build-simulator-app.sh` |
| `sign_adhoc` | `build-simulator-app.sh sign` へ委譲（ad-hoc 署名・署名検証） | `build-simulator-app.sh` |
| `package_ipa` | `build-simulator-app.sh ipa` → `verify` へ委譲（`.ipa` 生成・構造検証） | `build-simulator-app.sh` |
| `install_simulator` | `simctl install`/`launch`（オプトイン、既定 skip） | `--install` 指定時のみ到達。`simctl` が macOS 実機・シミュレータランタイム前提のため未提供で、到達時は非 0 終了する（TASK-11.1-S4 / #223、人間担当） |

## 3. 引数仕様

```
使い方: scripts/build/build-and-sign.sh <platform> [options]

platform:
  ios              iOS シミュレータ版 .app/.ipa 生成 + ad-hoc 署名（本タスク系列）
                   （未対応値は usage エラーとして拒否する。android の追加は TASK-11.2）

options:
  --release          リリースビルド（既定でこの動作。
                     scripts/ios/build-simulator-app.sh の FANDHE_IOS_PROFILE 既定値 release に
                     合わせている）
  --debug            デバッグビルド（既定は release のため、debug が必要な場合に指定）
  --output-dir <dir> 成果物出力先（既定: target/build-and-sign/<platform>/）
  --install          シミュレータへのインストール・起動確認を行う（既定: 行わない。
                     安全側に倒し、明示的な --install 指定でのみ有効化する設計とした）
  --dry-run          実行せず、実行予定ステップとコマンド計画を表示して exit 0
                     （Linux CI・非 macOS 環境での構造検証用）
  -h, --help         usage 表示して exit 0
```

- 環境変数上書き（既存慣行に合わせ `FANDHE_` 接頭辞）: `FANDHE_IOS_SIM_TARGET`（既定
  `aarch64-apple-ios-sim`）・`FANDHE_IOS_PROFILE`（既定 `release`。`release`/`debug` のみ対応）・
  `FANDHE_IOS_APP_BINARY`（既定は空。非空の場合は `build_app_bundle` が `cargo build` をスキップし、
  指定パスのバイナリをそのまま使う）・`FANDHE_BUILD_OUTPUT_DIR`・`FANDHE_IOS_OUT_DIR`（Issue #486 で
  追加。CLI が解決済みの `--output-dir` を `build-simulator-app.sh` の出力先へ渡すための注入点。
  CLI 経由でない単体実行時の既定値は従来どおり `target/ios-sim`）・`FANDHE_IOS_SKIP_SIGN`（既定は空
  = 署名する。値 `1` のみ許容し、明示 opt-in で署名フェーズをスキップする。非 macOS 構造検証モードでは
  CLI 側が自動で `1` を export する）。
  - 環境変数名・既定値は `scripts/ios/build-simulator-app.sh`（TASK-11.1-S2/S3 / #221・#222）の
    呼び出し契約に合わせている。同スクリプトが実処理を担う対象パッケージは `fandhe-egui-proxy` に
    固定されており、`FANDHE_IOS_PACKAGES` のようなパッケージ選択用の環境変数は提供しない
    （`scripts/ios/build-cross-targets.sh` の `FANDHE_IOS_TARGETS`/`FANDHE_IOS_PACKAGES` は
    `fandhe-core`/`fandhe-swiftui-bridge` ライブラリ向けの別スクリプトの契約であり、本 CLI の
    `ios` サブコマンドが委譲する `build-simulator-app.sh` とは別物）。
- 優先順位は「CLI オプション > 環境変数 > 既定値」。

### 非 macOS 環境での挙動（構造検証モード、Issue #486）

- **非 macOS + `FANDHE_IOS_APP_BINARY` 指定あり**: `check_environment` が「構造検証モード」へ切り替え、
  Xcode/macOS 必須部分（`cargo build` 実ビルド・`codesign` 署名）を明示スキップする旨をログ出力した上で
  `FANDHE_IOS_SKIP_SIGN=1` を自動 export して続行します。完了時にも「未署名・構造検証専用・実配布不可」
  であることをサマリで明示します（ユーザーが `FANDHE_IOS_SKIP_SIGN` に `1` 以外の非空値を設定済みの
  場合は前提検査エラーとして拒否します）。
- **非 macOS + `FANDHE_IOS_APP_BINARY` 未指定**: 実ビルドができないため exit 3（環境前提エラー）とし、
  スタブバイナリの用意方法を案内するメッセージを表示します（黙って偽の成果物を作らない安全側の設計）。
- **macOS**: 上記の分岐は適用されず、従来どおり `cargo`/`rustup`/`codesign` 等の不在を検査します
  （`FANDHE_IOS_APP_BINARY` 指定時は `cargo`/`rustup` を、`FANDHE_IOS_SKIP_SIGN=1` 指定時は
  `codesign` を必須対象から除外する）。

### `--skip-install` ではなく `--install` を採用した理由

計画段階では `--skip-install`（既定でインストールを実行し、明示的に無効化する）と `--install`
（既定でインストールを行わず、明示的に有効化する）の 2 案を比較しました。シミュレータへの
インストール・起動は副作用が大きく、CI（ubuntu-latest・macOS ランナー不在）や開発者のローカル環境を
不用意に変更しないことを優先し、**安全側のデフォルト**として `--install`（オプトイン）を採用しています。

## 4. 終了コード体系（REQ-11「終了コードで成否を判別できる」への対応）

| code | 意味 |
|------|------|
| 0 | 成功（dry-run 含む） |
| 2 | usage エラー（未知の platform・未知のオプション・引数不足） |
| 3 | 環境前提エラー（非 macOS で `ios` 実行、`cargo`/`rustup`/`python3`/`codesign` 等の不在） |
| 4 | ビルド失敗（`cross_build_rust` / `build_app_bundle` / `install_simulator`） |
| 5 | 署名・署名検証失敗（`sign_adhoc`） |
| 6 | パッケージング（`.ipa` 生成）失敗（`package_ipa`） |

各フェーズ関数は委譲先スクリプト（`scripts/ios/build-simulator-app.sh`）の非 0 終了を検知して
該当コードで exit します。`install_simulator` のみ、`--install` 指定時にのみ到達する未実装スタブです
（TASK-11.1-S4 / #223、macOS 実機・人間担当）。

## 5. PoC-3 実証ステップとの対応

`docs/spec/03-poc/store-submission/pipeline/build-and-sign.sh` で実証済みの手順と、
本 CLI のフェーズ関数との対応は以下のとおりです。

| PoC-3 のステップ | 本 CLI のフェーズ関数 |
|-----------------|----------------------|
| [1/4] Rust ライブラリのクロスビルド | `cross_build_rust` |
| [2/4] swiftc ビルド・`.app` 組み立て | `build_app_bundle` |
| [2/4] `codesign --sign -`（ad-hoc 署名） | `sign_adhoc`（前半） |
| [2/4] `Payload/` 構造での `.ipa` 化 | `package_ipa` |
| [4/4] `codesign --verify` による署名検証 | `sign_adhoc`（後半） |
| （PoC-3 になし。REQ-11 受け入れ基準の「シミュレータへインストール・起動できる」に対応） | `install_simulator`（オプトイン） |

PoC-3 は macOS 単体 `.app` の生成も行っていますが、本 CLI の `ios` サブコマンドは REQ-11 の
受け入れ基準（iOS シミュレータ版 `.ipa`）に対象を絞っています。macOS 単体アプリの配布は
本タスク系列のスコープ外です。

> **注記**: 本 CLI は `swiftc`/`xcrun` を使わず、`cargo build --target aarch64-apple-ios-sim` の成果物
> （`fandhe-egui-proxy` パッケージの `fandhe-egui-proxy-desktop` バイナリ）を直接バンドルする。
> 上表の PoC-3 側「swiftc ビルド」は PoC-3 の手順であり、本 CLI は cargo build のみで完結する。
> `check_environment` の必須コマンドもこれに合わせ `cargo`/`rustup`/`python3`/`codesign` とする。

## 6. Android（TASK-11.2）への同型拡張方針

- 第 1 位置引数のサブコマンドに `android` を追加し、`case` 文の分岐を追加するのみで拡張できる構造と
  しています（引数解析・usage・終了コード体系は共通化し、フェーズ関数の中身のみプラットフォーム別に
  実装する想定）。
- Android 側の環境変数命名は `FANDHE_ANDROID_*` を踏襲し、`scripts/android/build-android.sh` の
  `cargo-ndk` 呼び出しへ委譲する構成を想定しています（詳細設計は TASK-11.2 側で行う）。
- 終了コード体系（2〜6）はプラットフォーム非依存で共通利用します。

## 7. ad-hoc 署名の境界（REQ-11 と REQ-12 の切り分け）

- 本 CLI（および TASK-11.1/11.2）が扱うのは**開発用の ad-hoc 署名まで**です。`codesign --sign -`
  は開発・シミュレータ用途限定であり、実配布署名・notarization（Apple Developer 証明書・Team ID・
  App 用パスワードが必要）は REQ-12（Should、TASK-12.1 以降）のスコープです。
- `sign_adhoc` の実装はこの境界（ad-hoc 署名のみ・実配布用途への流用禁止）を
  CLI 出力（ログ）と実装コメントの双方に明記する。

## 8. スコープ外・申し送り

- **`install_simulator`（`simctl install`/`launch`）の実装** → TASK-11.1-S4（#223、macOS 実機・
  人間担当）。`simctl` 実行はシミュレータランタイムを持つ macOS 実機前提のため、実機検証タスクと
  一体で扱います。
- **macOS 実環境でのパイプライン動作確認・検証レポート・CI への組み込み要否判断**
  → TASK-11.1-S4（#223）。`.github/workflows/ci.yml` は ubuntu-latest の Linux ランナーのみで
  macOS ランナーが存在しないため、CLI の実行検証ジョブ追加は本 issue では行っていません。
  dry-run 検証ジョブの CI 組み込み要否は S4 側で判断してください。
- **Android への同型拡張** → TASK-11.2（別タスク、#224）。
- **実配布署名・notarization** → REQ-12（Should、別タスク、#249）。

## 9. セキュリティ考慮（OWASP Top 10 観点）

- **A03 インジェクション**: 引数解析は `eval` を使わず `case` によるホワイトリスト検証のみで行う。
  `--output-dir` 等の外部入力はシェル変数として扱い、コマンド文字列へ直接展開しない。
- **A05 セキュリティ設定ミス**: ad-hoc 署名（`codesign --sign -`）は開発・シミュレータ用途限定である
  ことを CLI 出力・本書双方に明記し、実配布への流用境界を固定する。
- **A02 / 秘密情報混入防止**: 本 CLI は証明書・Apple ID・Team ID・パスワード等の認証情報を
  一切扱わない（REQ-12 スコープ）。`FANDHE_*` 環境変数は非機密のビルド設定に限定する。
- **A08 整合性の不備**: 署名検証（`codesign --verify`）を必須フェーズ（`sign_adhoc`）として
  設計に組み込み、検証失敗を exit 5 で失敗扱いとする方針を固定化する。
