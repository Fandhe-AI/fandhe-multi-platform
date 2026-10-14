# React Native 最小比較アプリ

`#647`（性能比較の再定義、RN を上回ることを Must / Flutter は努力目標）の S1（Issue #656）。
`crates/fandhe-core/src/ui/sample.rs::sample_screen()` と同一画面構成の React Native アプリを
Pixel 10a 向けにビルドし、アイドル時メモリ等の実測比較材料にするための計測専用フィクスチャ。

`platforms/`（ストア提出パイプライン）や `benchmarks/coldstart/`（コールドスタート比較）とは
独立しており、本 issue の範囲は「release APK をビルドできる状態」まで。計測スクリプトへの組み込み
（`scripts/metrics/android-memory/measure-idle-memory-device.sh` の対象追加）は S3（#658）、
実機計測・判定は S4（#659）で扱う。iOS 版は Apple Developer Program 取得（#12）後に着手する。

同梱アイコン（`android/app/src/main/res/mipmap-*/ic_launcher{,_round}.png`）は React Native 0.87.0 の
テンプレート（`@react-native-community/template`）由来の既定アイコン。同テンプレートは MIT。

## 画面構成対応表

`App.tsx` は `sample_screen()` の 6 要素をこの順序のまま `useState` のみで再現する。

| # | `sample_screen()`（fandhe） | `App.tsx`（React Native） |
|---|------------------------------|------------------------------|
| 1 | `.text("代表 1 画面サンプル")` | `<Text>` 見出し |
| 2 | `.text_field("メモ", ...)` | `<TextInput placeholder="メモ">` |
| 3 | `.button(..., "カウントを増やす", increment_count)` | `<Button title="カウントを増やす">` |
| 4 | `.button(..., "項目を追加", add_item)` | `<Button title="項目を追加">` |
| 5 | `.list(expand_items)`（`item-N`） | `<FlatList>`（`item-N`） |
| 6 | `.dynamic_text(count_label)`（末尾、Issue #581） | `<Text>カウント: {count}</Text>`（末尾） |

`scripts/metrics/android-memory/native-sample`（Jetpack Compose 版）はカウント値をボタンラベルに
埋め込んでおり本アプリとは表示位置が異なるが、本アプリは `sample_screen()` の要素順序（動的テキスト
末尾）をそのまま踏襲する。

## 前提

- Node: `react-native@0.87.0` の `engines.node` 要件（`^22.13.0 || ^24.3.0 || >= 26.0.0`）を満たすこと
- Android ビルド環境: `source scripts/android/android-env.sh` で `versions.env` の SSOT（JDK 21）を
  読み込み `JAVA_HOME` を反映する。`./gradlew` は `android/gradle/wrapper/gradle-wrapper.properties`
  同梱の Gradle（後述の逸脱事項参照。`versions.env` の Gradle 9.6.1 とは別系統）で実行されるため
  `GRADLE_HOME` 側の PATH は使わない
- 追加 SDK パッケージ: 本アプリは RN 0.87.0 テンプレート既定の `compileSdk 37` /
  `buildToolsVersion 37.0.0` / `ndkVersion 27.1.12297006` を要求する。これは `versions.env`
  （compileSdk 35 / build-tools 36.0.0 / NDK 27）より新しい。`versions.env` は
  `platforms/android`（配布用アプリ）の SSOT であり、本アプリは RN テンプレート既定に固定する
  独立フィクスチャ（後述の逸脱事項参照）のため `versions.env` 側は変更せず、`sdkmanager` で
  個別に明示導入する（Gradle の暗黙自動導入は使わない）:

  ```bash
  sdkmanager --sdk_root="$ANDROID_HOME" \
    "platforms;android-37" "build-tools;37.0.0" "ndk;27.1.12297006"
  ```

## ビルド手順（release APK）

```bash
cd benchmarks/memory/react-native
npm ci --ignore-scripts
cd android
./gradlew assembleRelease
# 生成物: app/build/outputs/apk/release/app-release.apk
adb install -r app/build/outputs/apk/release/app-release.apk   # Pixel 10a、実機は人間担当
```

`npm ci --ignore-scripts` は npm 依存導入時のライフサイクルフック実行を避けるための固定運用
（`~/.claude/CLAUDE.md` の npm 供給網対策）。`package-lock.json` を追跡しているため再現可能。

## 逸脱事項（既存比較アプリとの差分）

- **Gradle wrapper を同梱する**: `benchmarks/coldstart/android-native` は wrapper 非同梱で
  `versions.env` の Gradle に委ねているが、本アプリは RN の Gradle Plugin が要求する
  Gradle / AGP バージョンが厳密なため、RN テンプレート同梱の wrapper（`gradlew` /
  `gradle/wrapper/*`、`distributionSha256Sum` 検証つき）をそのまま追跡する
- **release は AGP 既定の debug 署名**: RN テンプレート既定はプロジェクト同梱の
  `debug.keystore` を明示参照するが、リポジトリ全体の `.gitignore` が `*.keystore` を一律
  除外しているため、`android-native` と同様に AGP 既定（`~/.android/debug.keystore`、
  存在しなければ AGP が自動生成）へ寄せている。計測専用でストア配布物ではない
- **Hermes / New Architecture は有効のまま**: RN の代表的な本番構成で比較するため、
  テンプレート既定を変更しない
- **`INTERNET` 権限は debug ビルドのみ**: Metro 接続に必要な `INTERNET` は
  `android/app/src/debug/AndroidManifest.xml` にのみ置き、計測対象の release ビルド
  （`android/app/src/main/AndroidManifest.xml`）には持たせない
- **ABI は arm64-v8a のみ**: `android/gradle.properties` の `reactNativeArchitectures` を
  Pixel 10a（arm64）に合わせて絞り込んでいる

## 依存の絞り込み・監査状況

`devDependencies` は Android release ビルドに必須な 5 件（`@react-native-community/cli` /
`cli-platform-android` / `@react-native/gradle-plugin` / `@react-native/metro-config` /
`@react-native/babel-preset`）のみとし、テンプレート既定の eslint / jest / prettier /
TypeScript 一式は削っている（供給網リスクの縮小。既知の侵害系譜 keyv / cacheable /
flat-cache / file-entry-cache は `package-lock.json` に含まれないことを確認済み）。

`npm audit`（2026-08-29 時点）は high 8 件を報告するが、いずれも `react-native@0.87.0` が
内部で使う `metro`（開発時バンドラ）系の推移依存（`image-size` の ICNS/JXL/HEIF パーサ DoS）
であり、固定した `react-native` バージョン自体を下げない限り解消できない。release APK の
実行時コード（JS バンドル・ネイティブコード）には含まれず、ローカルの開発用ビルドツール
にのみ影響するため、`react-native` バージョン追随の課題として記録するにとどめる。

## 検証済み事項

- `npm ci --ignore-scripts` → `./gradlew assembleRelease` で `app-release.apk` を生成できることを
  ローカル（macOS、上記追加 SDK パッケージ導入後）で確認済み
- `aapt2 dump badging` で `package: name='com.fandhe.metrics.reactnative'`、
  `uses-permission` に `INTERNET` が含まれないこと、`native-code: 'arm64-v8a'` を確認済み
- 実機（Pixel 10a）へのインストール・起動・6 要素の表示確認は **人間担当**（S4 #659 / #660）
