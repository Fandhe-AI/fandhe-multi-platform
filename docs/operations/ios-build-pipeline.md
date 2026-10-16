# 運用手順: iOS シミュレータ版ビルドパイプライン（`build-simulator-app.sh`）

- 対応: REQ-11（ストア提出パイプライン・開発署名まで）/ TASK-11.1（M3）系列。
  親 issue #219、sub-issue の責務対応は S1（#220: CLI 構成・引数設計）/
  S2（#221: `.app`/`.ipa` 生成）/ S3（#222: ad-hoc 署名）/ S4（#223: 本文書）。
  CLI からの委譲配線は Issue #486（TASK-11.1）で対応。
- 検証: S3（#222）・S4（#223）がこの手順書の実行根拠。
- アクセシビリティ検証: TASK-9.2（iOS 実機での `accesskit` VoiceOver 読み上げ検証）は development 署名での
  実機インストールが前提。本書はシミュレータ版（ad-hoc 署名）のみを扱い、実機インストール手順は含まない。

## 1. 位置付けとスコープ

`scripts/ios/build-simulator-app.sh` は、`fandhe-egui-proxy`（iOS エントリ実装済み）を
iOS シミュレータターゲットへビルドし、シミュレータ版アプリバンドル（`.app`）へ
ad-hoc 署名を施した上で `Payload/` 構造の `.ipa`（zip）を組み立てる。位置引数による
フェーズ選択（`build`/`bundle`/`sign`/`ipa`/`verify`。引数なしは従来どおり全フェーズ実行）
に対応しており、単体スクリプトとしても、`scripts/build/build-and-sign.sh`（CLI）からの
フェーズ単位の呼び出しにも対応する（Issue #486）。

`scripts/build/build-and-sign.sh ios` が単一コマンドとして `scripts/ios/build-simulator-app.sh`
の各フェーズを委譲呼び出しし、失敗を CLI の終了コード体系（4/5/6）へマップする
（Issue #486）。

REQ-11 受け入れ基準「単一コマンドから iOS シミュレータ版 `.ipa`（ad-hoc 署名）が
生成され、シミュレータへインストール・起動できる」のうち、「単一コマンド」
「ad-hoc 署名」の両部分は macOS 実機上で `scripts/build/build-and-sign.sh ios` を
実行することで達成される。実機での実行は §5 の人間担当手順で行う。「シミュレータへの
インストール・起動」（`install_simulator` / `--install`）は TASK-11.1-S4（#223）・
人間担当のスコープである。本書は CLI 経由・単体スクリプト経由の両方の利用手順を扱う。

## 2. 前提環境

| 項目 | Linux（本開発環境） | macOS（実機検証に必須） |
|------|---------------------|--------------------------|
| `cargo build --target aarch64-apple-ios-sim` | 不可（Apple SDK リンカが必要） | 可 |
| `cargo check --target aarch64-apple-ios-sim` | 可（`xcrun` が無いと live-activity の Swift 薄層生成を警告付きでスキップ。本番は `FANDHE_REQUIRE_LIVE_ACTIVITY=1` で失敗させる） | 可 |
| バンドル組み立て・`.ipa` 化ロジックの構造検証 | 可（`FANDHE_IOS_APP_BINARY` によるスタブ注入） | 可 |
| `codesign`（ad-hoc 署名） | 不可（`FANDHE_IOS_SKIP_SIGN=1` で構造検証専用にスキップ可能） | 可（既定で実行） |
| `xcrun simctl install`/`launch` | 不可 | 可 |
| `xcodegen` / `xcodebuild`（ライブアクティビティ拡張のビルド、Issue #295） | 不可（`FANDHE_IOS_APP_BINARY` 注入時は拡張を同梱せず、実行時は `Unsupported`） | 必須（無ければ build フェーズが失敗する） |
| `zip` コマンド | 通常なし（python3 `zipfile` へ自動フォールバック） | 標準搭載 |

Linux 環境では `rustup target add aarch64-apple-ios-sim` の上で `cargo check` までは
可能だが、実バイナリ生成・署名・シミュレータへのインストールは原理的に実行できない。
これらは「3. macOS 実機検証手順（人間担当）」に従うこと。

## 3. 環境変数インターフェース

`scripts/build/build-and-sign.sh`（CLI）は、これらの環境変数を経由して本スクリプトを
フェーズ単位で呼び出す（Issue #486）。単体スクリプトとして直接実行する場合も同じ契約に従う。

| 環境変数 | 既定値 | 説明 |
|----------|--------|------|
| `FANDHE_IOS_SIM_TARGET` | `aarch64-apple-ios-sim` | ビルド対象の Rust ターゲットトリプル |
| `FANDHE_IOS_PROFILE` | `release` | `release` または `debug` のみ対応。それ以外は成果物パス導出規約が崩れるため拒否（exit 1）。`debug` は cargo 組み込みプロファイル `dev` へ変換され、出力ディレクトリは `target/<target>/debug/` になる |
| `FANDHE_IOS_APP_BINARY` | 空 | 非空の場合、`cargo build` をスキップし指定パスのバイナリをそのままアプリ実行体として使う。Apple SDK リンカが使えない Linux 上で、バンドル組み立て・`.ipa` 化ロジックをスタブバイナリで構造検証するための注入点（**この経路では `FANDHE_IOS_PROFILE` は評価されない**。プロファイルは cargo ビルドの成果物パス導出にのみ使われ、バイナリ注入時は使用されないため） |
| `FANDHE_IOS_SKIP_SIGN` | 空（署名する） | 値 `1` のみ許容し、ad-hoc 署名フェーズをスキップする（未署名・構造検証専用、実配布不可）。それ以外の非空値は拒否（exit 1）。`codesign` が存在しない Linux 上で構造検証のみ行う場合に明示 opt-in で指定する。既定（署名する）時に `codesign` が見つからない場合は黙って未署名成果物を生成せず exit 1 とする。CLI 経由で非 macOS + `FANDHE_IOS_APP_BINARY` 指定時は CLI 側が自動で `1` を export する |
| `FANDHE_IOS_OUT_DIR` | `target/ios-sim`（単体実行時の既定値） | 成果物（`.app`/`.ipa`）の出力先ディレクトリ。CLI から呼び出す場合、CLI が解決した `--output-dir`（既定 `target/build-and-sign/<platform>/`）をここへ渡すための注入点（Issue #486）。単体実行時は指定しなければ従来どおり `target/ios-sim/` に生成される（後方互換） |

## 4. 使い方

### 4.1 macOS 実機での通常実行（CLI 経由、単一コマンド）

```bash
scripts/build/build-and-sign.sh ios
```

`build-and-sign.sh` が `check_environment` → 4 フェーズ（`cross_build_rust` /
`build_app_bundle` / `sign_adhoc` / `package_ipa`）→ `install_simulator`（`--install`
未指定時は skip）の順に `build-simulator-app.sh` をフェーズ委譲呼び出しする。
成果物は既定で `target/build-and-sign/ios/`（`--output-dir` で変更可）に生成される。

`build-simulator-app.sh` を単体で直接実行することも引き続き可能（後方互換）:

```bash
scripts/ios/build-simulator-app.sh
```

成果物:

- `<出力先>/FandheIosApp.app/`（実行体・`Info.plist`・ad-hoc 署名済み `_CodeSignature/`）
- `<出力先>/FandheIosApp-simulator.ipa`（署名済み `Payload/FandheIosApp.app/` を含む zip）
  - 単体実行時の `<出力先>` 既定値は `target/ios-sim/`。CLI 経由時は `--output-dir` の解決先。

### 4.2 Linux 上での構造検証（スタブバイナリ注入・署名スキップ）

CLI 経由（推奨・単一コマンド）:

```bash
FANDHE_IOS_APP_BINARY=/path/to/any/executable scripts/build/build-and-sign.sh ios
```

非 macOS + `FANDHE_IOS_APP_BINARY` 指定時、CLI の `check_environment` が「構造検証モード」へ
自動的に切り替え、`FANDHE_IOS_SKIP_SIGN=1` を自動 export した上で続行する（署名スキップの
明示ログと、完了時の「未署名・構造検証専用・実配布不可」サマリを出力する）。

単体スクリプト直接実行（フェーズ単位の切り分けデバッグ用）:

```bash
FANDHE_IOS_APP_BINARY=/path/to/any/executable FANDHE_IOS_SKIP_SIGN=1 scripts/ios/build-simulator-app.sh
# フェーズ単位: build / bundle / sign / ipa / verify を個別実行することも可能
FANDHE_IOS_APP_BINARY=/path/to/any/executable FANDHE_IOS_SKIP_SIGN=1 scripts/ios/build-simulator-app.sh build
```

いずれの経路も `cargo build`・署名を経ずにバンドル組み立て・`.ipa` 化・構造検証のロジックのみを
確認できる。実配布・実インストール検証には使えない（あくまでロジックのドライラン用）。
`codesign` が存在しない Linux 環境で単体スクリプトを直接実行する場合は
`FANDHE_IOS_SKIP_SIGN=1` を必ず指定すること（未指定のまま実行すると `codesign` 不在で
exit 1 になる。CLI 経由の場合はこの export を CLI 側が自動で行う）。

### 4.3 終了コード

| 状況 | 終了コード |
|------|-----------|
| 正常終了（バンドル・署名（スキップ時は除く）・`.ipa` 生成・構造検証すべて成功） | `0` |
| `Info.plist` が見つからない | `1` |
| `FANDHE_IOS_APP_BINARY` 指定時、指定パスにファイルが存在しない | `1` |
| `FANDHE_IOS_PROFILE` が `release`/`debug` 以外（ビルド経路使用時のみ検証される） | `1` |
| `FANDHE_IOS_SKIP_SIGN` が空/`1` 以外 | `1` |
| ビルド成果物が期待パスに見つからない | `1` |
| 署名要求時（既定）に `codesign` が見つからない | `1` |
| `codesign --sign`/`codesign --verify` が失敗 | `1` |
| `.ipa` 生成・構造検証（署名済み時は `_CodeSignature/CodeResources` の検証を含む）に失敗 | `1` |
| `zip`/`unzip`/`python3` のいずれも見つからず zip 化・検証ができない | `1` |

終了コードのみで成否を判別可能（REQ-11 の CLI 非機能要件に対応。標準出力にも
`[build-simulator-app] エラー: ...` 形式でエラー内容を出力する）。

### 4.4 冪等性

同一コマンドを繰り返し実行しても、`target/ios-sim/` 配下は毎回作り直され
（`rm -rf` 後に再生成）、失敗残骸を残さず同一の成果物になる。

## 5. macOS 実機検証手順（人間担当・チェックリスト）

以下は macOS 実機・Xcode 環境がなければ実行できないため、人間担当として実施すること
（CLAUDE.md「実機検証」規約）。

> シミュレータへの実インストール・起動・スクリーンショット取得までを 1 コマンドで
> 実施する場合は `scripts/ios/verify-simulator-render.sh`（TASK-1.4 / Issue #229）を
> 使う。本チェックリストの手動手順（`xcrun simctl` 呼び出し）と等価な処理を統合済み。

- [ ] macOS 上で `rustup target add aarch64-apple-ios-sim` を実行する。
- [ ] `scripts/ios/build-simulator-app.sh` を実行し、`cargo build` が実際に成功することを
      確認する（Linux 環境では原理的に検証不能）。
- [ ] 生成された `target/ios-sim/FandheIosApp-simulator.ipa` を `unzip -l` で確認し、
      `Payload/FandheIosApp.app/` 配下にディレクトリエントリを含む zip 構造になっている
      ことを確認する（macOS 標準 `zip -qr` はディレクトリエントリを含む一方、Linux の
      python3 `zipfile` フォールバックはファイルエントリのみのため、両者の zip バイト列は
      異なる。実インストール可否は **macOS 生成物を正とする**）。
- [ ] `scripts/ios/build-simulator-app.sh`（`FANDHE_IOS_SKIP_SIGN` は未指定 = 既定で
      署名する）を実行し、`codesign --sign - --force target/ios-sim/FandheIosApp.app` に
      よる ad-hoc 署名が実際に成功することを確認する（Linux 環境では原理的に検証不能。
      Linux 上での呼び出し順序・分岐ロジックは fake shim により
      確認済み）。
- [ ] `codesign --verify --verbose target/ios-sim/FandheIosApp.app` で署名を検証する
      （スクリプト内でも自動実行されるが、実機での独立確認として実施する）。
- [ ] `xcrun simctl list devices` でシミュレータ UDID を確認し、
      `xcrun simctl boot <UDID>` でシミュレータを起動する。
- [ ] `xcrun simctl install <UDID> target/ios-sim/FandheIosApp.app` でインストールする
      （`.app` を直接インストールする経路。`.ipa` からのインストールを検証する場合は
      別途 unzip して `.app` を取り出す）。
- [ ] `xcrun simctl launch <UDID> com.fandhe.iosapp` でアプリを起動し、クラッシュせず
      画面が表示されることを確認する。
- [ ] 上記結果を Issue #223 へ記録する。

## 6. セキュリティ・配布に関する注意

- 本スクリプトが生成する `.ipa` は既定で **ad-hoc 署名済み**（`FANDHE_IOS_SKIP_SIGN=1`
  指定時のみ未署名）であり、いずれも **シミュレータ検証専用・実配布不可**である。
  実配布署名・notarization は REQ-12（別スコープ、認証情報が必要）で扱う。
- 証明書・署名鍵・Apple ID 等の認証情報は本パイプラインでは一切扱わない
  （ad-hoc 署名は `codesign --sign -` で鍵不要）。
- 生成物はすべて `target/` 配下（`.gitignore` の `/target` で除外済み）に出力され、
  リポジトリへ混入しない。

## 7. 既知の制約・残課題

- ライブアクティビティの Widget Extension（`FandheLiveActivity.appex`、Issue #295）はシミュレータ向け
  バンドルにだけ `PlugIns/` として同梱し、拡張 → アプリの順に ad-hoc 署名する。実機向けは拡張の署名手順が
  無く同梱しないため、開始は `Unsupported` を返す（追跡は #293）。

- 通常は「4.1 macOS 実機での通常実行」の CLI 経由コマンド（Issue #486）を使うこと。
  単体スクリプト直接実行はフェーズ単位のデバッグ・切り分け用途として利用できる。
- `install_simulator`（`simctl install`/`launch`、`--install` 指定時のみ到達）は
  TASK-11.1-S4（#223）・人間担当のスコープであり未実装である。
- 本環境（Linux）では macOS 実機でのビルド・署名の実実行・`simctl install`/`launch` に
  よる実インストール・起動確認ができない（§5 参照、人間担当）。Linux 上での署名
  フェーズの呼び出しロジック・分岐は fake codesign shim による検証で担保している。
