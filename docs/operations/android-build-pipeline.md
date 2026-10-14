# 運用手順: Android クロスビルド・成果物検証パイプライン

- 対応: REQ-11（ストア提出パイプライン・開発署名まで）/ TASK-11.2（M3）系列・TASK-11.3（M4）。
  親 issue #224、sub-issue の責務対応は S1（#225: パイプライン構成設計）/
  S2（#226: cargo-ndk 成果物の Gradle 統合実装。`build-aab.sh`・`platforms/android/`）/
  S3（#227: 開発署名鍵による署名処理）/ S4（#228: 本文書）。実機/エミュレータへの
  インストール・起動確認は TASK-11.3（#246、`scripts/android/verify-aab-install.sh`）が担う。
- 設計書: `docs/architecture/android-aab-pipeline.md`（S1 成果物。パイプライン全体の
  4 フェーズ構成・sub-issue 分界を定義）。
- 検証: TASK-11.2-S4（#228）・TASK-11.3（#246）がこの手順書の実行根拠。

## 1. 位置付けとスコープ

`scripts/android/build-aab.sh` から単一コマンドで `.aab` を生成できる。
`FANDHE_ANDROID_KEYSTORE` 環境変数を指定すれば開発用署名鍵で署名済みの `.aab` を、
未指定であれば未署名の `.aab` を生成する（詳細は `docs/operations/android-aab-signing.md`）。

REQ-11 受け入れ基準「単一コマンドから Android `.aab`（開発用署名鍵）が生成され、
エミュレータへインストール・起動できる」のうち、「単一コマンドから `.aab` が生成され、
開発用署名鍵で署名される」までを本パイプラインが担う。「エミュレータ（実機を含む）へ
インストール・起動できる」は TASK-11.3（#246、`scripts/android/verify-aab-install.sh`）が
担う（4.4 節参照）。

本書は、`scripts/android/build-android.sh`（cargo-ndk による `fandhe-core` のみの
クロスビルド）・`scripts/android/build-aab.sh`（S2/S3、`.aab` 生成・開発署名）・
`scripts/android/verify-android-artifacts.sh`（`.so` / `.aab` 成果物の検証）・
`scripts/android/verify-aab-install.sh`（TASK-11.3、実機/エミュレータへのインストール・
起動確認）の 4 スクリプトの利用手順を扱う。

## 2. 前提環境

- `scripts/android/setup-android-env.sh` 実行済み（JDK 21.0.12+8 / Android SDK
  commandline-tools / NDK 27.3.13750724 / Gradle 9.6.1 / bundletool 1.18.3 /
  cargo-ndk 4.1.2 を導入）。セットアップ手順の詳細は
  `docs/environment/android-ndk-gradle-setup.md` 参照。
- バージョン・チェックサムは `scripts/android/versions.env` を単一の SSOT として参照する
  （個別ツールのバージョンをここに重複記載しない）。
- 本パイプラインは Linux 上で `.aab` 生成まで完結する（iOS と異なり Apple SDK 相当の
  クロスリンク制約がない）。macOS 実機・エミュレータでの追加確認は不要。
- 実機/エミュレータへのインストール・起動確認（`verify-aab-install.sh`）は端末接続が
  前提となるため、本開発環境では未接続時の fail-fast のみを担保する
  （4.4 節参照）。

## 3. 環境変数インターフェース

| 環境変数 | 既定値 | 説明 |
|----------|--------|------|
| `ANDROID_NDK_HOME` 等 | `android-env.sh` が `versions.env` から導出 | `setup-android-env.sh` 導入先を指す。`build-android.sh`・`build-aab.sh`・`verify-android-artifacts.sh` はいずれも本スクリプトを source する前提 |
| `RUST_ANDROID_TARGETS` | `aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`（`versions.env`） | cargo-ndk がビルドする 3 ABI（arm64-v8a / armeabi-v7a / x86_64）に対応する rustup/cargo の triple |
| `ANDROID_MIN_API` | `24`（`versions.env`） | cargo-ndk `--platform` フラグに渡す Android 最小 API レベル |
| `FANDHE_ANDROID_PROFILE` | `release`（`build-aab.sh` のみ） | `release` のみ対応。デバッグ署名鍵付きの `bundleDebug` を既定経路にしないための意図的な制限（security.md A05 対策） |
| `FANDHE_ANDROID_ABIS` | `"arm64-v8a armeabi-v7a x86_64"`（`build-aab.sh` のみ、空白区切り） | cargo-ndk へ渡す ABI 一覧。同じ一覧が `-PfandheAndroidAbis` として Gradle 側の `ndk.abiFilters` にも反映される |

`FANDHE_ANDROID_KEYSTORE` / `FANDHE_ANDROID_KEYSTORE_PASSWORD` / `FANDHE_ANDROID_KEY_ALIAS` /
`FANDHE_ANDROID_KEY_PASSWORD`（`build-aab.sh` / `verify-aab-install.sh` 共通の
インターフェース）は `docs/operations/android-aab-signing.md` 参照。CI
（`.github/workflows/ci.yml`）へは keystore シークレットを設定しない方針のため、CI 経路は
未署名 `.aab` の生成・検証のままとなる（4 節・5 節参照）。

## 4. 使い方

### 4.1 fandhe-core のみのクロスビルド

```bash
scripts/android/build-android.sh          # debug
scripts/android/build-android.sh --release
```

対象パッケージは `fandhe-core`（rlib のみ）。TASK-1.5-S1 時点からの既存スクリプトで、
本 issue（S4）では変更していない。

### 4.2 .aab の生成（S2/S3、`build-aab.sh`）

```bash
scripts/android/build-aab.sh                                    # 未署名 .aab
FANDHE_ANDROID_KEYSTORE=/path/to/dev.jks \
FANDHE_ANDROID_KEYSTORE_PASSWORD=xxxx \
  scripts/android/build-aab.sh                                  # 開発署名済み .aab
```

`fandhe-egui-proxy`（cdylib、TASK-1.5-S2 の Vulkan バックエンド分岐を含む）を 3 ABI 向けに
`cargo ndk` でビルドし、`target/android/jniLibs/` へ配置した上で
`gradle -p platforms/android :app:bundleRelease` を実行して
`platforms/android/app/build/outputs/bundle/release/app-release.aab` を生成する。
生成直後にスクリプト自身が (a) 各 ABI の `.so` 収載、(b) 要求外 ABI の混入なし、
(c) 署名有無（`FANDHE_ANDROID_KEYSTORE` の指定に応じて署名済み/未署名のいずれか）、
を検証する。開発署名鍵の生成手順・環境変数インターフェースは
`docs/operations/android-aab-signing.md`（S3、#227）参照。

`FANDHE_ANDROID_PROFILE`・`FANDHE_ANDROID_ABIS` は 3 節の環境変数インターフェース参照。

### 4.3 成果物検証（`verify-android-artifacts.sh`）

```bash
scripts/android/verify-android-artifacts.sh                    # debug（.so のみ）
scripts/android/verify-android-artifacts.sh --release           # release（.so のみ）
scripts/android/verify-android-artifacts.sh --release --aab     # release + .aab 検証
```

`--release`/`--aab` なしの `.so` 検証部分は、内部で `fandhe-core`（rlib）・
`fandhe-egui-proxy`（cdylib）の両方を 3 ABI 向けに単一の
`cargo ndk ... build -p fandhe-core -p fandhe-egui-proxy --lib --target-dir
target/android-verify` 呼び出しでビルドし、以下の成果物存在を確認する
（`--target-dir` で専用ディレクトリを使う理由は 4.6 節参照）。

- `target/android-verify/<triple>/<profile>/libfandhe_core.rlib`
- `target/android-verify/<triple>/<profile>/libfandhe_egui_proxy.so`

`--lib` を明示することで、`fandhe-egui-proxy` が併せ持つ `[[bin]]
fandhe-egui-proxy-desktop`（native/デスクトップ向け、Android 非対応）をビルド対象から
除外する。成果物が期待パスに見つからない場合は `fail()` により標準エラーへメッセージを
出力し、非 0 で終了する。

`--aab`（本 issue、S4 で追加）を付けると、`build-aab.sh` が生成した
`platforms/android/app/build/outputs/bundle/release/app-release.aab` を対象に
独立した事後検証を行う。

- `.aab` の存在（未生成時は「先に `build-aab.sh` を実行してください」と案内して非 0 終了）
- zip 構造妥当性（`unzip -l` / `python3 -m zipfile` フォールバック）
- `base/manifest/AndroidManifest.xml` の収載
- 3 ABI 分の `base/lib/<abi>/libfandhe_egui_proxy.so` の収載

`build-aab.sh` 自身も生成直後に同様の検証を内蔵しているが、`--aab` は
「生成済み成果物への独立した事後検証」として役割を分けている（生成ロジックの
cargo-ndk 呼び出し・Gradle 呼び出しは `build-aab.sh` 側にのみ存在し、二重管理しない）。

### 4.4 実機/エミュレータへのインストール・起動確認（TASK-11.3、`verify-aab-install.sh`）

```bash
FANDHE_ANDROID_KEYSTORE=/path/to/dev.jks \
FANDHE_ANDROID_KEYSTORE_PASSWORD=xxxx \
  scripts/android/verify-aab-install.sh
```

REQ-11 受け入れ基準「エミュレータへインストール・起動できる」を実証する。`.aab` は adb で
直接インストールできないため、Google 公式ツール bundletool
（`scripts/android/versions.env` の `BUNDLETOOL_*` が SSOT、`setup-android-env.sh` で導入）で
接続端末向けの `.apks`（split APK 集合）へ変換してからインストールする。

1. `build-aab.sh` へ委譲して開発署名済み `.aab` を生成（4.2 節と同一ロジック。二重実装しない）
2. `bundletool build-apks --connected-device`（同じ開発署名鍵で `.apks` 側も署名。
   `.aab` 自体の署名ブロックはデバイスのインストール検証には使われないため、
   インストールを通すには `.apks` 生成時にも鍵指定が必要）
3. `bundletool install-apks` でインストール
4. `adb shell am start` で起動し、`pidof` / `logcat --pid` / `dumpsys activity activities`
   による機械的起動判定（`verify-android-launch.sh`、TASK-1.6 と同じロジック）
5. `adb exec-out screencap` で証跡取得、既定では検証後に `adb uninstall`

REQ-11 が「開発用署名鍵」を明示するため、`FANDHE_ANDROID_KEYSTORE` 未設定時は
未署名 `.aab` での代替検証を行わず fail-closed で終了する。デバイス（実機/エミュレータ）
未接続時もフェーズ 2 で fail-fast する。本ホストには KVM が無くエミュレータが実用速度で
動作しないため、実機（Pixel 10a）を主経路とする。CI 組み込みは
行わない（CI runner にデバイスが存在しないため対象外）。

### 4.5 終了コード

| 状況 | 終了コード |
|------|-----------|
| 正常終了（要求した検証項目すべて確認） | `0` |
| `cargo` / `cargo-ndk` / `gradle` / `adb` 未導入、`ANDROID_NDK_HOME`・`GRADLE_HOME`・`JAVA_HOME`・`BUNDLETOOL_JAR` 不在 | `1`（前提チェックで即時 `fail()`） |
| cargo-ndk ビルド・Gradle ビルド自体が失敗（コンパイルエラー等） | `1`（`set -euo pipefail` により子プロセスの終了コードを継承） |
| ビルド成功後、期待パスに成果物が見つからない（`.so` / `.aab` いずれも） | `1`（`fail()` が該当パスを明示） |
| `.aab` 内に要求外 ABI が混入（`build-aab.sh` のみ） | `1`（stale ABI 残置の疑いとして明示） |
| 不明な引数（`verify-android-artifacts.sh` のみ） | `1`（使い方を案内して `fail()`） |
| `FANDHE_ANDROID_KEYSTORE` 未設定・デバイス未接続（`verify-aab-install.sh` のみ） | `1`（fail-closed／fail-fast） |

### 4.6 冪等性

過去に「ビルド開始直前のマーカーより新しいか」で成果物の鮮度を再検証する実装だったが、
ソース差分がなく cargo-ndk 呼び出しが実質 no-op で完了するケースで偽陽性の失敗を起こして
いたため、現在は cargo 自身の終了コードにビルド成否判定を委ね、検証は「期待パスに成果物が
存在するか」のみに絞っている（`verify-android-artifacts.sh` 冒頭コメント参照）。同一コマンドを
繰り返し実行しても同じ結果になる。`build-aab.sh` は実行のたびに `target/android/jniLibs/` を
`rm -rf` してから ABI を再配置するため、過去に広い ABI 集合で実行した際の残置 ABI が
`.aab` へ混入することもない。

`verify-android-artifacts.sh` の release 検証ビルドは `--target-dir
target/android-verify` でメインの `target/` から隔離したディレクトリを使う。
メインの `target/` を共有すると `target/<triple>/release/libfandhe_core.rlib` が
見つからない事象が CI runner 環境で再現する（`fandhe-core` は
`fandhe-egui-proxy` の依存クレートでもあり、直前のステップ（`build-aab.sh` の
`-p fandhe-egui-proxy -o <jniLibs dir>` 呼び出し）の影響を受けていると推測されるが、
cargo・cargo-ndk 側の詳細な原因は特定できていない。参照: PR #483）。他ステップが
一切触れない専用ディレクトリへ隔離することで、本スクリプト自身が要求したビルド結果
のみが確実に存在する状態を保証する。`target/android-verify` 自体は本スクリプトの
実行間でキャッシュとして機能するため、2 回目以降の実行では冪等性・実行速度とも
損なわれない。

## 5. CI 統合について

`.github/workflows/ci.yml` の `android-build` ジョブは、`cargo ndk build（fandhe-core）`
（`build-android.sh`、debug）の後段に以下 2 ステップを実行する。

1. `bash scripts/android/build-aab.sh`（未署名 `.aab` 生成。keystore シークレットは CI に
   設定しないため、常に未署名経路を通る）
2. `bash scripts/android/verify-android-artifacts.sh --release --aab`（`.aab` の独立した
   事後検証。4.6 節の対策により、直前の `build-aab.sh` が `fandhe-egui-proxy` のみを
   ビルドしたことに起因する fandhe-core の release 成果物欠落を回避する。参照: PR #483）

`android-build` ジョブの `timeout-minutes` は 90 とする（初回実行時に AGP・Gradle
依存パッケージのダウンロードが発生するため）。使い捨て VM のため
`~/.gradle` はジョブ間で共有されない（導入先 `~/.fandhe/android` のみ `actions/cache` で復元する）。

## 6. 既知の制約・残課題

- `verify-android-artifacts.sh` と `build-android.sh` の cargo-ndk 呼び出しロジックが
  二重管理になっている件（TASK-1.5-S3 からの既知の引き継ぎ、
  `docs/architecture/android-aab-pipeline.md` 9 節でも言及）は本書・本 issue のスコープ外。
  別途 Issue で追跡するかはユーザー確認が必要なため、事実の記録のみに留める
  （`out-of-scope-tracking.md` 準拠）。
- `verify-aab-install.sh`（TASK-11.3 / #246）は本開発環境に実機・エミュレータが接続
  されていないため、デバイス検出フェーズでの fail-fast・keystore 未設定時の
  fail-closed までを担保する。実機での実インストール・起動確認は人間担当手順で行う。
- Android エミュレータ環境（`emulator` パッケージ・system-images）は、本ホストに
  KVM が無く実用速度で動作しない（TASK-1.6 / #230）ため対象外とする。実機
  （Pixel 10a）を主経路とする。
