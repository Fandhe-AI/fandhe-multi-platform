# Flutter 最小比較アプリ

fandhe `sample_screen()`（`crates/fandhe-core/src/ui/sample.rs`）と同じ画面構成を
Flutter で再現した性能比較用の計測専用フィクスチャです。`platforms/` 配下の配布用
Android プロジェクトとは独立しており、`benchmarks/memory/react-native/`（#656 S1）
と対になります。

## 目的・スコープ

- 親 Issue #647（RN を上回る＝Must / Flutter は努力目標）の実測材料として、
  Flutter 版の最小比較アプリを提供する
- 本 Issue（#657 / S2）の範囲は release APK をビルドできる状態までで、
  **計測スクリプトへの組み込みは含まない**（S3 #658）。実機計測・判定も対象外
  （S4 #659 / #660）

同梱アイコン（`android/app/src/main/res/mipmap-*/ic_launcher.png`）は `flutter create`
のアプリテンプレート（Flutter 3.38.5）由来の既定アイコン。flutter/flutter は BSD-3-Clause。

## 画面構成対応表

`sample_screen()` の 6 要素と `lib/main.dart` の対応関係です。

| # | fandhe `sample_screen()` | Flutter ウィジェット |
| --- | --- | --- |
| 1 | `.text("代表 1 画面サンプル")`（見出し） | `Text('代表 1 画面サンプル')` |
| 2 | `.text_field("メモ", get_memo, set_memo)` | `TextField(decoration: InputDecoration(hintText: 'メモ'))` |
| 3 | `.button(1, "カウントを増やす", increment_count)` | `ElevatedButton('カウントを増やす')` → `setState(() => _count++)` |
| 4 | `.button(2, "項目を追加", add_item)` | `ElevatedButton('項目を追加')` → `setState(() => _items.add(...))` |
| 5 | `.list(expand_items)` | `Expanded(ListView.builder(...))` |
| 6 | `.dynamic_text(count_label)`（末尾、Issue #581） | `Text('カウント: $_count')`（末尾） |

## 前提

- Flutter 3.38.5-stable（`benchmarks/memory/flutter/mise.toml` で固定。ディレクトリ配下で
  `mise install` すると計測条件どおりの SDK が入る）
- Android SDK: `source scripts/android/android-env.sh` で JDK 21 を通す。
  compileSdk / NDK バージョンは `android/app/build.gradle.kts` の
  `flutter.compileSdkVersion` / `flutter.ndkVersion`（Flutter 側の要求バージョン）
  に従う。`scripts/android/versions.env`（fandhe-core クロスビルド用 SSOT）とは
  独立した Flutter 側の管理下にあり、版が異なっていても `versions.env` は変更しない
  （`sdkmanager` で不足分のみ明示導入する）。Flutter 3.38.5 既定値は
  compileSdk 36 / targetSdk 36 / minSdk 24 / NDK 28.2.13676358 で、
  Gradle の暗黙自動導入に頼らず事前に明示導入する（security.md A05/A06）:

  ```bash
  sdkmanager --sdk_root="$ANDROID_HOME" "platforms;android-36" "ndk;28.2.13676358"
  ```

## ビルド手順

```bash
cd benchmarks/memory/flutter
flutter pub get --enforce-lockfile
flutter build apk --release --target-platform android-arm64
```

生成物: `build/app/outputs/flutter-apk/app-release.apk`

実機（Pixel 10a）へのインストールは人間担当です。

```bash
adb install -r build/app/outputs/flutter-apk/app-release.apk
```

## 逸脱事項（S1 react-native 版との方針差）

- **Gradle wrapper 同梱**: クリーン checkout から入口なしにビルド不可になるため、
  `gradlew` / `gradlew.bat` / `gradle-wrapper.jar`
  （Flutter SDK 同梱の `bin/cache/artifacts/gradle_wrapper` 由来）をバージョン管理する。
  S1 react-native 版とは異なり `android/.gitignore` では除外していない
- **release 署名**: AGP 既定の debug 署名（`signingConfigs.getByName("debug")`）を
  使用。開発用の暫定署名で、ストア提出用の署名ではない
- **arm64-v8a のみ**: `--target-platform android-arm64` で指定する。
  Gradle 側の `abiFilters` はテンプレート既定のまま変更していない
- **`INTERNET` 権限**: release ビルドが読む `android/app/src/main/AndroidManifest.xml`
  には含まれない（`debug` / `profile` 用 manifest にのみ含まれる、Flutter 既定）
- **Impeller / AOT**: レンダラ・コンパイルモードはテンプレート既定のまま変更していない

## 検証済み事項

- `flutter analyze`: 警告ゼロ
- `flutter test`: `test/widget_test.dart` 1 本 PASS
- `flutter build apk --release --target-platform android-arm64`: ビルド成功
- `aapt2 dump badging` で `package: name='com.fandhe.metrics.flutter_sample'`、
  `uses-permission` に `INTERNET` が含まれないこと、`native-code: 'arm64-v8a'`
  のみであることを確認済み
